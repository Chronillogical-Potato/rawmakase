//! Lightroom's Upright analysis: straight lines in the photo give its vertical and
//! horizontal vanishing points, from which Level, Vertical, Full and Auto are camera
//! rotations at the photo's focal length (docs/transform.md).
use super::{Geometry, Recipe, image_space::LensMap};
use crate::raw::CameraImage;

/// The lens settings an analysis is made through: when any of them changes, the
/// corrections analysed before no longer fit the photo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LensInputs {
    builtin: bool,
    profile: bool,
    distortion: f32,
    manual_distortion: f32,
    panel: super::panels::PanelState,
    /// Lens corrections render from process version 4.
    current_engine: bool,
}
impl LensInputs {
    pub fn of(r: &Recipe) -> Self {
        Self {
            builtin: r.lens_builtin,
            profile: r.lens_profile,
            distortion: r.lens_distortion,
            manual_distortion: r.lens_manual_distortion,
            panel: r.panels.state(super::panels::Panel::LensCorrections),
            current_engine: r.engine >= 4,
        }
    }
}

/// Long edge of the image the lines are found in.
const ANALYSIS_EDGE: u32 = 1024;

/// A straight edge, its ends in centred coordinates of the displayed photo, y down, in
/// units of its long edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Segment {
    pub a: [f32; 2],
    pub b: [f32; 2],
}
impl Segment {
    pub fn length(&self) -> f32 {
        (self.b[0] - self.a[0]).hypot(self.b[1] - self.a[1])
    }
}

/// The photo's luminance as displayed (orientation and lens correction applied, no crop,
/// straighten or Transform), gamma-encoded to 0–255, and its width and height.
pub fn analysis_image(im: &CameraImage, r: &Recipe) -> (Vec<f32>, usize, usize) {
    let mut a = r.clone();
    a.crop = [0., 0., 1., 1.];
    a.straighten = 0.;
    a.transform = Default::default();
    a.upright = Default::default();
    let g = Geometry::new(im, &a, ANALYSIS_EDGE);
    let lens = LensMap::new(im, &a);
    let (w, h) = (g.width as usize, g.height as usize);
    let (iw, ih) = (im.width as usize, im.height as usize);
    let at = |x: usize, y: usize| {
        let p = im.pixels[y.min(ih - 1) * iw + x.min(iw - 1)];
        p[0] * r.wb[0] + p[1] * r.wb[1] + p[2] * r.wb[2]
    };
    // Each output pixel averages the source over its footprint, so fine texture does not
    // alias into false edges.
    let step = (iw.max(ih) as f32 / w.max(h) as f32).max(1.);
    let taps = (step.round() as usize).clamp(1, 4);
    let mut out = vec![0f32; w * h];
    // Taps with no photo behind them (manual Distortion's white border), which render
    // white; they are set to white once the white level is known.
    let mut blank = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut sum = 0.;
            for j in 0..taps {
                for i in 0..taps {
                    let u = (x as f32 + (i as f32 + 0.5) / taps as f32) / w as f32;
                    let v = (y as f32 + (j as f32 + 0.5) / taps as f32) / h as f32;
                    let [sx, sy] = g.source(u, v);
                    if g.outside(sx, sy) {
                        blank[y * w + x] += 1;
                        continue;
                    }
                    let [sx, sy] = lens.as_ref().map_or([sx, sy], |l| l.forward(sx, sy));
                    // Clamped to the edge, as rendering samples.
                    sum += at(sx.max(0.) as usize, sy.max(0.) as usize);
                }
            }
            out[y * w + x] = sum / (taps * taps) as f32;
        }
    }
    let mut sorted: Vec<f32> = out.iter().step_by(7).copied().collect();
    sorted.sort_by(f32::total_cmp);
    let white = sorted
        .get(sorted.len() * 99 / 100)
        .copied()
        .unwrap_or(1.)
        .max(1e-6);
    let taps = (taps * taps) as f32;
    for (v, blank) in out.iter_mut().zip(blank) {
        *v = (*v / white + blank as f32 / taps).clamp(0., 1.).sqrt() * 255.;
    }
    (out, w, h)
}

