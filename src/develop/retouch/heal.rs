//! Rendering one Heal or Clone operation on linear camera pixels.
//!
//! Clone blends the source pixels in with the feathered shape. Heal copies the source,
//! then adds a smooth correction so the copy meets the destination at its border: the
//! difference between destination and source on a one-pixel ring outside the shape is
//! extended inward as a membrane (the solution of Laplace's equation). A constant or
//! linear difference is therefore reproduced exactly, and the source's texture is kept.
use super::{RetouchMode, RetouchOp, RetouchShape};
use crate::{develop::ImageFrame, raw::CameraImage};
use rayon::prelude::*;

/// A rectangle of decoded pixels, `[x0, y0, x1, y1)`.
pub(crate) type PixelRect = [i32; 4];

fn clip(r: PixelRect, w: u32, h: u32) -> PixelRect {
    [
        r[0].clamp(0, w as i32),
        r[1].clamp(0, h as i32),
        r[2].clamp(0, w as i32),
        r[3].clamp(0, h as i32),
    ]
}

/// The values the membrane is solved in. Only `Log` renders; the tests compare it
/// with the alternatives, which exist only there.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Domain {
    #[cfg(test)]
    Linear,
    /// ln(x + 0.001)
    Log,
    /// ln(1 + x)
    #[cfg(test)]
    Log1p,
}
impl Domain {
    fn to(self, v: f32) -> f32 {
        match self {
            #[cfg(test)]
            Domain::Linear => v,
            Domain::Log => (v.max(0.) + 1e-3).ln(),
            #[cfg(test)]
            Domain::Log1p => v.max(0.).ln_1p(),
        }
    }
    fn from(self, v: f32) -> f32 {
        match self {
            #[cfg(test)]
            Domain::Linear => v,
            Domain::Log => (v.exp() - 1e-3).max(0.),
            #[cfg(test)]
            Domain::Log1p => v.exp_m1().max(0.),
        }
    }
}
/// Heal's domain; see `retouch::tests` for the comparison on test data.
const DOMAIN: Domain = Domain::Log;

