//! Text for watermarks, drawn from glyph outlines at the export's own size
//! with coverage kept in f32: an accumulation rasterizer (signed area per
//! pixel, summed along each row), and a drop shadow as the offset, blurred
//! coverage.
use super::{Align, Shadow, fonts::Font};
use anyhow::Result;
use skrifa::{
    MetadataProvider,
    instance::{LocationRef, Size},
    outline::{DrawSettings, OutlinePen},
};

/// Line height in ems where the font gives none usable.
const LINE: f32 = 1.2;

pub(super) struct Text {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<[f32; 4]>,
}

struct Layout {
    /// Each glyph: id and pen position (x, baseline y), top-down pixels.
    glyphs: Vec<(skrifa::GlyphId, f32, f32)>,
    width: f32,
    height: f32,
}

fn layout(font: &Font, text: &str, px: f32, align: Align) -> Result<Layout> {
    let face = font.face()?;
    let location = location(font, &face);
    let size = Size::new(px);
    let metrics = face.metrics(size, &location);
    let advances = face.glyph_metrics(size, &location);
    let charmap = face.charmap();
    let ascent = metrics.ascent;
    let line = {
        let natural = metrics.ascent - metrics.descent + metrics.leading;
        if natural > 0. { natural } else { px * LINE }
    };
    let lines: Vec<Vec<(skrifa::GlyphId, f32)>> = text
        .lines()
        .map(|l| {
            l.chars()
                .filter_map(|c| {
                    // A character the font lacks is left out rather than drawn
                    // as a box.
                    let gid = charmap.map(c)?;
                    Some((gid, advances.advance_width(gid).unwrap_or(0.)))
                })
                .collect()
        })
        .collect();
    let widths: Vec<f32> = lines
        .iter()
        .map(|l| l.iter().map(|(_, a)| a).sum())
        .collect();
    let width = widths.iter().copied().fold(0., f32::max);
    let mut glyphs = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let mut x = match align {
            Align::Left => 0.,
            Align::Center => (width - widths[i]) / 2.,
            Align::Right => width - widths[i],
        };
        let baseline = ascent + i as f32 * line;
        for (gid, advance) in l {
            glyphs.push((*gid, x, baseline));
            x += advance;
        }
    }
    let height = if lines.is_empty() {
        0.
    } else {
        ascent - metrics.descent + (lines.len() - 1) as f32 * line
    };
    Ok(Layout {
        glyphs,
        width,
        height,
    })
}

fn location<'a>(font: &Font, face: &skrifa::FontRef<'a>) -> skrifa::instance::Location {
    match font.weight {
        Some(weight) => face.axes().location([("wght", weight)]),
        None => face.axes().location(std::iter::empty::<(&str, f32)>()),
    }
}

/// The size of `text`'s block at `px`.
/// The size of `text`'s ink at `px`, as `text` draws it without a shadow.
pub(super) fn measure(font: &Font, text: &str, px: f32) -> Option<(f32, f32)> {
    let drawn = self::text(font, text, px, Align::Left, [1.; 3], &Shadow::default())?;
    let inked = drawn.rgba.iter().any(|p| p[3] > 0.);
    inked.then_some((drawn.width as f32, drawn.height as f32))
}

/// `text` in `color` at `px`, with its shadow; the block's own size plus
/// room for the shadow.
pub(super) fn text(
    font: &Font,
    text: &str,
    px: f32,
    align: Align,
    color: [f32; 3],
    shadow: &Shadow,
) -> Option<Text> {
    let l = layout(font, text, px, align).ok()?;
    let face = font.face().ok()?;
    let location = location(font, &face);
    let outlines = face.outline_glyphs();
    // Room for the shadow on every side it may reach.
    let (dx, dy, blur) = if shadow.enabled {
        let angle = shadow.angle.to_radians();
        (
            shadow.offset * px * angle.cos(),
            -shadow.offset * px * angle.sin(),
            shadow.radius * px,
        )
    } else {
        (0., 0., 0.)
    };
    // Room for outlines that reach past their advances (italics, swashes)
    // and for the shadow; trimmed to the ink afterwards.
    let pad = (0.25 * px + dx.abs().max(dy.abs()) + 3. * blur).ceil() + 2.;
    let (w, h) = (
        (l.width + 2. * pad).ceil() as usize,
        (l.height + 2. * pad).ceil() as usize,
    );
    let mut raster = Raster::new(w, h);
    for (gid, x, baseline) in &l.glyphs {
        let Some(glyph) = outlines.get(*gid) else {
            continue;
        };
        let mut pen = Pen {
            raster: &mut raster,
            origin: (x + pad, baseline + pad),
            start: (0., 0.),
            at: (0., 0.),
        };
        let settings = DrawSettings::unhinted(Size::new(px), LocationRef::from(&location));
        let _ = glyph.draw(settings, &mut pen);
        pen.close();
    }
    let coverage = raster.coverage();
    let shadow_alpha = shadow
        .enabled
        .then(|| shadow_of(&coverage, w, h, dx, dy, blur));
    let rgba = coverage
        .iter()
        .enumerate()
        .map(|(i, &a)| {
            let s = shadow_alpha
                .as_ref()
                .map_or(0., |s| s[i] * shadow.opacity.clamp(0., 1.));
            // The text over its black shadow, as one straight-alpha color.
            let alpha = a + s * (1. - a);
            if alpha <= 0. {
                return [0.; 4];
            }
            let mix = |c: f32| c * a / alpha;
            [mix(color[0]), mix(color[1]), mix(color[2]), alpha]
        })
        .collect();
    Some(trimmed(rgba, w, h))
}