/// Straight edges, found as in LSD (von Gioi et al., 2012): pixels with a strong
/// gradient grow into regions whose level-line angle stays within 22.5°, and each
/// region long and dense enough becomes a segment.
pub fn segments(image: &[f32], w: usize, h: usize) -> Vec<Segment> {
    if w < 3 || h < 3 {
        return Vec::new();
    }
    const TOLERANCE: f32 = std::f32::consts::PI / 8.;
    // LSD's gradient threshold for a quantisation error of 2 levels.
    let threshold = 2. / TOLERANCE.sin();
    let (gw, gh) = (w - 1, h - 1);
    let mut angle = vec![0f32; gw * gh];
    let mut magnitude = vec![0f32; gw * gh];
    for y in 0..gh {
        for x in 0..gw {
            let i = y * w + x;
            let a = image[i + w + 1] - image[i];
            let b = image[i + 1] - image[i + w];
            let (gx, gy) = (a + b, a - b);
            let m = 0.5 * gx.hypot(gy);
            magnitude[y * gw + x] = m;
            angle[y * gw + x] = gx.atan2(-gy);
        }
    }
    // Seeds in decreasing gradient order.
    let mut order: Vec<u32> = (0..(gw * gh) as u32)
        .filter(|&i| magnitude[i as usize] > threshold)
        .collect();
    order.sort_unstable_by(|&a, &b| magnitude[b as usize].total_cmp(&magnitude[a as usize]));
    let mut used = vec![false; gw * gh];
    for (u, &m) in used.iter_mut().zip(&magnitude) {
        *u = m <= threshold;
    }
    let min_pixels = (w.max(h) as f32 * 0.015) as usize;
    let long = w.max(h) as f32;
    let mut out = Vec::new();
    let mut region: Vec<usize> = Vec::new();
    for &seed in &order {
        let seed = seed as usize;
        if used[seed] {
            continue;
        }
        region.clear();
        region.push(seed);
        used[seed] = true;
        let (mut sx, mut sy) = (angle[seed].cos(), angle[seed].sin());
        let mut theta = angle[seed];
        let mut next = 0;
        while next < region.len() {
            let p = region[next];
            next += 1;
            let (px, py) = ((p % gw) as isize, (p / gw) as isize);
            for dy in -1..=1isize {
                for dx in -1..=1isize {
                    let (x, y) = (px + dx, py + dy);
                    if x < 0 || y < 0 || x >= gw as isize || y >= gh as isize {
                        continue;
                    }
                    let q = y as usize * gw + x as usize;
                    if used[q] || !aligned(angle[q], theta, TOLERANCE) {
                        continue;
                    }
                    used[q] = true;
                    region.push(q);
                    sx += angle[q].cos();
                    sy += angle[q].sin();
                    theta = sy.atan2(sx);
                }
            }
        }
        if region.len() < min_pixels {
            continue;
        }
        if let Some(s) = rectangle(&region, &magnitude, gw, theta) {
            let length = (s.1[0] - s.0[0]).hypot(s.1[1] - s.0[1]);
            // A region much wider than a line, or sparse, is texture rather than an edge.
            if length >= min_pixels as f32 && region.len() as f32 >= 0.7 * length * s.2 {
                let map = |p: [f32; 2]| {
                    [
                        (p[0] + 1. - 0.5 * w as f32) / long,
                        (p[1] + 1. - 0.5 * h as f32) / long,
                    ]
                };
                out.push(Segment {
                    a: map(s.0),
                    b: map(s.1),
                });
            }
        }
    }
    out
}
fn aligned(a: f32, theta: f32, tolerance: f32) -> bool {
    let mut d = (a - theta).abs();
    if d > std::f32::consts::PI {
        d = 2. * std::f32::consts::PI - d;
    }
    d <= tolerance
}
/// The region's ends along its principal axis (gradient-weighted, as LSD's rectangle)
/// and its width in pixels. Coordinates are of the gradient grid, whose samples sit
/// between image pixels.
fn rectangle(
    region: &[usize],
    magnitude: &[f32],
    gw: usize,
    theta: f32,
) -> Option<([f32; 2], [f32; 2], f32)> {
    let (mut cx, mut cy, mut sum) = (0f32, 0f32, 0f32);
    for &p in region {
        let m = magnitude[p];
        cx += m * (p % gw) as f32;
        cy += m * (p / gw) as f32;
        sum += m;
    }
    if sum <= 0. {
        return None;
    }
    let (cx, cy) = (cx / sum, cy / sum);
    let (mut ixx, mut iyy, mut ixy) = (0f32, 0f32, 0f32);
    for &p in region {
        let m = magnitude[p];
        let (dx, dy) = ((p % gw) as f32 - cx, (p / gw) as f32 - cy);
        ixx += m * dy * dy;
        iyy += m * dx * dx;
        ixy -= m * dx * dy;
    }
    // The direction of the smallest inertia, disambiguated by the level-line angle.
    let lambda = 0.5 * (ixx + iyy - ((ixx - iyy).powi(2) + 4. * ixy * ixy).sqrt());
    let mut dir = if ixx.abs() > iyy.abs() {
        (lambda - ixx).atan2(ixy)
    } else {
        ixy.atan2(lambda - iyy)
    };
    if !aligned(dir, theta, std::f32::consts::FRAC_PI_2) {
        dir += std::f32::consts::PI;
    }
    let (dx, dy) = (dir.cos(), dir.sin());
    let (mut l0, mut l1, mut w0, mut w1) = (0f32, 0f32, 0f32, 0f32);
    for &p in region {
        let (x, y) = ((p % gw) as f32 - cx, (p / gw) as f32 - cy);
        let l = x * dx + y * dy;
        let wv = -x * dy + y * dx;
        l0 = l0.min(l);
        l1 = l1.max(l);
        w0 = w0.min(wv);
        w1 = w1.max(wv);
    }
    Some((
        [cx + l0 * dx, cy + l0 * dy],
        [cx + l1 * dx, cy + l1 * dy],
        (w1 - w0).max(1.),
    ))
}

