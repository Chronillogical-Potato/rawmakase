//! Finding the red pupil inside the circle the user drags over an eye, as Lightroom's
//! tool does before it places the correction.
//!
//! Redness is the log ratio of red to the larger of green and blue. The pupil is the
//! connected area around the reddest point near the centre whose redness is more
//! than half-way from the surrounding face's to the pupil's, with any catchlight
//! inside filled in. Its ellipse has the area's centre, and semi-axes and correlation
//! from its second moments (a filled ellipse has semi-axes of twice the standard
//! deviation).
use super::MAX_CORRELATION;
use crate::{develop::ImageFrame, raw::CameraImage};

/// Why no pupil was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectError {
    /// Nothing red enough near the centre of the circle.
    NotRed,
    /// The red area has no edge within the circle: the circle is too small, or the
    /// area isn't a pupil.
    NoEdge,
}
impl std::fmt::Display for DetectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Lightroom's warning.
        f.write_str("Unable to find red eye. Be sure to use an area that includes the entire eye.")
    }
}

/// A pupil's ellipse in image space (see [`super::RedEyeOp`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pupil {
    pub center: [f32; 2],
    pub radius: [f32; 2],
    pub correlation: f32,
}

/// Red must be at least this many times the larger of green and blue at the pupil (in
/// linear values a brown iris reaches about 2.5, a red pupil 10 or more).
const MIN_RATIO: f32 = 3.5;
/// Where the pupil's edge lies between the surroundings' redness and the pupil's.
const EDGE: f32 = 0.5;