/// The mark cut down to its ink, with a pixel to spare.
fn trimmed(rgba: Vec<[f32; 4]>, w: usize, h: usize) -> Text {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if rgba[y * w + x][3] > 0. {
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
    }
    if x0 > x1 {
        return Text {
            width: w,
            height: h,
            rgba,
        };
    }
    let (x0, y0) = (x0.saturating_sub(1), y0.saturating_sub(1));
    let (x1, y1) = ((x1 + 1).min(w - 1), (y1 + 1).min(h - 1));
    let (cw, ch) = (x1 - x0 + 1, y1 - y0 + 1);
    let mut out = Vec::with_capacity(cw * ch);
    for y in y0..=y1 {
        out.extend_from_slice(&rgba[y * w + x0..=y * w + x1]);
    }
    Text {
        width: cw,
        height: ch,
        rgba: out,
    }
}

/// The shadow's coverage: shifted and blurred, at a reduced size when the
/// blur is wide, so a large shadow costs what a small one does.
fn shadow_of(coverage: &[f32], w: usize, h: usize, dx: f32, dy: f32, blur: f32) -> Vec<f32> {
    let k = ((blur / 8.).ceil() as usize).max(1);
    if k == 1 {
        return blurred(&shift(coverage, w, h, dx, dy), w, h, blur);
    }
    let (sw, sh) = (w.div_ceil(k), h.div_ceil(k));
    let mut small = vec![0.; sw * sh];
    for y in 0..h {
        for x in 0..w {
            small[(y / k) * sw + x / k] += coverage[y * w + x];
        }
    }
    let area = (k * k) as f32;
    for v in &mut small {
        *v /= area;
    }
    let k_f = k as f32;
    let small = blurred(
        &shift(&small, sw, sh, dx / k_f, dy / k_f),
        sw,
        sh,
        blur / k_f,
    );
    // Back to full size, bilinearly.
    let at = |x: f32, y: f32| -> f32 {
        let (x, y) = (x.clamp(0., (sw - 1) as f32), y.clamp(0., (sh - 1) as f32));
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        let row = |yy: usize| small[yy * sw + x0] * (1. - tx) + small[yy * sw + x1] * tx;
        row(y0) * (1. - ty) + row(y1) * ty
    };
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            out.push(at(
                (x as f32 + 0.5) / k_f - 0.5,
                (y as f32 + 0.5) / k_f - 0.5,
            ));
        }
    }
    out
}

/// Moves coverage by a fraction of pixels, bilinearly.
fn shift(src: &[f32], w: usize, h: usize, dx: f32, dy: f32) -> Vec<f32> {
    let at = |x: i64, y: i64| -> f32 {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            0.
        } else {
            src[y as usize * w + x as usize]
        }
    };
    let (fx, fy) = (dx.floor(), dy.floor());
    let (tx, ty) = (dx - fx, dy - fy);
    let mut out = vec![0.; w * h];
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let (sx, sy) = (x - fx as i64, y - fy as i64);
            let v = at(sx, sy) * (1. - tx) * (1. - ty)
                + at(sx - 1, sy) * tx * (1. - ty)
                + at(sx, sy - 1) * (1. - tx) * ty
                + at(sx - 1, sy - 1) * tx * ty;
            out[y as usize * w + x as usize] = v;
        }
    }
    out
}

/// A Gaussian blur with `radius` as its standard deviation, separably.
fn blurred(src: &[f32], w: usize, h: usize, radius: f32) -> Vec<f32> {
    if radius < 0.3 {
        return src.to_vec();
    }
    let reach = (3. * radius).ceil() as i64;
    let kernel: Vec<f32> = (-reach..=reach)
        .map(|i| (-(i * i) as f32 / (2. * radius * radius)).exp())
        .collect();
    let total: f32 = kernel.iter().sum();
    let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0.; w * h];
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let mut sum = 0.;
                for (k, weight) in kernel.iter().enumerate() {
                    let d = k as i64 - reach;
                    let (sx, sy) = if horizontal { (x + d, y) } else { (x, y + d) };
                    if sx >= 0 && sy >= 0 && sx < w as i64 && sy < h as i64 {
                        sum += weight * src[sy as usize * w + sx as usize];
                    }
                }
                out[y as usize * w + x as usize] = sum / total;
            }
        }
        out
    };
    pass(&pass(src, true), false)
}