type Mat = [[f32; 3]; 3];
const IDENTITY: Mat = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
fn mat(a: Mat, b: Mat) -> Mat {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}
fn apply(m: Mat, v: [f32; 3]) -> [f32; 3] {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(a: [f32; 3]) -> [f32; 3] {
    let n = dot(a, a).sqrt().max(1e-12);
    [a[0] / n, a[1] / n, a[2] / n]
}
/// Rotation by `angle` radians about `axis`.
fn rotation(axis: [f32; 3], angle: f32) -> Mat {
    let [x, y, z] = unit(axis);
    let (s, c) = angle.sin_cos();
    let t = 1. - c;
    [
        [c + x * x * t, x * y * t - z * s, x * z * t + y * s],
        [y * x * t + z * s, c + y * y * t, y * z * t - x * s],
        [z * x * t - y * s, z * y * t + x * s, c + z * z * t],
    ]
}
/// The smallest rotation taking direction `from` to `to`.
fn align(from: [f32; 3], to: [f32; 3]) -> Mat {
    let (from, to) = (unit(from), unit(to));
    let axis = cross(from, to);
    let s = dot(axis, axis).sqrt();
    if s < 1e-9 {
        return IDENTITY;
    }
    rotation(axis, s.atan2(dot(from, to)))
}

/// A segment's plane through the camera centre, as its unit normal, for focal length
/// `f` in long-edge units.
fn normal(s: &Segment, f: f32) -> [f32; 3] {
    unit(cross(
        [s.a[0] / f, s.a[1] / f, 1.],
        [s.b[0] / f, s.b[1] / f, 1.],
    ))
}
/// Degrees between a segment and the line from its midpoint towards the vanishing
/// direction `d`.
fn deviation(s: &Segment, d: [f32; 3], f: f32) -> f32 {
    let m = [0.5 * (s.a[0] + s.b[0]), 0.5 * (s.a[1] + s.b[1])];
    let to = [d[0] * f - m[0] * d[2], d[1] * f - m[1] * d[2]];
    let dir = [s.b[0] - s.a[0], s.b[1] - s.a[1]];
    let sin = (dir[0] * to[1] - dir[1] * to[0]).abs()
        / (dir[0].hypot(dir[1]) * to[0].hypot(to[1])).max(1e-12);
    sin.min(1.).asin().to_degrees()
}
/// The vanishing direction of `segments` best supported by the longest of them, and
/// its support (the summed squared length of the segments that meet it). `accept`
/// rules out implausible directions; `prior` weighs the rest.
fn vanishing(
    segments: &[Segment],
    f: f32,
    accept: impl Fn([f32; 3]) -> bool,
    prior: impl Fn([f32; 3]) -> f32,
) -> Option<([f32; 3], f32)> {
    let mut by_length: Vec<&Segment> = segments.iter().collect();
    by_length.sort_by(|a, b| b.length().total_cmp(&a.length()));
    let normals: Vec<[f32; 3]> = segments.iter().map(|s| normal(s, f)).collect();
    let support = |d: [f32; 3], tolerance: f32| -> f32 {
        segments
            .iter()
            .filter(|s| deviation(s, d, f) < tolerance)
            .map(|s| s.length().powi(2))
            .sum()
    };
    let candidates = &by_length[..by_length.len().min(150)];
    let mut best: Option<([f32; 3], f32)> = None;
    for (i, a) in candidates.iter().enumerate() {
        for b in &candidates[i + 1..] {
            let d = cross(normal(a, f), normal(b, f));
            if dot(d, d) < 1e-12 {
                continue;
            }
            let d = unit(d);
            let d = if d[1] < 0. || (d[1] == 0. && d[0] < 0.) {
                [-d[0], -d[1], -d[2]]
            } else {
                d
            };
            if !accept(d) {
                continue;
            }
            let score = support(d, 2.) * prior(d);
            if best.is_none_or(|(_, s)| score > s) {
                best = Some((d, score));
            }
        }
    }
    let (mut d, _) = best?;
    // Refine on the segments that meet it: the direction closest to all their planes.
    for _ in 0..3 {
        let mut m = [[0f32; 3]; 3];
        for (s, n) in segments.iter().zip(&normals) {
            if deviation(s, d, f) < 1. {
                let w = s.length().powi(2);
                for i in 0..3 {
                    for j in 0..3 {
                        m[i][j] += w * n[i] * n[j];
                    }
                }
            }
        }
        let refined = smallest_eigenvector(m);
        let refined = if dot(refined, d) < 0. {
            [-refined[0], -refined[1], -refined[2]]
        } else {
            refined
        };
        if !refined.iter().all(|v| v.is_finite()) || !accept(refined) {
            break;
        }
        d = refined;
    }
    Some((d, support(d, 1.)))
}
/// Eigenvector of the smallest eigenvalue of a symmetric 3×3 matrix, by inverse
/// iteration on a shifted copy.
fn smallest_eigenvector(m: Mat) -> [f32; 3] {
    let trace = m[0][0] + m[1][1] + m[2][2];
    // Power iteration on (trace·I − m) finds the eigenvector of m's smallest eigenvalue.
    let s: Mat = std::array::from_fn(|i| {
        std::array::from_fn(|j| if i == j { trace - m[i][j] } else { -m[i][j] })
    });
    let mut v = [0.3, 0.9, 0.3];
    for _ in 0..200 {
        v = unit([dot(s[0], v), dot(s[1], v), dot(s[2], v)]);
    }
    v
}

/// What the analysis found, in the displayed photo's camera frame (x right, y down,
/// z forward).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vanishing {
    /// Direction of vertical lines, pointing down the photo.
    pub vertical: Option<[f32; 3]>,
    /// Direction of horizontal lines, orthogonal to `vertical`.
    pub horizontal: Option<[f32; 3]>,
    /// Roll angle of the horizon in radians, for Level when there is no vertical.
    pub horizon: f32,
}
/// Minimum support of a vanishing direction; below it Lightroom corrects nothing.
const MIN_SUPPORT: f32 = 0.01;
pub fn vanishing_points(segments: &[Segment], f: f32) -> Vanishing {
    let angle = |s: &Segment| {
        (s.b[1] - s.a[1])
            .atan2(s.b[0] - s.a[0])
            .to_degrees()
            .rem_euclid(180.)
    };
    let near_vertical: Vec<Segment> = segments
        .iter()
        .filter(|s| (angle(s) - 90.).abs() < 20.)
        .copied()
        .collect();
    let tilt = |d: [f32; 3]| d[2].abs().atan2(d[1]).to_degrees();
    let vertical = vanishing(
        &near_vertical,
        f,
        |d| tilt(d) < 50. && d[0].abs().atan2(d[1]).to_degrees() < 20.,
        // Most photos lean a few degrees; strong tilts need strong evidence.
        |d| (-0.5 * (tilt(d) / 15.).powi(2)).exp(),
    )
    .filter(|&(_, support)| support >= MIN_SUPPORT)
    .map(|(d, _)| d);
    let others: Vec<Segment> = segments
        .iter()
        .filter(|s| (angle(s) - 90.).abs() >= 20.)
        .copied()
        .collect();
    let horizontal = vertical.and_then(|v| {
        vanishing(
            &others,
            f,
            // Orthogonal to the vertical within 10°.
            |d| dot(d, v).abs() < 0.17,
            |_| 1.,
        )
        .filter(|&(_, support)| support >= MIN_SUPPORT)
        .map(|(d, _)| {
            let d = [
                d[0] - dot(d, v) * v[0],
                d[1] - dot(d, v) * v[1],
                d[2] - dot(d, v) * v[2],
            ];
            let d = unit(d);
            if d[0] < 0. { [-d[0], -d[1], -d[2]] } else { d }
        })
    });
    // Without a vertical, level the long near-horizontal edges.
    let (mut sum, mut weight) = (0f32, 0f32);
    for s in segments {
        let a = (s.b[1] - s.a[1]).atan2(s.b[0] - s.a[0]);
        let a = (a + std::f32::consts::FRAC_PI_2).rem_euclid(std::f32::consts::PI)
            - std::f32::consts::FRAC_PI_2;
        if a.abs() < 5f32.to_radians() {
            let w = s.length().powi(2);
            sum += w * a;
            weight += w;
        }
    }
    Vanishing {
        vertical,
        horizontal,
        horizon: if weight >= MIN_SUPPORT {
            sum / weight
        } else {
            0.
        },
    }
}

