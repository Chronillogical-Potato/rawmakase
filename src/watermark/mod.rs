//! Lightroom's export watermark: a text or graphic mark, laid over the
//! exported pixels after the render and before they are quantized, so a
//! 16-bit TIFF keeps its precision outside the mark and under a partly
//! transparent one. Watermarks are saved as presets in the data folder's
//! `watermarks`, a graphic one with its own copy of the image.
pub mod fonts;
mod raster;
use crate::develop::Rendered;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The Export dialog's choice that takes its text from the photo's
/// copyright, as Lightroom's Simple Copyright Watermark does.
pub const SIMPLE_COPYRIGHT: &str = "<simple-copyright>";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Style {
    #[default]
    Text,
    Graphic,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}
/// How large the mark is: a fraction of the photo's width, as wide as the
/// photo, or as large as fits it both ways. Insets are taken off first.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Size {
    Proportional(f32),
    Fit,
    Fill,
}
impl Default for Size {
    fn default() -> Self {
        Self::Proportional(0.2)
    }
}
/// One of nine points: (horizontal, vertical), each 0 start, 1 center, 2 end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor(pub u8, pub u8);
impl Default for Anchor {
    /// Lightroom's: bottom left.
    fn default() -> Self {
        Self(0, 2)
    }
}
/// A drop shadow under the text, as Lightroom's: offset and radius in
/// fractions of the text size, the angle in degrees (0 points right, 90 up).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shadow {
    pub enabled: bool,
    pub opacity: f32,
    pub offset: f32,
    pub radius: f32,
    pub angle: f32,
}
impl Default for Shadow {
    fn default() -> Self {
        Self {
            enabled: false,
            opacity: 0.5,
            offset: 0.04,
            radius: 0.04,
            angle: -45.,
        }
    }
}

/// A watermark preset.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Watermark {
    pub name: String,
    pub style: Style,
    pub text: String,
    /// Font family and style, as the font menus name them.
    pub family: String,
    pub face: String,
    pub align: Align,
    /// sRGB, 0–1.
    pub color: [f32; 3],
    pub shadow: Shadow,
    /// The preset's own copy of its image, by file name in
    /// `watermarks/images`.
    pub image: Option<String>,
    pub opacity: f32,
    pub size: Size,
    /// Horizontal and vertical inset, fractions of the photo's width and
    /// height.
    pub inset: [f32; 2],
    pub anchor: Anchor,
    /// Quarter turns counter-clockwise.
    pub rotation: u8,
}
impl Default for Watermark {
    fn default() -> Self {
        Self {
            name: String::new(),
            style: Style::Text,
            text: "Copyright".into(),
            family: fonts::INTER.into(),
            face: "Regular".into(),
            align: Align::Left,
            color: [1., 1., 1.],
            shadow: Shadow::default(),
            image: None,
            opacity: 1.,
            size: Size::default(),
            inset: [0.02, 0.02],
            anchor: Anchor::default(),
            rotation: 0,
        }
    }
}

/// A mark ready to lay over a photo: straight-alpha RGBA in output-encoded
/// sRGB, and where its top left corner goes.
pub struct Placed {
    pub x: i64,
    pub y: i64,
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<[f32; 4]>,
}

/// A watermark with what it needs loaded: its image decoded or its font
/// read, so an export can fail before rendering when either is missing.
pub struct Ready {
    watermark: Watermark,
    image: Option<image::Rgba32FImage>,
    font: Option<fonts::Font>,
}

pub fn dir() -> PathBuf {
    crate::storage::data_dir().join("watermarks")
}
fn images_dir() -> PathBuf {
    dir().join("images")
}

