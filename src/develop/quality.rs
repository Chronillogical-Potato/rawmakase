//! Full-resolution detail processing shared by Fit, 100% regions and exports.
use crate::develop::{
    pipeline::Toned,
    preview_renderer::Stages,
    stage_cache::{BlurKey, LocalKey, StageCache},
};
use crate::{
    develop::{self, Geometry, Recipe, Rendered},
    raw::CameraImage,
};
use anyhow::{Result, ensure};
use rayon::prelude::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
    Ok(())
}

fn luminance(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}
pub fn fit_edge(width: u32, height: u32, viewport: [u32; 2]) -> u32 {
    let scale = (viewport[0].max(1) as f64 / width as f64)
        .min(viewport[1].max(1) as f64 / height as f64)
        .min(1.);
    ((width.max(height) as f64 * scale).round() as u32).max(1)
}
pub(crate) fn output_size(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    if max_edge == 0 || width.max(height) <= max_edge {
        return (width, height);
    }
    let scale = max_edge as f64 / width.max(height) as f64;
    (
        (width as f64 * scale).round().max(1.) as u32,
        (height as f64 * scale).round().max(1.) as u32,
    )
}
pub fn resize(image: Rendered, max_edge: u32) -> Rendered {
    if max_edge == 0 || image.width.max(image.height) <= max_edge {
        return image;
    }
    let (w, h) = output_size(image.width, image.height, max_edge);
    let buffer = image::Rgb32FImage::from_raw(
        image.width,
        image.height,
        image.pixels.into_iter().flatten().collect(),
    )
    .unwrap();
    let buffer = image::imageops::resize(&buffer, w, h, image::imageops::FilterType::Lanczos3);
    Rendered {
        width: w,
        height: h,
        pixels: buffer
            .into_raw()
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| [p[0].clamp(0., 1.), p[1].clamp(0., 1.), p[2].clamp(0., 1.)])
            .collect(),
    }
}
#[cfg(test)]
pub(super) fn sharpen(im: &mut Rendered, r: &Recipe) {
    sharpen_cancellable(im, r, &AtomicBool::new(false)).unwrap();
}
fn sharpen_cancellable(im: &mut Rendered, r: &Recipe, cancel: &AtomicBool) -> Result<()> {
    sharpen_with_radius(im, r, r.sharpening_radius, cancel)
}
/// Normalized Gaussian taps. Below half a pixel, which only scaled previews use, a
/// sampled Gaussian degenerates to a single tap; three taps with the same variance
/// keep the sharpening response of the full-resolution render.
fn gaussian(sigma: f32) -> (i32, Vec<f32>) {
    if sigma < 0.5 {
        let side = sigma * sigma / 2.;
        return (1, vec![side, 1. - 2. * side, side]);
    }
    let radius = (sigma * 3.).ceil() as i32;
    let weights: Vec<f32> = (-radius..=radius)
        .map(|x| (-0.5 * (x as f32 / sigma).powi(2)).exp())
        .collect();
    let sum: f32 = weights.iter().sum();
    (radius, weights.into_iter().map(|x| x / sum).collect())
}
fn sharpen_with_radius(
    im: &mut Rendered,
    r: &Recipe,
    sigma: f32,
    cancel: &AtomicBool,
) -> Result<()> {
    check_cancel(cancel)?;
    if r.sharpening == 0. {
        return Ok(());
    }
    let (radius, weights) = gaussian(sigma);
    let lum: Vec<f32> = im.pixels.par_iter().map(|p| luminance(*p)).collect();
    let w = im.width as usize;
    let h = im.height as usize;
    let mut horizontal = vec![0.; lum.len()];
    horizontal.par_iter_mut().enumerate().for_each(|(i, p)| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let x = i % w;
        let y = i / w;
        for (k, weight) in weights.iter().enumerate() {
            let xx = (x as i32 + k as i32 - radius).clamp(0, w as i32 - 1) as usize;
            *p += lum[y * w + xx] * weight;
        }
    });
    im.pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let x = i % w;
        let y = i / w;
        let mut blur = 0.;
        for (k, weight) in weights.iter().enumerate() {
            let yy = (y as i32 + k as i32 - radius).clamp(0, h as i32 - 1) as usize;
            blur += horizontal[yy * w + x] * weight;
        }
        let d = lum[i] - blur;
        // Edge mask suppresses sharpening of smooth areas; Detail admits finer texture.
        let threshold = r.sharpening_masking * 0.03 * (1. - r.sharpening_detail * 0.8);
        let mask = if threshold == 0. {
            1.
        } else {
            (d.abs() / threshold).clamp(0., 1.)
        };
        let delta = (d * r.sharpening * 2. * mask).clamp(-0.08, 0.08);
        // Add only luminance detail, preserving inter-channel differences.
        for v in p {
            *v = (*v + delta).clamp(0., 1.);
        }
    });
    check_cancel(cancel)
}
/// Neighborhood-ratio reconstruction in camera space, before color conversion.
/// Fully clipped neighborhoods have no recoverable color and use a neutral fallback.
pub fn recover_highlights(im: &CameraImage) -> CameraImage {
    recover_highlights_cancellable(im, &AtomicBool::new(false)).unwrap()
}
fn recover_highlights_cancellable(im: &CameraImage, cancel: &AtomicBool) -> Result<CameraImage> {
    check_cancel(cancel)?;
    let mut out = im.clone();
    out.pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let sensor =
            std::array::from_fn::<_, 3, _>(|c| im.pixels[i][c] / im.metadata.wb[c].max(0.001));
        let peak = sensor.into_iter().fold(0f32, f32::max);
        if peak < 0.97 {
            return;
        }
        let valid: Vec<usize> = (0..3).filter(|c| sensor[*c] < 0.97).collect();
        let x = i as i32 % im.width as i32;
        let y = i as i32 / im.width as i32;
        let mut estimate = [0.; 3];
        let mut weight_sum = 0.;
        if !valid.is_empty() {
            for dy in -3i32..=3 {
                for dx in -3i32..=3 {
                    let xx = (x + dx).clamp(0, im.width as i32 - 1);
                    let yy = (y + dy).clamp(0, im.height as i32 - 1);
                    let q = im.pixels[(yy as u32 * im.width + xx as u32) as usize];
                    if (0..3).any(|c| q[c] / im.metadata.wb[c].max(0.001) >= 0.97) {
                        continue;
                    }
                    let denom: f32 = valid.iter().map(|c| q[*c]).sum();
                    if denom < 1e-5 {
                        continue;
                    }
                    let target: f32 = valid.iter().map(|c| p[*c]).sum();
                    let gain = (target / denom).clamp(0.25, 4.);
                    let weight = 1. / (1. + (dx * dx + dy * dy) as f32);
                    weight_sum += weight;
                    for c in 0..3 {
                        estimate[c] += q[c] * gain * weight;
                    }
                }
            }
        }
        let neutral = valid.iter().map(|c| p[*c]).sum::<f32>() / valid.len().max(1) as f32;
        let neutral = if valid.is_empty() {
            p.iter().copied().fold(0f32, f32::max)
        } else {
            neutral
        };
        for c in 0..3 {
            let blend = ((sensor[c] - 0.97) / 0.03).clamp(0., 1.);
            let target = if weight_sum > 0. {
                estimate[c] / weight_sum
            } else {
                neutral
            };
            p[c] = p[c] * (1. - blend) + target * blend;
        }
    });
    check_cancel(cancel)?;
    Ok(out)
}
fn box_blur(
    src: &[f32],
    w: usize,
    h: usize,
    radius: usize,
    cancel: &AtomicBool,
) -> Result<Vec<f32>> {
    check_cancel(cancel)?;
    let mut tmp = vec![0.; src.len()];
    tmp.par_chunks_mut(w)
        .enumerate()
        .try_for_each(|(y, row)| -> Result<()> {
            check_cancel(cancel)?;
            let input = &src[y * w..(y + 1) * w];
            let mut prefix = vec![0.; w + 1];
            for x in 0..w {
                prefix[x + 1] = prefix[x] + input[x];
            }
            for (x, p) in row.iter_mut().enumerate() {
                let a = x.saturating_sub(radius);
                let b = (x + radius + 1).min(w);
                *p = (prefix[b] - prefix[a]) / (b - a) as f32;
            }
            Ok(())
        })?;
    // Independent columns preserve the reference accumulation order, while using
    // all CPU cores. Transposed output gives each task a disjoint contiguous slice.
    let mut columns = vec![0.; src.len()];
    columns
        .par_chunks_mut(h)
        .enumerate()
        .try_for_each(|(x, column)| -> Result<()> {
            check_cancel(cancel)?;
            let mut prefix = vec![0.; h + 1];
            for y in 0..h {
                prefix[y + 1] = prefix[y] + tmp[y * w + x];
            }
            for (y, p) in column.iter_mut().enumerate() {
                let a = y.saturating_sub(radius);
                let b = (y + radius + 1).min(h);
                *p = (prefix[b] - prefix[a]) / (b - a) as f32;
            }
            Ok(())
        })?;
    tmp.par_chunks_mut(w)
        .enumerate()
        .try_for_each(|(y, row)| -> Result<()> {
            check_cancel(cancel)?;
            for (x, p) in row.iter_mut().enumerate() {
                *p = columns[x * h + y];
            }
            Ok(())
        })?;
    Ok(tmp)
}
/// Log2 luminance before exposure and its box blurs. They depend only on white
/// balance, profile and lens vignetting, so Clarity, Texture and exposure edits reuse
/// them; exposure shifts every value by the same amount.
pub(crate) struct LocalBlurs {
    width: usize,
    logs: Vec<f32>,
    fine: Coarse,
    broad: Coarse,
    texture: Option<Vec<f32>>,
}
impl LocalBlurs {
    fn bytes(&self) -> usize {
        (self.logs.len() + self.texture.as_ref().map_or(0, Vec::len)) * 4
            + (self.fine.data.len() + self.broad.data.len()) * 4
    }
}
/// A box blur computed on a reduced copy of the image and read back bilinearly.
struct Coarse {
    width: usize,
    height: usize,
    factor: f32,
    data: Vec<f32>,
}
/// Blur radius, in pixels of the reduced copy, below which no further halving is done.
const COARSE_RADIUS: usize = 4;
impl Coarse {
    /// Box blur of `radius` pixels of `values`: halved (2×2 means) while the radius
    /// stays at least `COARSE_RADIUS` there, blurred, and upsampled when read. Small
    /// radii are blurred at full resolution.
    fn new(
        values: &[f32],
        (w, h): (usize, usize),
        radius: usize,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        let mut level = std::borrow::Cow::Borrowed(values);
        let (mut w, mut h, mut factor) = (w, h, 1usize);
        while radius / (factor * 2) >= COARSE_RADIUS && w > 1 && h > 1 {
            level = std::borrow::Cow::Owned(halve(&level, w, h));
            (w, h, factor) = (w.div_ceil(2), h.div_ceil(2), factor * 2);
        }
        let radius = ((radius as f32 / factor as f32).round() as usize).max(1);
        Ok(Self {
            width: w,
            height: h,
            factor: factor as f32,
            data: box_blur(&level, w, h, radius, cancel)?,
        })
    }
    fn at(&self, x: usize, y: usize) -> f32 {
        if self.factor == 1. {
            return self.data[y * self.width + x];
        }
        let fx = ((x as f32 + 0.5) / self.factor - 0.5).clamp(0., (self.width - 1) as f32);
        let fy = ((y as f32 + 0.5) / self.factor - 0.5).clamp(0., (self.height - 1) as f32);
        let (ix, iy) = (fx as usize, fy as usize);
        let (jx, jy) = ((ix + 1).min(self.width - 1), (iy + 1).min(self.height - 1));
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let v = |x: usize, y: usize| self.data[y * self.width + x];
        let top = v(ix, iy) * (1. - tx) + v(jx, iy) * tx;
        let bottom = v(ix, jy) * (1. - tx) + v(jx, jy) * tx;
        top * (1. - ty) + bottom * ty
    }
}
/// 2×2 means; an odd last row or column averages with itself.
fn halve(values: &[f32], w: usize, h: usize) -> Vec<f32> {
    let (hw, hh) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = vec![0.; hw * hh];
    out.par_chunks_mut(hw).enumerate().for_each(|(y, row)| {
        let (y0, y1) = (2 * y, (2 * y + 1).min(h - 1));
        for (x, v) in row.iter_mut().enumerate() {
            let (x0, x1) = (2 * x, (2 * x + 1).min(w - 1));
            *v = (values[y0 * w + x0]
                + values[y0 * w + x1]
                + values[y1 * w + x0]
                + values[y1 * w + x1])
                * 0.25;
        }
    });
    out
}
/// `scale` is the image's size relative to the full-resolution photo; radii given in
/// full-resolution pixels shrink with it.
fn local_blurs(
    im: &CameraImage,
    r: &Recipe,
    scale: f32,
    texture: bool,
    cancel: &AtomicBool,
) -> Result<LocalBlurs> {
    check_cancel(cancel)?;
    let matrix = develop::profile_matrix(&im.metadata, r);
    let vignetting = develop::pipeline::VignetteField::new(im, r);
    let logs: Vec<f32> = im
        .pixels
        .par_iter()
        .enumerate()
        .map(|(i, p)| {
            if cancel.load(Ordering::Relaxed) {
                return 0.;
            }
            let gain = vignetting.as_ref().map_or(1., |v| {
                v.gain(
                    (i % im.width as usize) as f32,
                    (i / im.width as usize) as f32,
                )
            });
            let p = p.map(|v| v * gain);
            let p = std::array::from_fn(|c| p[c] * r.wb[c]);
            let rgb = if let Some(profile) = &r.profile {
                profile.camera_color(p, matrix, r.temperature)
            } else {
                develop::mul(matrix, p)
            };
            luminance(rgb).max(1e-6).log2()
        })
        .collect();
    // Radii scale with the image (16 and 64 px on a 6000 px long edge), so previews
    // rendered from reduced images keep the same local contrast as full renders.
    let long = im.width.max(im.height) as f32;
    let radius = |px: f32| ((px / 6000. * long).round() as usize).max(1);
    let size = (im.width as usize, im.height as usize);
    let fine = Coarse::new(&logs, size, radius(16.), cancel)?;
    let broad = Coarse::new(&logs, size, radius(64.), cancel)?;
    let texture = if texture {
        let radius = ((3. * scale).round() as usize).max(1);
        Some(box_blur(&logs, size.0, size.1, radius, cancel)?)
    } else {
        None
    };
    check_cancel(cancel)?;
    Ok(LocalBlurs {
        width: size.0,
        logs,
        fine,
        broad,
        texture,
    })
}
/// Shadows, Highlights (before engine 4), Clarity and Texture as a per-pixel gain of
/// the camera image.
fn apply_local(b: &LocalBlurs, r: &Recipe, cancel: &AtomicBool) -> Result<Vec<f32>> {
    let exposure = r.exposure + r.camera_exposure;
    let mut gains = vec![0.; b.logs.len()];
    gains.par_iter_mut().enumerate().for_each(|(i, gain)| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let (x, y) = (i % b.width, i / b.width);
        let logs = b.logs[i] + exposure;
        // Range guidance limits halos at strong boundaries; details stay in the residual.
        let guide = |base: f32| {
            let d = base + exposure - logs;
            logs + d / (1. + d * d)
        };
        let fine = guide(b.fine.at(x, y));
        let base = (fine + guide(b.broad.at(x, y))) * 0.5;
        let y = 2f32.powf(base);
        let shadow = (-y * 6.).exp();
        let high = y / (y + 0.5);
        let clarity = (logs - fine).clamp(-1., 1.) * r.effects.clarity * 0.6;
        let texture = b.texture.as_ref().map_or(0., |t| {
            (b.logs[i] - t[i]).clamp(-0.5, 0.5) * r.effects.texture * 0.7
        });
        *gain = 2f32.powf(r.shadows * shadow * 2. + r.highlights * high * 2. + clarity + texture);
    });
    check_cancel(cancel)?;
    Ok(gains)
}
#[cfg(test)]
fn local_tones(
    im: &CameraImage,
    r: &Recipe,
    scale: f32,
    cancel: &AtomicBool,
) -> Result<CameraImage> {
    let blurs = local_blurs(im, r, scale, r.effects.texture != 0., cancel)?;
    let gains = apply_local(&blurs, r, cancel)?;
    let mut out = im.clone();
    for (p, g) in out.pixels.iter_mut().zip(gains) {
        *p = p.map(|v| v * g);
    }
    Ok(out)
}
/// The highlight-recovered image, computed once per decoded image.
pub(crate) fn recovered(im: &CameraImage, cancel: &AtomicBool) -> Result<Arc<CameraImage>> {
    if let Some(recovered) = im.recovered.get() {
        return Ok(recovered.clone());
    }
    let recovered = Arc::new(recover_highlights_cancellable(im, cancel)?);
    Ok(im.recovered.get_or_init(|| recovered).clone())
}
/// Clarity, Texture and, before engine 4, Shadows and Highlights, as a gain of the
/// camera image, plus the recipe for the per-pixel stage that follows.
fn local_stage(
    im: &Arc<CameraImage>,
    r: &Recipe,
    scale: f32,
    cancel: &AtomicBool,
    cache: Option<&mut StageCache>,
) -> Result<(Toned, Recipe)> {
    // Engine 4 renders Shadows and Highlights in the pixel pipeline (local_tone.rs);
    // this pre-pass then only carries Clarity and Texture.
    let measured = r.engine >= 4 && r.reference_curves;
    let mut spatial = r.clone();
    if measured {
        spatial.shadows = 0.;
        spatial.highlights = 0.;
    }
    let mut tonal = r.clone();
    if !measured {
        tonal.shadows = 0.;
        tonal.highlights = 0.;
    }
    let mut toned = Toned {
        image: im.clone(),
        gain: None,
    };
    if spatial.shadows == 0.
        && spatial.highlights == 0.
        && r.effects.clarity == 0.
        && r.effects.texture == 0.
    {
        return Ok((toned, tonal));
    }
    let texture = r.effects.texture != 0.;
    toned.gain = Some(match cache {
        None => {
            let blurs = local_blurs(im, &spatial, scale, texture, cancel)?;
            Arc::new(apply_local(&blurs, &spatial, cancel)?)
        }
        Some(cache) => {
            let key = BlurKey::new(im, &spatial, scale, texture);
            let blurs = cache.blurs.get_or_try(key, LocalBlurs::bytes, || {
                local_blurs(im, &spatial, scale, texture, cancel)
            })?;
            let key = LocalKey::new(&blurs, &spatial);
            let bytes = |gains: &Vec<f32>| gains.len() * 4;
            cache
                .local
                .get_or_try(key, bytes, || apply_local(&blurs, &spatial, cancel))?
        }
    });
    Ok((toned, tonal))
}
/// Fit and zoomed-out previews from a pyramid level (see `pyramid.rs`). Each output
/// pixel is developed once, from the level sampled over the pixel's footprint, and
/// radius-based effects are scaled to the output, so the result approximates the
/// full-resolution render resized to `size` at a fraction of the cost. `full` is the
/// full-resolution image the level was reduced from; `region` is a rectangle of the
/// `size` output.
pub(crate) fn render_level(
    level: &Arc<CameraImage>,
    full: &CameraImage,
    r: &Recipe,
    size: (u32, u32),
    region: [u32; 4],
    cancel: &AtomicBool,
    stages: &mut Stages,
) -> Result<Rendered> {
    check_cancel(cancel)?;
    r.validate()?;
    let effective = r.resolved(&level.metadata);
    let r = effective.as_ref();
    if let Some(p) = &r.profile {
        p.ensure_camera(&level.metadata)?;
    }
    let level_scale = level.width.max(level.height) as f32 / full.width.max(full.height) as f32;
    let (toned, tonal_recipe) = local_stage(level, r, level_scale, cancel, Some(stages.cache))?;
    let mut g = Geometry::new(level, r, 0);
    let footprint = g.width.max(g.height) as f32 / size.0.max(size.1) as f32;
    (g.width, g.height) = size;
    // Output pixels per full-resolution pixel.
    let scale = level_scale / footprint;
    let sigma = r.sharpening_radius * scale;
    let [x, y, w, h] = region;
    ensure!(
        w > 0
            && h > 0
            && x.checked_add(w).is_some_and(|v| v <= size.0)
            && y.checked_add(h).is_some_and(|v| v <= size.1),
        "Invalid viewport region"
    );
    let halo = if r.sharpening > 0. {
        gaussian(sigma).0 as u32
    } else {
        0
    };
    let (left, top) = (x.saturating_sub(halo), y.saturating_sub(halo));
    let right = (x + w + halo).min(size.0);
    let bottom = (y + h + halo).min(size.1);
    check_cancel(cancel)?;
    let mut out = develop::render_base(
        &toned,
        &tonal_recipe,
        &g,
        [left, top, right - left, bottom - top],
        develop::pipeline::footprint_spread(footprint),
        cancel,
        Some(stages),
    )?;
    sharpen_with_radius(&mut out, r, sigma, cancel)?;
    crate::develop::effects::spatial_finish_scaled(
        &mut out,
        r,
        [left, top],
        [size.0, size.1],
        scale,
    );
    check_cancel(cancel)?;
    Ok(crop(out, [x - left, y - top, w, h]))
}
fn crop(im: Rendered, [x, y, w, h]: [u32; 4]) -> Rendered {
    if (x, y, w, h) == (0, 0, im.width, im.height) {
        return im;
    }
    let pixels = (0..h)
        .flat_map(|row| {
            let a = ((y + row) * im.width + x) as usize;
            im.pixels[a..a + w as usize].iter().copied()
        })
        .collect();
    Rendered {
        width: w,
        height: h,
        pixels,
    }
}
pub fn render(
    im: &CameraImage,
    r: &Recipe,
    max_edge: u32,
    region: Option<[u32; 4]>,
) -> Result<Rendered> {
    render_cancellable(
        im,
        r,
        max_edge,
        region,
        &std::sync::atomic::AtomicBool::new(false),
    )
}
pub fn render_cancellable(
    im: &CameraImage,
    r: &Recipe,
    max_edge: u32,
    region: Option<[u32; 4]>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Rendered> {
    render_preview(im, r, max_edge, region, cancel, None)
}
pub(crate) fn render_preview(
    im: &CameraImage,
    r: &Recipe,
    max_edge: u32,
    region: Option<[u32; 4]>,
    cancel: &std::sync::atomic::AtomicBool,
    mut stages: Option<&mut Stages>,
) -> Result<Rendered> {
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Render superseded"
    );
    r.validate()?;
    let effective = r.resolved(&im.metadata);
    let r = effective.as_ref();
    if let Some(p) = &r.profile {
        p.ensure_camera(&im.metadata)?;
    }
    let source = recovered(im, cancel)?;
    let (toned, tonal_recipe) = local_stage(
        &source,
        r,
        1.,
        cancel,
        stages.as_mut().map(|s| &mut *s.cache),
    )?;
    let g = Geometry::new(&source, r, 0);
    let [x, y, w, h] = region.unwrap_or([0, 0, g.width, g.height]);
    ensure!(
        w > 0
            && h > 0
            && x.checked_add(w).is_some_and(|v| v <= g.width)
            && y.checked_add(h).is_some_and(|v| v <= g.height),
        "Invalid viewport region"
    );
    let halo = if r.sharpening > 0. {
        (3. * r.sharpening_radius).ceil() as u32
    } else {
        0
    };
    let left = x.saturating_sub(halo);
    let top = y.saturating_sub(halo);
    let right = (x + w + halo).min(g.width);
    let bottom = (y + h + halo).min(g.height);
    // Highlight recovery is prepared once per decoded image by the caller; see CameraImage.
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Render superseded"
    );
    let mut out = develop::render_base(
        &toned,
        &tonal_recipe,
        &g,
        [left, top, right - left, bottom - top],
        0.,
        cancel,
        stages.as_deref_mut(),
    )?;
    let spatial =
        r.effects.grain != 0. || r.effects.vignette != 0. || r.effects.lens_vignette != 0.;
    let mut gpu_sharpened = false;
    let edge = if region.is_some() { 0 } else { max_edge };
    if !spatial
        && let Some(finished) = stages
            .as_mut()
            .and_then(|s| s.backend.finish(&out, r, edge, cancel))
    {
        if region.is_none() {
            return Ok(finished);
        }
        out = finished;
        gpu_sharpened = true;
    }
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Render superseded"
    );
    if !gpu_sharpened {
        sharpen_cancellable(&mut out, r, cancel)?;
    }
    crate::develop::effects::spatial_finish(&mut out, r, [left, top], [g.width, g.height]);
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Render superseded"
    );
    if region.is_some() {
        Ok(crop(out, [x - left, y - top, w, h]))
    } else {
        // Spatial effects retain their CPU reference implementation. Resize can
        // still use compute after those effects, without sharpening twice.
        if spatial {
            let mut finished_recipe = r.clone();
            finished_recipe.sharpening = 0.;
            let finished = stages
                .as_mut()
                .and_then(|s| s.backend.finish(&out, &finished_recipe, max_edge, cancel));
            if let Some(finished) = finished {
                return Ok(finished);
            }
            ensure!(
                !cancel.load(std::sync::atomic::Ordering::Relaxed),
                "Render superseded"
            );
        }
        Ok(resize(out, max_edge))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parallel_box_blur_preserves_clipped_support_and_cancels() -> Result<()> {
        let (w, h) = (17usize, 9usize);
        let pixels: Vec<f32> = (0..w * h).map(|i| (i as f32 * 0.37).sin() * 3.).collect();
        let cancel = AtomicBool::new(false);
        for radius in [0, 1, 3, 64] {
            let actual = box_blur(&pixels, w, h, radius, &cancel)?;
            for y in 0..h {
                for x in 0..w {
                    let mut sum = 0.;
                    let mut count = 0;
                    for yy in y.saturating_sub(radius)..(y + radius + 1).min(h) {
                        for xx in x.saturating_sub(radius)..(x + radius + 1).min(w) {
                            sum += pixels[yy * w + xx];
                            count += 1;
                        }
                    }
                    assert!((actual[y * w + x] - sum / count as f32).abs() < 1e-5);
                }
            }
        }
        cancel.store(true, Ordering::Relaxed);
        assert!(box_blur(&pixels, w, h, 3, &cancel).is_err());
        let image = fixture();
        assert!(recover_highlights_cancellable(&image, &cancel).is_err());
        assert!(image.recovered.get().is_none());
        assert!(local_tones(&image, &Recipe::default(), 1., &cancel).is_err());
        Ok(())
    }
    fn fixture() -> CameraImage {
        CameraImage {
            width: 96,
            height: 80,
            pixels: (0..96 * 80)
                .map(|i| {
                    let y = (i % 96) as f32 / 192. + 0.05;
                    [y * 0.8, y, y * 0.6]
                })
                .collect(),
            metadata: crate::raw::Metadata {
                width: 96,
                height: 80,
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                wb: [1.; 3],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            recovered: Default::default(),
        }
    }
    #[test]
    fn preset_effects_match_tiles_and_survive_serialization() -> Result<()> {
        let im = fixture();
        let mut r = Recipe::default();
        r.effects.grain = 0.5;
        r.effects.vignette = -0.3;
        r.effects.clarity = 0.4;
        r.effects.texture = -0.2;
        r.effects.channels[0].insert([0.4, 0.5]);
        r.effects.calibration[2] = [0.2, -0.1];
        r.effects.parametric = [0.1, -0.2, 0.1, 0.];
        let saved = serde_json::to_vec(&r)?;
        let restored: Recipe = serde_json::from_slice(&saved)?;
        assert_eq!(r, restored);
        let full = render(&im, &r, 0, None)?;
        let tile = render(&im, &r, 0, Some([30, 25, 40, 40]))?;
        for y in 0..40 {
            for x in 0..40 {
                let a = tile.pixels[y * 40 + x];
                let b = full.pixels[(y + 25) * 96 + x + 30];
                for c in 0..3 {
                    assert!((a[c] - b[c]).abs() < 2e-6);
                }
            }
        }
        assert_eq!(full.pixels, render(&im, &r, 0, None)?.pixels);
        assert_ne!(
            full.pixels,
            render(&im, &Recipe::default(), 0, None)?.pixels
        );
        Ok(())
    }
    #[test]
    fn region_matches_full_with_large_radius_and_local_tones() -> Result<()> {
        let im = fixture();
        let r = Recipe {
            sharpening_radius: 3.,
            sharpening: 0.8,
            shadows: 0.5,
            highlights: -0.4,
            ..Default::default()
        };
        let full = render(&im, &r, 0, None)?;
        for [x, y, w, h] in [[0, 0, 20, 30], [30, 25, 40, 40], [80, 60, 16, 20]] {
            let tile = render(&im, &r, 0, Some([x, y, w, h]))?;
            for yy in 0..h {
                for xx in 0..w {
                    let a = tile.pixels[(yy * w + xx) as usize];
                    let b = full.pixels[((yy + y) * full.width + xx + x) as usize];
                    for c in 0..3 {
                        assert!((a[c] - b[c]).abs() < 2e-6);
                    }
                }
            }
        }
        Ok(())
    }
    #[test]
    fn final_fit_is_export_resized_after_detail() -> Result<()> {
        let im = fixture();
        let r = Recipe {
            sharpening: 0.8,
            ..Default::default()
        };
        let full = render(&im, &r, 0, None)?;
        let expected = resize(full, 48);
        let fit = render(&im, &r, 48, None)?;
        assert_eq!(fit.pixels, expected.pixels);
        Ok(())
    }
    #[test]
    fn partial_highlight_uses_neighbor_ratios_and_full_clip_is_neutral() {
        let mut im = fixture();
        im.pixels.fill([0.8, 0.4, 0.2]);
        let i = 40 * 96 + 40;
        im.pixels[i] = [1., 0.6, 0.3];
        let recovered = recover_highlights(&im);
        assert!((recovered.pixels[i][0] - 1.2).abs() < 1e-5);
        assert_eq!(recovered.pixels[i][1], 0.6);
        im.pixels.fill([1., 1., 1.]);
        assert!(recover_highlights(&im).pixels.iter().all(|p| *p == [1.; 3]));
    }
    #[test]
    fn cancellation_does_not_publish_partial_frame() {
        let im = fixture();
        assert!(
            render_cancellable(
                &im,
                &Recipe::default(),
                0,
                None,
                &std::sync::atomic::AtomicBool::new(true)
            )
            .is_err()
        );
    }
    #[test]
    fn sharpening_increases_edge_contrast_without_tint() {
        let mut im = Rendered {
            width: 32,
            height: 32,
            pixels: (0..1024)
                .map(|i| [if i % 32 < 16 { 0.3 } else { 0.6 }; 3])
                .collect(),
        };
        sharpen(&mut im, &Recipe::default());
        assert!(im.pixels[15][0] < 0.3);
        assert!(im.pixels[16][0] > 0.6);
        assert!(im.pixels.iter().all(|p| p[0] == p[1] && p[1] == p[2]));
    }
    #[test]
    fn physical_fit_size_has_no_fixed_ceiling() {
        assert_eq!(fit_edge(6000, 4000, [3000, 2000]), 3000);
        assert_eq!(fit_edge(6000, 4000, [1000, 2000]), 1000);
    }
    #[test]
    fn sharpening_preserves_flat_fields() {
        let mut im = Rendered {
            width: 20,
            height: 20,
            pixels: vec![[0.4, 0.3, 0.2]; 400],
        };
        let before = im.pixels.clone();
        sharpen(&mut im, &Recipe::default());
        for (a, b) in im.pixels.iter().zip(before) {
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 1e-6);
            }
        }
    }
}