/// The share of a vertical tilt (degrees) that Auto corrects, fitted loosely to
/// Lightroom's choices on 426 photos: little for slight tilts, most around 8°, less
/// again for strong ones.
fn auto_tilt_share(tilt: f32) -> f32 {
    let t = tilt.abs();
    0.75 * (1. - (-(t / 4.).powi(2)).exp()) * (-(t - 10.).max(0.) / 15.).exp()
}

/// Lightroom's rotations for each Upright mode, indexed by [`super::UprightMode::code`]:
/// Level rolls, Vertical also tilts, Full also pans, Auto corrects part of the tilt and
/// only a slight pan.
pub fn rotations(v: &Vanishing) -> [Mat; 5] {
    let down = [0., 1., 0.];
    let roll = |d: [f32; 3]| d[0].atan2(d[1]);
    let level = match v.vertical {
        Some(d) => rotation([0., 0., 1.], roll(d)),
        None => rotation([0., 0., 1.], -v.horizon),
    };
    let Some(d) = v.vertical else {
        return [IDENTITY, level, level, level, level];
    };
    let vertical = align(d, down);
    // Tilt left once the roll is removed, and what Auto keeps of it.
    let rolled = apply(level, d);
    let tilt = (-rolled[2]).atan2(rolled[1]);
    let kept = tilt * (1. - auto_tilt_share(tilt.to_degrees()));
    let mut auto = align(d, [0., kept.cos(), -kept.sin()]);
    let mut full = vertical;
    if let Some(h) = v.horizontal {
        let h = apply(vertical, h);
        // The facade turned least: the horizontal lines and those at right angles to
        // them are both candidates.
        let quarter = std::f32::consts::FRAC_PI_2;
        let pan = (h[2].atan2(h[0]) + 0.5 * quarter).rem_euclid(quarter) - 0.5 * quarter;
        full = mat(rotation(down, pan), vertical);
        if pan.to_degrees().abs() < 5. {
            auto = mat(rotation(down, 0.4 * pan), auto);
        }
    }
    [IDENTITY, auto, full, level, vertical]
}