impl Watermark {
    /// Lightroom's Simple Copyright Watermark: the copyright as text, small,
    /// bottom left.
    pub fn simple_copyright(copyright: &str) -> Self {
        Self {
            name: "Simple Copyright Watermark".into(),
            text: copyright.into(),
            ..Default::default()
        }
    }
    /// Loads its image or font, from `images` for a graphic preset.
    pub fn ready_in(&self, images: &Path) -> Result<Ready> {
        let (image, font) = match self.style {
            Style::Graphic => {
                let name = self.image.clone().unwrap_or_default();
                let path = images.join(&name);
                ensure!(
                    !name.is_empty() && path.is_file(),
                    "Watermark image not found: {name}"
                );
                let decoded = image::ImageReader::open(&path)?
                    .with_guessed_format()?
                    .decode()
                    .with_context(|| format!("Watermark image not readable: {name}"))?;
                (Some(decoded.into_rgba32f()), None)
            }
            Style::Text => (None, Some(fonts::load(&self.family, &self.face)?)),
        };
        Ok(Ready {
            watermark: self.clone(),
            image,
            font,
        })
    }
    pub fn ready(&self) -> Result<Ready> {
        self.ready_in(&images_dir())
    }
    /// A graphic watermark whose image is gone, without reading it.
    pub fn image_missing(&self) -> bool {
        self.style == Style::Graphic
            && !self
                .image
                .as_ref()
                .is_some_and(|name| images_dir().join(name).is_file())
    }
}

impl Ready {
    /// The mark for a photo `width` × `height`, placed.
    pub fn place(&self, width: u32, height: u32) -> Option<Placed> {
        let w = &self.watermark;
        let (pw, ph) = (width as f32, height as f32);
        let room = (
            (pw * (1. - 2. * w.inset[0])).max(1.),
            (ph * (1. - 2. * w.inset[1])).max(1.),
        );
        let turned = w.rotation % 2 == 1;
        // The mark's own aspect, as shown after its rotation.
        let aspect = |mw: f32, mh: f32| if turned { mh / mw } else { mw / mh };
        let fit = |aspect: f32| -> (f32, f32) {
            match w.size {
                // Never larger than the photo, a turned mark included.
                Size::Proportional(p) => {
                    let mw = (pw * p.clamp(0.01, 1.)).min(room.0).min(room.1 * aspect);
                    (mw, mw / aspect)
                }
                Size::Fit => (room.0, room.0 / aspect),
                Size::Fill => {
                    let mw = room.0.min(room.1 * aspect);
                    (mw, mw / aspect)
                }
            }
        };
        let mut rgba;
        let (mut mw, mut mh);
        match (&self.image, &self.font) {
            (Some(image), _) => {
                let (iw, ih) = (image.width() as f32, image.height() as f32);
                let (tw, th) = fit(aspect(iw, ih));
                let (sw, sh) = if turned { (th, tw) } else { (tw, th) };
                let (sw, sh) = (sw.round().max(1.) as u32, sh.round().max(1.) as u32);
                let scaled =
                    image::imageops::resize(image, sw, sh, image::imageops::FilterType::Lanczos3);
                (mw, mh) = (sw as usize, sh as usize);
                rgba = scaled
                    .pixels()
                    .map(|p| {
                        let [r, g, b, a] = p.0;
                        [
                            r.clamp(0., 1.),
                            g.clamp(0., 1.),
                            b.clamp(0., 1.),
                            a.clamp(0., 1.),
                        ]
                    })
                    .collect();
            }
            (None, Some(font)) => {
                // Measured at a reference size, then drawn at the one that
                // gives the mark its width.
                let reference = raster::measure(font, &w.text, 100.)?;
                let (tw, _) = fit(aspect(reference.0, reference.1));
                let target = if turned {
                    tw * reference.0 / reference.1
                } else {
                    tw
                };
                let px = 100. * target / reference.0.max(1e-3);
                let text = raster::text(font, &w.text, px, w.align, w.color, &w.shadow)?;
                (mw, mh, rgba) = (text.width, text.height, text.rgba);
            }
            (None, None) => return None,
        }
        for _ in 0..w.rotation % 4 {
            (rgba, mw, mh) = rotate(&rgba, mw, mh);
        }
        let opacity = w.opacity.clamp(0., 1.);
        for p in &mut rgba {
            p[3] *= opacity;
        }
        let along = |slot: u8, size: f32, mark: usize, inset: f32| -> i64 {
            (match slot {
                0 => size * inset,
                1 => (size - mark as f32) / 2.,
                _ => size - mark as f32 - size * inset,
            })
            .round() as i64
        };
        Some(Placed {
            x: along(w.anchor.0, pw, mw, w.inset[0]),
            y: along(w.anchor.1, ph, mh, w.inset[1]),
            width: mw,
            height: mh,
            rgba,
        })
    }
    /// Lays the mark over `image`'s pixels.
    pub fn apply(&self, image: &mut Rendered) {
        if let Some(mark) = self.place(image.width, image.height) {
            composite(image, &mark);
        }
    }
}