/// The red pupil within `radius` (a fraction of the long edge) of image-space
/// position `center` in `im`.
pub fn find_pupil(im: &CameraImage, center: [f32; 2], radius: f32) -> Result<Pupil, DetectError> {
    let frame = ImageFrame::new(im);
    let [cx, cy] = frame.to_source(center);
    let r = (radius * frame.long_edge()).max(3.);
    let (w, h) = (im.width as i32, im.height as i32);
    let x0 = ((cx - r).floor() as i32).clamp(0, w);
    let y0 = ((cy - r).floor() as i32).clamp(0, h);
    let x1 = ((cx + r).ceil() as i32 + 1).clamp(0, w);
    let y1 = ((cy + r).ceil() as i32 + 1).clamp(0, h);
    let (gw, gh) = ((x1 - x0) as usize, (y1 - y0) as usize);
    if gw < 3 || gh < 3 {
        return Err(DetectError::NotRed);
    }
    let at =
        |x: usize, y: usize| im.pixels[(y0 as usize + y) * im.width as usize + x0 as usize + x];
    let distance =
        |x: usize, y: usize| ((x0 as f32 + x as f32 - cx).hypot(y0 as f32 + y as f32 - cy)) / r;
    // A small floor keeps noise in the darkest pixels from looking red.
    let floor = 2e-3;
    let redness: Vec<f32> = (0..gw * gh)
        .map(|i| {
            let p = at(i % gw, i / gw);
            ((p[0].max(0.) + floor) / (p[1].max(p[2]).max(0.) + floor)).ln()
        })
        .collect();
    let smooth = |x: usize, y: usize| {
        let (mut sum, mut n) = (0., 0.);
        for yy in y.saturating_sub(1)..(y + 2).min(gh) {
            for xx in x.saturating_sub(1)..(x + 2).min(gw) {
                sum += redness[yy * gw + xx];
                n += 1.;
            }
        }
        sum / n
    };
    // The reddest point near the centre, and the redness of the circle's rim.
    let mut seed = None::<(usize, usize, f32)>;
    let mut rim = Vec::new();
    for y in 0..gh {
        for x in 0..gw {
            let d = distance(x, y);
            if d <= 0.6 {
                let v = smooth(x, y);
                if seed.is_none_or(|s| v > s.2) {
                    seed = Some((x, y, v));
                }
            } else if (0.85..=1.).contains(&d) {
                rim.push(redness[y * gw + x]);
            }
        }
    }
    let (sx, sy, peak) = seed.ok_or(DetectError::NotRed)?;
    if peak < MIN_RATIO.ln() || rim.is_empty() {
        return Err(DetectError::NotRed);
    }
    rim.sort_by(f32::total_cmp);
    let around = rim[rim.len() / 2];
    let threshold = around + EDGE * (peak - around);
    // The connected area at least that red, within the circle.
    let inside = |x: usize, y: usize| distance(x, y) <= 1.;
    let mut area = vec![false; gw * gh];
    let mut stack = vec![(sx, sy)];
    area[sy * gw + sx] = true;
    while let Some((x, y)) = stack.pop() {
        for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            if nx < 0 || ny < 0 || nx >= gw as i32 || ny >= gh as i32 {
                continue;
            }
            let (nx, ny) = (nx as usize, ny as usize);
            let i = ny * gw + nx;
            if !area[i] && inside(nx, ny) && redness[i] >= threshold {
                area[i] = true;
                stack.push((nx, ny));
            }
        }
    }
    // An area reaching the circle's rim all round is not a pupil.
    let on_rim = (0..gw * gh)
        .filter(|i| area[*i] && distance(i % gw, i / gw) > 0.9)
        .count();
    let rim_pixels = (0..gw * gh)
        .filter(|i| (0.9..=1.).contains(&distance(i % gw, i / gw)))
        .count();
    if on_rim * 2 > rim_pixels {
        return Err(DetectError::NoEdge);
    }
    fill_holes(&mut area, gw, gh);
    // Second moments of the filled area.
    let (mut n, mut mx, mut my) = (0f64, 0f64, 0f64);
    for i in (0..gw * gh).filter(|i| area[*i]) {
        n += 1.;
        mx += (i % gw) as f64;
        my += (i / gw) as f64;
    }
    if n < 3. {
        return Err(DetectError::NotRed);
    }
    let (mx, my) = (mx / n, my / n);
    let (mut vxx, mut vyy, mut vxy) = (0f64, 0f64, 0f64);
    for i in (0..gw * gh).filter(|i| area[*i]) {
        let (dx, dy) = ((i % gw) as f64 - mx, (i / gw) as f64 - my);
        vxx += dx * dx;
        vyy += dy * dy;
        vxy += dx * dy;
    }
    // A pixel's own extent (1/12) keeps tiny areas from collapsing to a line.
    let (vxx, vyy, vxy) = (vxx / n + 1. / 12., vyy / n + 1. / 12., vxy / n);
    let source_radius = [2. * vxx.sqrt() as f32, 2. * vyy.sqrt() as f32];
    let correlation = ((vxy / (vxx * vyy).sqrt()) as f32).clamp(-MAX_CORRELATION, MAX_CORRELATION);
    let long = frame.long_edge();
    let center = frame.to_image(x0 as f32 + mx as f32, y0 as f32 + my as f32);
    // Back from decoded axes to image axes: a quarter turn swaps them and mirrors the tilt.
    let (radius, correlation) = if frame.turns % 2 == 1 {
        (
            [source_radius[1] / long, source_radius[0] / long],
            -correlation,
        )
    } else {
        (
            [source_radius[0] / long, source_radius[1] / long],
            correlation,
        )
    };
    Ok(Pupil {
        center,
        radius,
        correlation,
    })
}

/// Marks pixels enclosed by `area` (not reachable from the grid's border without
/// crossing it) as part of it: a catchlight inside a pupil.
fn fill_holes(area: &mut [bool], w: usize, h: usize) {
    let mut outside = vec![false; w * h];
    let mut stack: Vec<usize> = (0..w * h)
        .filter(|i| {
            let (x, y) = (i % w, i / w);
            (x == 0 || y == 0 || x == w - 1 || y == h - 1) && !area[*i]
        })
        .collect();
    for i in &stack {
        outside[*i] = true;
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        let mut visit = |j: usize| {
            if !area[j] && !outside[j] {
                outside[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            visit(i - 1);
        }
        if x + 1 < w {
            visit(i + 1);
        }
        if y > 0 {
            visit(i - w);
        }
        if y + 1 < h {
            visit(i + w);
        }
    }
    for (a, o) in area.iter_mut().zip(outside) {
        *a = !o;
    }
}