/// Focal length in units of the long edge: 35mm-equivalent over 36 mm, as Lightroom's
/// `UprightFocalLength35mm` (checked on its stored corrections).
pub fn focal(m: &crate::raw::Metadata) -> f32 {
    let equivalent = if m.focal_35mm > 0. {
        m.focal_35mm
    } else if m.focal > 0. {
        // Fujifilm leaves the 35mm equivalent out; its GFX sensors are 44 mm wide and
        // the rest APS-C.
        let fujifilm_gfx = m.make.eq_ignore_ascii_case("FUJIFILM") && m.model.starts_with("GFX");
        m.focal * if fujifilm_gfx { 0.79 } else { 1.5226 }
    } else {
        35.
    };
    equivalent / 36.
}

/// Where the corrected photo sits in the frame. Lightroom enlarges it to fill the frame
/// (moving it as needed) when that takes at most 110%; otherwise Level keeps its size
/// and the other modes fit its width. Measured on Lightroom's stored corrections.
fn framing(g: Mat, width: f32, height: f32, level: bool) -> Mat {
    let (hw, hh) = (0.5 * width, 0.5 * height);
    let frame = [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]];
    let quad = frame.map(|[x, y]| {
        let p = apply(g, [x, y, 1.]);
        [p[0] / p[2], p[1] / p[2]]
    });
    let centre = [
        quad.iter().map(|p| p[0]).sum::<f32>() / 4.,
        quad.iter().map(|p| p[1]).sum::<f32>() / 4.,
    ];
    // Each edge of the quad as an inward normal n and a point a on it.
    let edges: Vec<([f32; 2], [f32; 2])> = (0..4)
        .map(|i| {
            let (a, b) = (quad[i], quad[(i + 1) % 4]);
            let mut n = [a[1] - b[1], b[0] - a[0]];
            if n[0] * (centre[0] - a[0]) + n[1] * (centre[1] - a[1]) < 0. {
                n = [-n[0], -n[1]];
            }
            (n, a)
        })
        .collect();
    // The frame fits inside s·quad + t when n·t ≤ n·p − s·n·a for every corner p and
    // edge; these are half-planes in t, so when any t fits, one where two of their
    // lines meet does.
    let fits = |s: f32| -> Option<[f32; 2]> {
        let lines: Vec<([f32; 2], f32)> = edges
            .iter()
            .flat_map(|&(n, a)| {
                frame.map(|p| {
                    (
                        n,
                        n[0] * p[0] + n[1] * p[1] - s * (n[0] * a[0] + n[1] * a[1]),
                    )
                })
            })
            .collect();
        let ok = |t: [f32; 2]| {
            lines
                .iter()
                .all(|&(n, c)| n[0] * t[0] + n[1] * t[1] <= c + 1e-6)
        };
        if ok([0., 0.]) {
            return Some([0., 0.]);
        }
        for (i, &(n1, c1)) in lines.iter().enumerate() {
            for &(n2, c2) in &lines[i + 1..] {
                let det = n1[0] * n2[1] - n1[1] * n2[0];
                if det.abs() < 1e-12 {
                    continue;
                }
                let t = [
                    (c1 * n2[1] - c2 * n1[1]) / det,
                    (n1[0] * c2 - n2[0] * c1) / det,
                ];
                if ok(t) {
                    return Some(t);
                }
            }
        }
        None
    };
    let scaled = |s: f32, t: [f32; 2]| [[s, 0., t[0]], [0., s, t[1]], [0., 0., 1.]];
    if let Some(t) = fits(1.1) {
        let (mut lo, mut hi, mut best) = (0.5f32, 1.1f32, t);
        for _ in 0..30 {
            let mid = 0.5 * (lo + hi);
            match fits(mid) {
                Some(t) => (hi, best) = (mid, t),
                None => lo = mid,
            }
        }
        return scaled(hi, best);
    }
    if level {
        return IDENTITY;
    }
    let (x0, x1) = quad.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
        (lo.min(p[0]), hi.max(p[0]))
    });
    let (y0, y1) = quad.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
        (lo.min(p[1]), hi.max(p[1]))
    });
    let s = width / (x1 - x0);
    scaled(s, [-s * 0.5 * (x0 + x1), -s * 0.5 * (y0 + y1)])
}