/// Alpha-over in output-encoded sRGB: pixels the mark doesn't cover keep
/// their exact values.
pub fn composite(image: &mut Rendered, mark: &Placed) {
    let (w, h) = (image.width as i64, image.height as i64);
    for my in 0..mark.height as i64 {
        let y = mark.y + my;
        if y < 0 || y >= h {
            continue;
        }
        for mx in 0..mark.width as i64 {
            let x = mark.x + mx;
            if x < 0 || x >= w {
                continue;
            }
            let [r, g, b, a] = mark.rgba[(my as usize) * mark.width + mx as usize];
            if a <= 0. {
                continue;
            }
            let p = &mut image.pixels[(y * w + x) as usize];
            for (c, m) in p.iter_mut().zip([r, g, b]) {
                *c = m * a + *c * (1. - a);
            }
        }
    }
}

/// A quarter turn counter-clockwise.
fn rotate(rgba: &[[f32; 4]], w: usize, h: usize) -> (Vec<[f32; 4]>, usize, usize) {
    let mut out = vec![[0.; 4]; w * h];
    for y in 0..h {
        for x in 0..w {
            // (x, y) goes to (y, w - 1 - x) in an h-wide image.
            out[(w - 1 - x) * h + y] = rgba[y * w + x];
        }
    }
    (out, h, w)
}

/// Saved presets, by name.
pub fn presets() -> Vec<Watermark> {
    presets_in(&dir())
}
pub fn presets_in(dir: &Path) -> Vec<Watermark> {
    let mut found: Vec<Watermark> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok())
        .collect();
    found.sort_by_key(|w: &Watermark| w.name.to_lowercase());
    found
}
/// A preset's file name for its name.
fn file_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{}.json", safe.trim())
}
/// Saves `watermark` under its name in `dir`. A graphic preset takes its own
/// copy of `source`, the image chosen for it, when given.
pub fn save_in(dir: &Path, watermark: &Watermark, source: Option<&Path>) -> Result<Watermark> {
    ensure!(!watermark.name.trim().is_empty(), "Name the watermark");
    std::fs::create_dir_all(dir.join("images"))?;
    let mut saved = watermark.clone();
    if let Some(source) = source {
        let extension = source
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        ensure!(
            matches!(extension.as_str(), "png" | "jpg" | "jpeg"),
            "Choose a PNG or JPEG image"
        );
        let stem = file_name(&watermark.name);
        let name = format!("{}.{extension}", stem.trim_end_matches(".json"));
        std::fs::copy(source, dir.join("images").join(&name))?;
        saved.image = Some(name);
    }
    crate::storage::atomic_json(&dir.join(file_name(&saved.name)), &saved)?;
    Ok(saved)
}
pub fn save(watermark: &Watermark, source: Option<&Path>) -> Result<Watermark> {
    save_in(&dir(), watermark, source)
}
/// Deletes a preset and the image it owns.
pub fn delete_in(dir: &Path, watermark: &Watermark) -> Result<()> {
    let path = dir.join(file_name(&watermark.name));
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    if let Some(image) = &watermark.image {
        let image = dir.join("images").join(image);
        if image.exists() {
            std::fs::remove_file(image)?;
        }
    }
    Ok(())
}
pub fn delete(watermark: &Watermark) -> Result<()> {
    delete_in(&dir(), watermark)
}
/// Renames: `watermark` is saved under its new name, keeping its image, and
/// the preset named `old` goes.
pub fn rename_in(dir: &Path, old: &str, watermark: &Watermark) -> Result<Watermark> {
    let saved = save_in(dir, watermark, None)?;
    let old_path = dir.join(file_name(old));
    if old != saved.name && old_path.exists() {
        std::fs::remove_file(old_path)?;
    }
    Ok(saved)
}

#[cfg(test)]
mod tests;