/// Turns outlines into line segments in pixel space (y down).
struct Pen<'r> {
    raster: &'r mut Raster,
    origin: (f32, f32),
    start: (f32, f32),
    at: (f32, f32),
}
impl Pen<'_> {
    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        (self.origin.0 + x, self.origin.1 - y)
    }
}
impl OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = self.point(x, y);
        self.at = self.start;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let to = self.point(x, y);
        self.raster.line(self.at, to);
        self.at = to;
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (c, to) = (self.point(cx0, cy0), self.point(x, y));
        let from = self.at;
        let steps = segments(from, c, c, to);
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let u = 1. - t;
            let p = (
                u * u * from.0 + 2. * u * t * c.0 + t * t * to.0,
                u * u * from.1 + 2. * u * t * c.1 + t * t * to.1,
            );
            self.raster.line(self.at, p);
            self.at = p;
        }
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0, c1, to) = (self.point(cx0, cy0), self.point(cx1, cy1), self.point(x, y));
        let from = self.at;
        let steps = segments(from, c0, c1, to);
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let u = 1. - t;
            let p = (
                u * u * u * from.0
                    + 3. * u * u * t * c0.0
                    + 3. * u * t * t * c1.0
                    + t * t * t * to.0,
                u * u * u * from.1
                    + 3. * u * u * t * c0.1
                    + 3. * u * t * t * c1.1
                    + t * t * t * to.1,
            );
            self.raster.line(self.at, p);
            self.at = p;
        }
    }
    fn close(&mut self) {
        if self.at != self.start {
            self.raster.line(self.at, self.start);
            self.at = self.start;
        }
    }
}
/// Enough line segments that a curve is off by well under a pixel.
fn segments(a: (f32, f32), b: (f32, f32), c: (f32, f32), d: (f32, f32)) -> usize {
    let len = |p: (f32, f32), q: (f32, f32)| ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt();
    let hull = len(a, b) + len(b, c) + len(c, d);
    ((hull / 2.).sqrt().ceil() as usize).clamp(1, 256)
}

/// Signed area per pixel; a row's running sum is its coverage.
struct Raster {
    w: usize,
    h: usize,
    acc: Vec<f32>,
}
impl Raster {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            acc: vec![0.; w * h + 4],
        }
    }
    fn line(&mut self, p0: (f32, f32), p1: (f32, f32)) {
        if (p0.1 - p1.1).abs() <= f32::EPSILON {
            return;
        }
        let (dir, p0, p1) = if p0.1 < p1.1 {
            (1., p0, p1)
        } else {
            (-1., p1, p0)
        };
        let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
        let mut x = p0.0;
        if p0.1 < 0. {
            x -= p0.1 * dxdy;
        }
        let max_x = (self.w - 1) as f32;
        let y_end = (p1.1.ceil().max(0.) as usize).min(self.h);
        for y in (p0.1.max(0.) as usize)..y_end {
            let row = y * self.w;
            let dy = ((y + 1) as f32).min(p1.1) - (y as f32).max(p0.1);
            let x_next = x + dxdy * dy;
            let d = dy * dir;
            let (x0, x1) = if x < x_next { (x, x_next) } else { (x_next, x) };
            let (x0, x1) = (x0.clamp(0., max_x), x1.clamp(0., max_x));
            let x0_floor = x0.floor();
            let x0i = x0_floor as usize;
            let x1_ceil = x1.ceil();
            let x1i = x1_ceil as usize;
            if x1i <= x0i + 1 {
                let xmf = 0.5 * (x0 + x1) - x0_floor;
                self.acc[row + x0i] += d - d * xmf;
                self.acc[row + x0i + 1] += d * xmf;
            } else {
                let s = (x1 - x0).recip();
                let x0f = x0 - x0_floor;
                let a0 = 0.5 * s * (1. - x0f) * (1. - x0f);
                let x1f = x1 - x1_ceil + 1.;
                let am = 0.5 * s * x1f * x1f;
                self.acc[row + x0i] += d * a0;
                if x1i == x0i + 2 {
                    self.acc[row + x0i + 1] += d * (1. - a0 - am);
                } else {
                    let a1 = s * (1.5 - x0f);
                    self.acc[row + x0i + 1] += d * (a1 - a0);
                    for xi in x0i + 2..x1i - 1 {
                        self.acc[row + xi] += d * s;
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    self.acc[row + x1i - 1] += d * (1. - a2 - am);
                }
                self.acc[row + x1i] += d * am;
            }
            x = x_next;
        }
    }
    fn coverage(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.w * self.h);
        let mut sum = 0.;
        for (i, a) in self.acc[..self.w * self.h].iter().enumerate() {
            if i % self.w == 0 {
                // Each row starts outside every outline.
                sum = 0.;
            }
            sum += a;
            out.push(sum.abs().min(1.));
        }
        out
    }
}