/// Lightroom's Upright corrections for every mode, indexed by
/// [`super::UprightMode::code`], as [`super::Upright::corrections`] stores them: forward
/// homographies in 0–1 coordinates of the photo as recorded. Guided needs guides drawn
/// on the photo, so there is none for it.
pub fn analyse(im: &CameraImage, r: &Recipe) -> Vec<[f32; 9]> {
    let (image, w, h) = analysis_image(im, r);
    let f = focal(&im.metadata);
    let found = vanishing_points(&segments(&image, w, h), f);
    let long = w.max(h) as f32;
    let (width, height) = (w as f32 / long, h as f32 / long);
    let k = [[f, 0., 0.], [0., f, 0.], [0., 0., 1.]];
    let k_inverse = [[1. / f, 0., 0.], [0., 1. / f, 0.], [0., 0., 1.]];
    // 0–1 coordinates of the displayed photo to centred long-edge units, and displayed
    // 0–1 coordinates to recorded ones (flips, then turns, as `Geometry::source`).
    let centred = [
        [width, 0., -0.5 * width],
        [0., height, -0.5 * height],
        [0., 0., 1.],
    ];
    let turns = (super::ImageFrame::new(im).turns + r.rotation) % 4;
    let recorded = |x: f32, y: f32| {
        let x = if r.flip_x { 1. - x } else { x };
        let y = if r.flip_y { 1. - y } else { y };
        super::image_space::turn(turns, x, y)
    };
    let [ox, oy] = recorded(0., 0.);
    let [xx, xy] = recorded(1., 0.);
    let [yx, yy] = recorded(0., 1.);
    let orient = [[xx - ox, yx - ox, ox], [xy - oy, yy - oy, oy], [0., 0., 1.]];
    let to_recorded = mat(orient, crate::color_math::inverse(centred));
    let from_recorded = mat(centred, crate::color_math::inverse(orient));
    let rotations = rotations(&found);
    let mut out = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 5];
    for (code, rotation) in rotations.iter().enumerate().skip(1) {
        let g = mat(k, mat(*rotation, k_inverse));
        let level = code == super::UprightMode::Level.code();
        let g = mat(framing(g, width, height, level), g);
        let g = mat(to_recorded, mat(g, from_recorded));
        out[code] = std::array::from_fn(|i| g[i / 3][i % 3] / g[2][2]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A photo of vertical edges taken with the camera tilted up by `tilt` degrees and
    /// rolled by `roll`: light and dark wedges meeting at the vertical vanishing point.
    fn tilted(tilt: f32, roll: f32, f: f32) -> (Vec<f32>, usize, usize, [f32; 3]) {
        let (w, h) = (900usize, 600usize);
        let camera = mat(
            rotation([0., 0., 1.], roll.to_radians()),
            rotation([1., 0., 0.], tilt.to_radians()),
        );
        let d = apply(camera, [0., 1., 0.]);
        let vp = [d[0] * f / d[2], d[1] * f / d[2]];
        let long = w as f32;
        // Supersampled, so edges are smooth rather than pixel steps.
        let image = (0..w * h)
            .map(|i| {
                let mut sum = 0.;
                for j in 0..16 {
                    let x = ((i % w) as f32 + (j % 4) as f32 / 4. + 0.125 - 0.5 * w as f32) / long;
                    let y = ((i / w) as f32 + (j / 4) as f32 / 4. + 0.125 - 0.5 * h as f32) / long;
                    let a = (x - vp[0]).atan2(vp[1] - y);
                    sum += if (a * 180.).sin() > 0. { 200. } else { 40. };
                }
                sum / 16.
            })
            .collect();
        (image, w, h, if d[1] < 0. { d.map(|v| -v) } else { d })
    }

    /// Manual Distortion's white border is analysed as the white it renders, not as
    /// edge pixels stretched into it.
    #[test]
    fn analysis_sees_manual_distortions_white_border() {
        let im = CameraImage {
            width: 300,
            height: 200,
            // Darker towards the left edge, which the border covers.
            pixels: (0..300 * 200)
                .map(|i| [0.05 + 0.9 * (i % 300) as f32 / 300.; 3])
                .collect(),
            metadata: crate::raw::Metadata {
                width: 300,
                height: 200,
                wb: [1.; 3],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
            recovered: Default::default(),
        };
        let r = Recipe {
            wb: [1.; 3],
            lens_manual_distortion: 1.,
            ..Default::default()
        };
        let (image, w, h) = analysis_image(&im, &r);
        assert_eq!(image[h / 2 * w], 255.);
        assert!(image[h / 2 * w + w / 2] < 255.);
    }

    #[test]
    fn finds_the_vertical_of_a_tilted_camera() {
        let f = 1.;
        let (image, w, h, truth) = tilted(8., 1.5, f);
        let found = vanishing_points(&segments(&image, w, h), f);
        let d = found.vertical.expect("a vertical vanishing point");
        let error = dot(d, truth).clamp(-1., 1.).acos().to_degrees();
        assert!(error < 0.2, "{error}° from {truth:?}: {d:?}");
    }

    #[test]
    fn vertical_mode_makes_converging_edges_parallel_and_level_only_rolls() {
        let f = 1.2;
        let camera = mat(
            rotation([0., 0., 1.], 2f32.to_radians()),
            rotation([1., 0., 0.], -6f32.to_radians()),
        );
        let d = apply(camera, [0., 1., 0.]);
        let v = Vanishing {
            vertical: Some(if d[1] < 0. { d.map(|v| -v) } else { d }),
            ..Default::default()
        };
        let [_, _, _, level, vertical] = rotations(&v);
        // Each world vertical through a point of the photo, before and after.
        let k = [[f, 0., 0.], [0., f, 0.], [0., 0., 1.]];
        let k_inverse = [[1. / f, 0., 0.], [0., 1. / f, 0.], [0., 0., 1.]];
        let project = |r: Mat, p: [f32; 3]| {
            let q = apply(mat(k, mat(r, k_inverse)), p);
            [q[0] / q[2], q[1] / q[2]]
        };
        for x in [-0.4f32, 0., 0.4] {
            let a = [x, 0.2, 1.];
            let b = [
                a[0] + 0.1 * d[0] * f,
                a[1] + 0.1 * d[1] * f,
                1. + 0.1 * d[2],
            ];
            let (p, q) = (project(vertical, a), project(vertical, b));
            let lean = (q[0] - p[0]).atan2(q[1] - p[1]).to_degrees();
            assert!(lean.abs() < 0.01, "{x}: {lean}°");
        }
        // Level turns the photo in its plane only.
        assert!(level[2][0].abs() < 1e-6 && level[2][1].abs() < 1e-6);
        assert!(
            (level[0][1].asin().to_degrees() - 2.).abs() < 0.01,
            "{level:?}"
        );
    }

    #[test]
    fn framing_fills_small_corrections_and_fits_large_ones() {
        let (w, h) = (1., 2. / 3.);
        // A 1° turn needs 1.03× to fill the frame, as Level does.
        let turn = rotation([0., 0., 1.], 1f32.to_radians());
        let a = framing(turn, w, h, true);
        let expected = 1f32.to_radians().cos() + 1f32.to_radians().sin() * 1.5;
        assert!((a[0][0] - expected).abs() < 1e-4, "{a:?}");
        // 10° would need more than 110%: Level keeps the size, the others fit the width.
        let turn = rotation([0., 0., 1.], 10f32.to_radians());
        assert_eq!(framing(turn, w, h, true), IDENTITY);
        let (s, c) = 10f32.to_radians().sin_cos();
        let a = framing(turn, w, h, false);
        assert!((a[0][0] - w / (w * c + h * s)).abs() < 1e-4, "{a:?}");
    }
}