/// An operation placed on one image: its shape in decoded pixels.
pub(crate) struct Placed {
    pub(crate) mode: RetouchMode,
    pub(crate) opacity: f32,
    /// Path of dab centres (one point for a spot) and radius, in decoded pixels.
    pub(crate) points: Vec<[f32; 2]>,
    pub(crate) radius: f32,
    pub(crate) feather: f32,
    /// Source minus destination, in decoded pixels.
    pub(crate) offset: [f32; 2],
}
impl Placed {
    pub(crate) fn new(op: &RetouchOp, frame: &ImageFrame) -> Self {
        let points: Vec<[f32; 2]> = match &op.shape {
            RetouchShape::Spot { center, .. } => vec![frame.to_source(*center)],
            RetouchShape::Brush { points, .. } => {
                points.iter().map(|p| frame.to_source(*p)).collect()
            }
        };
        let anchor = op.pin();
        let a = frame.to_source(anchor);
        let b = frame.to_source([anchor[0] + op.offset[0], anchor[1] + op.offset[1]]);
        Self {
            mode: op.mode,
            opacity: op.opacity,
            points,
            radius: (op.radius() * frame.long_edge()).max(0.5),
            feather: op.feather,
            offset: [b[0] - a[0], b[1] - a[1]],
        }
    }
    /// Pixels the operation writes.
    pub(crate) fn dest(&self) -> PixelRect {
        let r = self.radius + 1.;
        let mut b = [i32::MAX, i32::MAX, i32::MIN, i32::MIN];
        for p in &self.points {
            b = [
                b[0].min((p[0] - r).floor() as i32),
                b[1].min((p[1] - r).floor() as i32),
                b[2].max((p[0] + r).ceil() as i32 + 1),
                b[3].max((p[1] + r).ceil() as i32 + 1),
            ];
        }
        b
    }
    /// Pixels the operation reads: its destination with the heal ring, and its source.
    pub(crate) fn reads(&self) -> [PixelRect; 2] {
        let d = self.dest();
        let d = [d[0] - 2, d[1] - 2, d[2] + 2, d[3] + 2];
        let (ox, oy) = (self.offset[0], self.offset[1]);
        let s = [
            d[0] + ox.floor() as i32 - 1,
            d[1] + oy.floor() as i32 - 1,
            d[2] + ox.ceil() as i32 + 1,
            d[3] + oy.ceil() as i32 + 1,
        ];
        [d, s]
    }
}
/// Feathered coverage of dabs of `radius` along `points` over `rect` of some pixel
/// grid (pixel centres at integer coordinates): 1 inside `1 - feather` of the
/// radius, falling smoothly to 0 at the radius.
pub(crate) fn coverage(
    points: &[[f32; 2]],
    radius: f32,
    feather: f32,
    rect: PixelRect,
) -> Vec<f32> {
    let (w, h) = ((rect[2] - rect[0]) as usize, (rect[3] - rect[1]) as usize);
    // Squared distance to the path, updated segment by segment within its bounds.
    let mut dist = vec![f32::INFINITY; w * h];
    let reach = radius + 1.;
    let segments: Vec<([f32; 2], [f32; 2])> = if points.len() == 1 {
        vec![(points[0], points[0])]
    } else {
        points.windows(2).map(|p| (p[0], p[1])).collect()
    };
    for (a, b) in segments {
        let x0 = ((a[0].min(b[0]) - reach).floor() as i32).max(rect[0]);
        let x1 = ((a[0].max(b[0]) + reach).ceil() as i32 + 1).min(rect[2]);
        let y0 = ((a[1].min(b[1]) - reach).floor() as i32).max(rect[1]);
        let y1 = ((a[1].max(b[1]) + reach).ceil() as i32 + 1).min(rect[3]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 - a[0], y as f32 - a[1]);
                let t = if len2 > 0. {
                    ((px * dx + py * dy) / len2).clamp(0., 1.)
                } else {
                    0.
                };
                let (ex, ey) = (px - t * dx, py - t * dy);
                let d = &mut dist[(y - rect[1]) as usize * w + (x - rect[0]) as usize];
                *d = d.min(ex * ex + ey * ey);
            }
        }
    }
    let inner = radius * (1. - feather);
    dist.into_iter()
        .map(|d2| profile(d2.sqrt(), inner, radius))
        .collect()
}
/// 1 up to `inner`, smoothstep down to 0 at `outer`.
pub(crate) fn profile(d: f32, inner: f32, outer: f32) -> f32 {
    if d <= inner {
        1.
    } else if d >= outer {
        0.
    } else {
        let t = (outer - d) / (outer - inner);
        t * t * (3. - 2. * t)
    }
}
fn bilinear(im: &CameraImage, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0., (im.width - 1) as f32);
    let y = y.clamp(0., (im.height - 1) as f32);
    let (ix, iy) = (x as u32, y as u32);
    let (jx, jy) = ((ix + 1).min(im.width - 1), (iy + 1).min(im.height - 1));
    let (fx, fy) = (x - ix as f32, y - iy as f32);
    let at = |x: u32, y: u32| im.pixels[(y * im.width + x) as usize];
    let (a, b, c, d) = (at(ix, iy), at(jx, iy), at(ix, jy), at(jx, jy));
    std::array::from_fn(|i| {
        (a[i] * (1. - fx) + b[i] * fx) * (1. - fy) + (c[i] * (1. - fx) + d[i] * fx) * fy
    })
}
/// Renders `op` into `im`; returns the rectangle it changed.
pub(crate) fn apply(im: &mut CameraImage, op: &Placed) -> PixelRect {
    let rect = clip(op.dest(), im.width, im.height);
    if rect[2] <= rect[0] || rect[3] <= rect[1] {
        return rect;
    }
    // One more pixel on each side is the heal ring.
    let grid = clip(
        [rect[0] - 1, rect[1] - 1, rect[2] + 1, rect[3] + 1],
        im.width,
        im.height,
    );
    let (w, h) = ((grid[2] - grid[0]) as usize, (grid[3] - grid[1]) as usize);
    let alpha = coverage(&op.points, op.radius, op.feather, grid);
    // Where the shape reaches the photo's edge there is no ring; the solver treats the
    // edge as mirrored.
    let at = |x: usize, y: usize| (grid[1] as usize + y) * im.width as usize + grid[0] as usize + x;
    let dest: Vec<[f32; 3]> = (0..w * h).map(|i| im.pixels[at(i % w, i / w)]).collect();
    let source: Vec<[f32; 3]> = (0..w * h)
        .map(|i| {
            let x = (grid[0] + (i % w) as i32) as f32 + op.offset[0];
            let y = (grid[1] + (i / w) as i32) as f32 + op.offset[1];
            bilinear(im, x, y)
        })
        .collect();
    let patch: Vec<[f32; 3]> = match op.mode {
        RetouchMode::Clone => source,
        RetouchMode::Heal => heal(&dest, &source, &alpha, w, h, DOMAIN),
    };
    for i in 0..w * h {
        let a = alpha[i] * op.opacity;
        if a > 0. {
            let d = dest[i];
            im.pixels[at(i % w, i / w)] = std::array::from_fn(|c| d[c] + (patch[i][c] - d[c]) * a);
        }
    }
    rect
}
/// Source plus the membrane of `dest - source` over `alpha > 0`, in `domain`.
pub(crate) fn heal(
    dest: &[[f32; 3]],
    source: &[[f32; 3]],
    alpha: &[f32],
    w: usize,
    h: usize,
    domain: Domain,
) -> Vec<[f32; 3]> {
    let (to, from) = (|v| domain.to(v), |v| domain.from(v));
    let inside: Vec<bool> = alpha.iter().map(|a| *a > 0.).collect();
    let channels: Vec<Vec<f32>> = (0..3)
        .into_par_iter()
        .map(|c| {
            let diff: Vec<f32> = dest
                .iter()
                .zip(source)
                .map(|(d, s)| to(d[c]) - to(s[c]))
                .collect();
            membrane(&diff, &inside, w, h)
        })
        .collect();
    (0..w * h)
        .map(|i| std::array::from_fn(|c| from(to(source[i][c]) + channels[c][i])))
        .collect()
}
/// Solves Laplace's equation for `values` over `inside`, keeping the values outside it
/// as the boundary: multigrid V-cycles on the shape's grid, so large brushed areas
/// converge as quickly as small spots.
pub(crate) fn membrane(values: &[f32], inside: &[bool], w: usize, h: usize) -> Vec<f32> {
    let mut v = values.to_vec();
    let start = boundary_mean(values, inside, w, h);
    for (v, i) in v.iter_mut().zip(inside) {
        if *i {
            *v = start;
        }
    }
    let rhs = vec![0.; w * h];
    for _ in 0..CYCLES {
        v_cycle(&mut v, &rhs, inside, w, h);
    }
    v
}
const CYCLES: usize = 8;
/// Relaxes `A v = rhs` (A: 4v minus the four neighbours) on `inside`, then solves for
/// the error on a grid half the size and corrects.
fn v_cycle(v: &mut [f32], rhs: &[f32], inside: &[bool], w: usize, h: usize) {
    if w <= 12 && h <= 12 {
        sweeps(v, rhs, inside, w, h, 60);
        return;
    }
    sweeps(v, rhs, inside, w, h, 3);
    let mut residual = vec![0.; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if inside[i] {
                let n = neighbours(v, w, h, x, y);
                residual[i] = rhs[i] - (4. * v[i] - n);
            }
        }
    }
    // Coarse cells are inside when any child is; the coarse equation has twice the
    // spacing, so its right-hand side is four times the children's mean residual.
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut coarse_rhs = vec![0.; cw * ch];
    let mut coarse_inside = vec![true; cw * ch];
    for y in 0..h {
        for x in 0..w {
            let c = (y / 2) * cw + x / 2;
            coarse_rhs[c] += residual[y * w + x];
            coarse_inside[c] &= inside[y * w + x];
        }
    }
    let mut error = vec![0.; cw * ch];
    v_cycle(&mut error, &coarse_rhs, &coarse_inside, cw, ch);
    for y in 0..h {
        for x in 0..w {
            if inside[y * w + x] {
                let fx = ((x as f32 - 0.5) / 2.).clamp(0., (cw - 1) as f32);
                let fy = ((y as f32 - 0.5) / 2.).clamp(0., (ch - 1) as f32);
                let (ix, iy) = (fx as usize, fy as usize);
                let (jx, jy) = ((ix + 1).min(cw - 1), (iy + 1).min(ch - 1));
                let (tx, ty) = (fx - ix as f32, fy - iy as f32);
                let e = |x: usize, y: usize| error[y * cw + x];
                v[y * w + x] += (e(ix, iy) * (1. - tx) + e(jx, iy) * tx) * (1. - ty)
                    + (e(ix, jy) * (1. - tx) + e(jx, jy) * tx) * ty;
            }
        }
    }
    sweeps(v, rhs, inside, w, h, 3);
}
/// The sum of the four neighbours; beyond the grid they mirror inward.
fn neighbours(v: &[f32], w: usize, h: usize, x: usize, y: usize) -> f32 {
    let left = if x > 0 { x - 1 } else { (x + 1).min(w - 1) };
    let right = if x + 1 < w {
        x + 1
    } else {
        x.saturating_sub(1)
    };
    let up = if y > 0 { y - 1 } else { (y + 1).min(h - 1) };
    let down = if y + 1 < h {
        y + 1
    } else {
        y.saturating_sub(1)
    };
    v[y * w + left] + v[y * w + right] + v[up * w + x] + v[down * w + x]
}
fn boundary_mean(values: &[f32], inside: &[bool], w: usize, h: usize) -> f32 {
    let (mut sum, mut n) = (0., 0.);
    for y in 0..h {
        for x in 0..w {
            if !inside[y * w + x] {
                sum += values[y * w + x];
                n += 1.;
            }
        }
    }
    if n > 0. { sum / n } else { 0. }
}
/// Red-black Gauss-Seidel with over-relaxation.
fn sweeps(v: &mut [f32], rhs: &[f32], inside: &[bool], w: usize, h: usize, count: usize) {
    const OMEGA: f32 = 1.15;
    for _ in 0..count {
        for parity in 0..2 {
            for y in 0..h {
                for x in ((y + parity) % 2..w).step_by(2) {
                    let i = y * w + x;
                    if inside[i] {
                        let target = (neighbours(v, w, h, x, y) + rhs[i]) * 0.25;
                        v[i] += OMEGA * (target - v[i]);
                    }
                }
            }
        }
    }
}
