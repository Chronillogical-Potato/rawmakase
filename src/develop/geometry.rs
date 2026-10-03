use super::Recipe;
use crate::raw::CameraImage;
use serde::{Deserialize, Serialize};

/// Lightroom's Transform panel (manual sliders). Applied after lens correction and
/// before crop, in the oriented frame. Slider values are Lightroom's divided by 100,
/// except `rotate` (degrees) and `scale` (1 = 100%).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Transform {
    pub vertical: f32,
    pub horizontal: f32,
    pub rotate: f32,
    pub aspect: f32,
    pub scale: f32,
    pub offset_x: f32,
    pub offset_y: f32,
}
impl Default for Transform {
    fn default() -> Self {
        Self {
            vertical: 0.,
            horizontal: 0.,
            rotate: 0.,
            aspect: 0.,
            scale: 1.,
            offset_x: 0.,
            offset_y: 0.,
        }
    }
}
/// How the displayed photo's axes lie in the frame the camera recorded: `m` maps a
/// displayed direction (x right, y down) to a recorded one. Its entries are 0 or ±1.
pub fn display_axes(turns: u8, flip_x: bool, flip_y: bool) -> [[f32; 2]; 2] {
    let recorded = |x: f32, y: f32| {
        let x = if flip_x { 1. - x } else { x };
        let y = if flip_y { 1. - y } else { y };
        super::image_space::turn(turns, x, y)
    };
    let [ox, oy] = recorded(0., 0.);
    let [xx, xy] = recorded(1., 0.);
    let [yx, yy] = recorded(0., 1.);
    [[xx - ox, yx - ox], [xy - oy, yy - oy]]
}
impl Transform {
    /// The sliders as Lightroom shows them on the displayed photo, when `self` holds
    /// them as stored, in the recorded frame (`m` from [`display_axes`]). Lightroom
    /// stores Vertical −70 on a photo turned 90° left as `PerspectiveHorizontal` +70.
    pub fn displayed(&self, m: [[f32; 2]; 2]) -> Self {
        self.reoriented([[m[0][0], m[1][0]], [m[0][1], m[1][1]]])
    }
    /// The stored sliders for sliders shown on the displayed photo: the inverse of
    /// [`Self::displayed`].
    pub fn recorded(&self, m: [[f32; 2]; 2]) -> Self {
        self.reoriented(m)
    }
    /// Perspective (h, v) and offsets (x, −y) are vectors in the frame; a swap of axes
    /// flips Aspect, and a mirror flips Rotate.
    fn reoriented(&self, m: [[f32; 2]; 2]) -> Self {
        let map = |[x, y]: [f32; 2]| [m[0][0] * x + m[0][1] * y, m[1][0] * x + m[1][1] * y];
        let [horizontal, vertical] = map([self.horizontal, self.vertical]);
        let [offset_x, minus_y] = map([self.offset_x, -self.offset_y]);
        let determinant = m[0][0] * m[1][1] - m[0][1] * m[1][0];
        Self {
            vertical,
            horizontal,
            rotate: self.rotate * determinant,
            aspect: if m[0][0] == 0. {
                -self.aspect
            } else {
                self.aspect
            },
            scale: self.scale,
            offset_x,
            offset_y: -minus_y,
        }
    }
    pub fn is_identity(&self) -> bool {
        *self == Self::default()
    }
    pub fn validate(&self) -> bool {
        [
            self.vertical,
            self.horizontal,
            self.aspect,
            self.offset_x,
            self.offset_y,
        ]
        .iter()
        .all(|v| v.is_finite() && v.abs() <= 1.)
            && self.rotate.is_finite()
            && self.rotate.abs() <= 10.
            && (0.5..=1.5).contains(&self.scale)
    }
    /// Homography from output to source coordinates, both centred, y down, in units
    /// of the long edge. The forward (source-to-output) matrix was fitted to Camera Raw
    /// 18.6 renders (docs/transform.md): Rotate applies first, then Vertical and
    /// Horizontal together, then Aspect, Scale and the offsets. With q = (h, v) and
    /// s = |q|, Vertical and Horizontal are [[I + e(s) q qᵀ / s², 0], [−qᵀ, 1]] with
    /// e(s) = 0.0391 s² + 0.9251 s³ − 0.2827 s⁴; Aspect scales y by 2^(0.137 a) and x by
    /// the inverse; offsets move by 0.811 of the image size, positive Y upward.
    fn inverse(&self, width: f32, height: f32) -> [[f32; 3]; 3] {
        let (qx, qy) = (self.horizontal, self.vertical);
        let s = qx.hypot(qy);
        // e(s) / s²
        let e = 0.0391 + 0.9251 * s - 0.2827 * s * s;
        let perspective = [
            [1. + e * qx * qx, e * qx * qy, 0.],
            [e * qx * qy, 1. + e * qy * qy, 0.],
            [-qx, -qy, 1.],
        ];
        let (sr, cr) = self.rotate.to_radians().sin_cos();
        let rotate = [[cr, -sr, 0.], [sr, cr, 0.], [0., 0., 1.]];
        let a = 2f32.powf(0.137 * self.aspect);
        let scale = [
            [self.scale / a, 0., 0.],
            [0., self.scale * a, 0.],
            [0., 0., 1.],
        ];
        let offset = [
            [1., 0., self.offset_x * 0.811 * width],
            [0., 1., -self.offset_y * 0.811 * height],
            [0., 0., 1.],
        ];
        let forward = mat(offset, mat(scale, mat(perspective, rotate)));
        crate::color_math::inverse(forward)
    }
}
/// Lightroom's Upright modes, in Adobe's `crs:PerspectiveUpright` order.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UprightMode {
    #[default]
    Off,
    Auto,
    Full,
    Level,
    Vertical,
    Guided,
}
impl UprightMode {
    pub const ALL: [Self; 6] = [
        Self::Off,
        Self::Auto,
        Self::Full,
        Self::Level,
        Self::Vertical,
        Self::Guided,
    ];
    /// Adobe's `crs:PerspectiveUpright` value, which also indexes `UprightTransform_N`.
    pub fn code(self) -> usize {
        self as usize
    }
    pub fn from_code(code: usize) -> Option<Self> {
        Self::ALL.get(code).copied()
    }
}
/// Lightroom's Upright: the chosen mode and the correction for each mode, as Lightroom
/// stores them so that switching modes needs no new analysis.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Upright {
    pub mode: UprightMode,
    /// Forward (source-to-output) homographies indexed by [`UprightMode::code`], row
    /// major, in 0–1 coordinates of the photo as the camera recorded it, before any
    /// rotation or flip and after the camera's default crop. This is how Lightroom
    /// stores `crs:UprightTransform_N`; Camera Raw renders them exactly (docs/transform.md).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub corrections: Vec<[f32; 9]>,
    /// Lightroom's other Upright settings (analysis centre, focal length, version,
    /// guides), kept to write back unchanged.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub lightroom: std::collections::BTreeMap<String, String>,
}
impl Upright {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// Keeps the mode but drops what was analysed from one photo, for settings moving
    /// to another: the corrections, and Lightroom's analysis details.
    pub fn clear_analysis(&mut self) {
        self.corrections.clear();
        self.lightroom.clear();
        // Guided can't be analysed again without its guides.
        if self.mode == UprightMode::Guided {
            self.mode = UprightMode::Off;
        }
    }
    pub fn validate(&self) -> bool {
        self.corrections.len() <= UprightMode::ALL.len()
            && self.corrections.iter().all(|m| {
                let [a, b, c, d, e, f, g, h, i] = *m;
                let determinant = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
                // Rendering inverts it; a singular one would silently render as Off.
                m.iter().all(|v| v.is_finite()) && determinant.abs() > 1e-6
            })
    }
    /// The chosen mode's correction, when it is not the identity.
    pub fn correction(&self) -> Option<[[f32; 3]; 3]> {
        if self.mode == UprightMode::Off {
            return None;
        }
        let m = self.corrections.get(self.mode.code())?;
        let m: [[f32; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| m[3 * i + j]));
        (m != IDENTITY && m[2][2] != 0.).then_some(m)
    }
}
/// Lightroom's manual lens Distortion, measured on Camera Raw 18.7 renders of the
/// synthetic chart: an output position at radius r (1 at the frame's corners) samples
/// the photo at radius r·(1 + k·(1 − r²)), with k = 0.4 × amount for positive amounts
/// and 0.5 × amount for negative ones. Corners stay put; positive amounts pull the
/// edges' middles in from outside the photo, which shows white, as Lightroom shows it
/// without Constrain Crop. It applies in the frame as recorded, after Upright, the
/// Transform sliders and the crop take an output position back to it, and before the
/// lens profile (docs/lens-corrections.md#manual-distortion).
#[derive(Clone, Copy, Debug, PartialEq)]
struct ManualDistortion {
    k: f32,
    /// Half the frame's width and height over its half diagonal.
    axes: [f32; 2],
}
impl ManualDistortion {
    /// For Lightroom's amount (−1 to 1) on a `width` by `height` frame; `None` at 0.
    fn new(amount: f32, width: f32, height: f32) -> Option<Self> {
        if amount == 0. {
            return None;
        }
        let diagonal = width.hypot(height);
        Some(Self {
            k: amount * if amount > 0. { 0.4 } else { 0.5 },
            axes: [width / diagonal, height / diagonal],
        })
    }
    /// The source position (0–1 of the frame) that output position (`x`, `y`) samples.
    fn source(&self, x: f32, y: f32) -> [f32; 2] {
        let g = self.ratio(self.radius_squared(x, y));
        [0.5 + (x - 0.5) * g, 0.5 + (y - 0.5) * g]
    }
    /// The output position that samples source position (`x`, `y`): the inverse of
    /// [`Self::source`], its radius found with Newton steps (the map is monotonic for
    /// every amount).
    fn output(&self, x: f32, y: f32) -> [f32; 2] {
        let target = self.radius_squared(x, y).sqrt();
        if target < 1e-6 {
            return [x, y];
        }
        let mut rho = target;
        for _ in 0..8 {
            let value = rho * self.ratio(rho * rho) - target;
            let slope = 1. + self.k - 3. * self.k * rho * rho;
            let step = value / slope;
            rho -= step;
            if step.abs() < 1e-7 {
                break;
            }
        }
        let scale = rho / target;
        [0.5 + (x - 0.5) * scale, 0.5 + (y - 0.5) * scale]
    }
    fn radius_squared(&self, x: f32, y: f32) -> f32 {
        let dx = (x - 0.5) * 2. * self.axes[0];
        let dy = (y - 0.5) * 2. * self.axes[1];
        dx * dx + dy * dy
    }
    fn ratio(&self, r2: f32) -> f32 {
        1. + self.k * (1. - r2)
    }
}

const IDENTITY: [[f32; 3]; 3] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
fn mat(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}
/// Inverse map from output normalized coordinates to un-oriented decoded pixels.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    pub width: u32,
    pub height: u32,
    pub oriented_width: f32,
    pub oriented_height: f32,
    crop: [f32; 4],
    angle: f32,
    zoom: f32,
    turns: u8,
    flip_x: bool,
    flip_y: bool,
    source_width: u32,
    source_height: u32,
    inset: [f32; 4],
    /// Output-to-source homography of Upright and the Transform sliders, when not the
    /// identity, in 0–1 coordinates of the photo as recorded (after flips and turns).
    transform: Option<[[f32; 3]; 3]>,
    /// Its inverse, source to output.
    forward: Option<[[f32; 3]; 3]>,
    /// Lightroom's manual Distortion, after the homography on the way to the source.
    manual: Option<ManualDistortion>,
}
impl Geometry {
    pub fn new(im: &CameraImage, r: &Recipe, max_edge: u32) -> Self {
        let frame = super::ImageFrame::new(im);
        let turns = (frame.turns + r.rotation) % 4;
        let [w, h] = frame.size();
        let (ow, oh) = if r.rotation % 2 == 1 { (h, w) } else { (w, h) };
        let angle = r.straighten.to_radians();
        let (s, c) = angle.sin_cos();
        let zoom = (c.abs() + s.abs() * oh / ow).max(c.abs() + s.abs() * ow / oh);
        let w = ow * (r.crop[2] - r.crop[0]);
        let h = oh * (r.crop[3] - r.crop[1]);
        let factor = if max_edge > 0 {
            (max_edge as f32 / w.max(h)).min(1.)
        } else {
            1.
        };
        let (frame_width, frame_height) = (
            im.width as f32 * frame.inset[2],
            im.height as f32 * frame.inset[3],
        );
        let transform = Self::homography(r, frame_width, frame_height);
        let manual = (r.engine >= 4)
            .then(|| ManualDistortion::new(r.lens_manual_distortion, frame_width, frame_height))
            .flatten();
        Self {
            width: (w * factor).round().max(1.) as u32,
            height: (h * factor).round().max(1.) as u32,
            oriented_width: ow,
            oriented_height: oh,
            crop: r.crop,
            angle,
            zoom,
            turns,
            flip_x: r.flip_x,
            flip_y: r.flip_y,
            source_width: im.width,
            source_height: im.height,
            inset: frame.inset,
            transform,
            forward: transform.map(crate::color_math::inverse),
            manual,
        }
    }
    /// Upright followed by the Transform sliders, output to source, in 0–1 coordinates
    /// of the photo as recorded (`width` by `height`). Camera Raw applies both in that
    /// frame, before rotating or flipping the photo, so on a photo turned to portrait
    /// Vertical keystones across the screen (docs/transform.md).
    fn homography(r: &Recipe, width: f32, height: f32) -> Option<[[f32; 3]; 3]> {
        if r.engine < 4 {
            return None;
        }
        let upright = r.upright.correction();
        if upright.is_none() && r.transform.is_identity() {
            return None;
        }
        let mut h = IDENTITY;
        if !r.transform.is_identity() {
            let (sx, sy) = (width / width.max(height), height / width.max(height));
            // 0–1 coordinates to the sliders' centred, long-edge units.
            let centred = [[sx, 0., -0.5 * sx], [0., sy, -0.5 * sy], [0., 0., 1.]];
            h = mat(
                crate::color_math::inverse(centred),
                mat(r.transform.inverse(sx, sy), centred),
            );
        }
        if let Some(u) = upright {
            h = mat(crate::color_math::inverse(u), h);
        }
        Some(h)
    }
    /// Whether a transformed or manually distorted output position has no source pixel;
    /// Lightroom shows white there.
    /// Pixels beyond the camera's default crop count as outside, as in Lightroom.
    pub fn outside(&self, x: f32, y: f32) -> bool {
        let (w, h) = (self.source_width as f32, self.source_height as f32);
        let [left, top, width, height] = self.inset;
        (self.transform.is_some() || self.manual.is_some())
            && (x < left * w - 0.5
                || y < top * h - 0.5
                || x > (left + width) * w - 0.5
                || y > (top + height) * h - 0.5)
    }
    /// The fields `source` reads, laid out for `gpu/local.wgsl` (`S_CROP` to the end of
    /// `S_HOMOGRAPHY`): crop, oriented size, zoom, sine and cosine, turns, flips, inset,
    /// then whether there is a transform and its homography.
    pub(crate) fn gpu_params(&self) -> [f32; 26] {
        let (s, c) = self.angle.sin_cos();
        let h = self.transform.unwrap_or([[0.; 3]; 3]);
        let mut out = [0.; 26];
        out[..4].copy_from_slice(&self.crop);
        out[4..12].copy_from_slice(&[
            self.oriented_width,
            self.oriented_height,
            self.zoom,
            s,
            c,
            self.turns as f32,
            self.flip_x as u8 as f32,
            self.flip_y as u8 as f32,
        ]);
        out[12..16].copy_from_slice(&self.inset);
        out[16] = self.transform.is_some() as u8 as f32;
        out[17..].copy_from_slice(h.as_flattened());
        out
    }
    /// Manual Distortion as `gpu/local.wgsl` reads it (`S_MANUAL`): k, 0 when off, and
    /// the frame's axes.
    pub(crate) fn gpu_manual(&self) -> [f32; 3] {
        self.manual.map_or([0.; 3], |m| [m.k, m.axes[0], m.axes[1]])
    }
    pub fn source(&self, u: f32, v: f32) -> [f32; 2] {
        let mut x = self.crop[0] + u * (self.crop[2] - self.crop[0]);
        let mut y = self.crop[1] + v * (self.crop[3] - self.crop[1]);
        x = (x - 0.5) * self.oriented_width / self.zoom;
        y = (y - 0.5) * self.oriented_height / self.zoom;
        let (s, c) = self.angle.sin_cos();
        let nx = (c * x + s * y) / self.oriented_width + 0.5;
        let ny = (-s * x + c * y) / self.oriented_height + 0.5;
        let (mut x, mut y) = (nx, ny);
        if self.flip_x {
            x = 1. - x;
        }
        if self.flip_y {
            y = 1. - y;
        }
        let (mut x, mut y) = match self.turns {
            1 => (y, 1. - x),
            2 => (1. - x, 1. - y),
            3 => (1. - y, x),
            _ => (x, y),
        };
        if let Some(h) = &self.transform {
            let w = h[2][0] * x + h[2][1] * y + h[2][2];
            // Points behind the virtual camera have no source; send them off-image.
            let w = if w > 1e-6 { w } else { 1e-6 };
            (x, y) = (
                (h[0][0] * x + h[0][1] * y + h[0][2]) / w,
                (h[1][0] * x + h[1][1] * y + h[1][2]) / w,
            );
        }
        if let Some(m) = &self.manual {
            [x, y] = m.source(x, y);
        }
        [
            (self.inset[0] + x * self.inset[2]) * self.source_width as f32 - 0.5,
            (self.inset[1] + y * self.inset[3]) * self.source_height as f32 - 0.5,
        ]
    }
    /// Output position (0–1 over the view) of decoded sample coordinates (`x`, `y`):
    /// the inverse of [`Self::source`].
    pub fn view(&self, x: f32, y: f32) -> [f32; 2] {
        let x = ((x + 0.5) / self.source_width as f32 - self.inset[0]) / self.inset[2];
        let y = ((y + 0.5) / self.source_height as f32 - self.inset[1]) / self.inset[3];
        let [x, y] = self.manual.map_or([x, y], |m| m.output(x, y));
        let (x, y) = match &self.forward {
            Some(f) => {
                let w = f[2][0] * x + f[2][1] * y + f[2][2];
                let w = if w.abs() > 1e-6 { w } else { 1e-6 };
                (
                    (f[0][0] * x + f[0][1] * y + f[0][2]) / w,
                    (f[1][0] * x + f[1][1] * y + f[1][2]) / w,
                )
            }
            None => (x, y),
        };
        let [mut x, mut y] = super::image_space::turn((4 - self.turns) % 4, x, y);
        if self.flip_x {
            x = 1. - x;
        }
        if self.flip_y {
            y = 1. - y;
        }
        let big_x = (x - 0.5) * self.oriented_width;
        let big_y = (y - 0.5) * self.oriented_height;
        let (s, c) = self.angle.sin_cos();
        let x0 = c * big_x - s * big_y;
        let y0 = s * big_x + c * big_y;
        let xc = x0 * self.zoom / self.oriented_width + 0.5;
        let yc = y0 * self.zoom / self.oriented_height + 0.5;
        [
            (xc - self.crop[0]) / (self.crop[2] - self.crop[0]),
            (yc - self.crop[1]) / (self.crop[3] - self.crop[1]),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliders_show_along_the_displayed_axes_as_in_lightroom() {
        // Lightroom's Vertical −70 on a photo the camera turned 90° left (LibRaw flip
        // 5, three quarter turns) is stored as PerspectiveHorizontal +70.
        let stored = Transform {
            horizontal: 0.7,
            ..Default::default()
        };
        let shown = stored.displayed(display_axes(3, false, false));
        assert!(
            (shown.vertical + 0.7).abs() < 1e-6 && shown.horizontal.abs() < 1e-6,
            "{shown:?}"
        );
    }

    #[test]
    fn displayed_sliders_are_the_same_homography_on_the_displayed_photo() {
        let stored = Transform {
            vertical: 0.3,
            horizontal: -0.2,
            rotate: 4.,
            aspect: 0.4,
            scale: 1.1,
            offset_x: 0.2,
            offset_y: -0.1,
        };
        let (w, h) = (1., 2. / 3.);
        for turns in 0..4 {
            for (flip_x, flip_y) in [(false, false), (true, false), (false, true)] {
                let m = display_axes(turns, flip_x, flip_y);
                let shown = stored.displayed(m);
                assert_eq!(shown.recorded(m), stored);
                let swapped = m[0][0] == 0.;
                let (dw, dh) = if swapped { (h, w) } else { (w, h) };
                // Displayed centred coordinates to recorded ones.
                let to = [[m[0][0], m[0][1], 0.], [m[1][0], m[1][1], 0.], [0., 0., 1.]];
                let expected = mat(
                    crate::color_math::inverse(to),
                    mat(stored.inverse(w, h), to),
                );
                let got = shown.inverse(dw, dh);
                for i in 0..3 {
                    for j in 0..3 {
                        let (a, b) = (got[i][j] / got[2][2], expected[i][j] / expected[2][2]);
                        assert!(
                            (a - b).abs() < 1e-5,
                            "{turns} {flip_x} {flip_y}: {got:?} {expected:?}"
                        );
                    }
                }
            }
        }
    }
}
#[cfg(test)]
mod manual_distortion_tests {
    use super::*;
    fn photo() -> CameraImage {
        CameraImage {
            width: 300,
            height: 200,
            pixels: vec![[0.2; 3]; 300 * 200],
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
        }
    }
    fn distorted(amount: f32) -> Recipe {
        Recipe {
            lens_manual_distortion: amount,
            ..Default::default()
        }
    }
    /// Camera Raw 18.7 on the synthetic chart: corners stay, the centre is scaled by
    /// 1 + 0.4 × amount (positive) or 1 + 0.5 × amount (negative), and positive amounts
    /// bring white in at the edges' middles.
    #[test]
    fn manual_distortion_matches_camera_raws_radial_map() {
        let im = photo();
        let plain = Geometry::new(&im, &Recipe::default(), 0);
        for (amount, centre) in [(0.5, 1.2), (-0.5, 0.75), (1., 1.4), (-1., 0.5)] {
            let g = Geometry::new(&im, &distorted(amount), 0);
            for corner in [[0., 0.], [1., 0.], [0., 1.], [1., 1.]] {
                let (a, b) = (
                    g.source(corner[0], corner[1]),
                    plain.source(corner[0], corner[1]),
                );
                assert!(
                    (a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3,
                    "{a:?} {b:?}"
                );
            }
            // Near the centre the radius scales by the centre ratio.
            let [cx, _] = plain.source(0.5, 0.5);
            let [x, _] = g.source(0.51, 0.5);
            let [px, _] = plain.source(0.51, 0.5);
            let ratio = (x - cx) / (px - cx);
            assert!((ratio - centre).abs() < 2e-3, "{amount}: {ratio}");
            let [ex, ey] = g.source(0., 0.5);
            assert_eq!(g.outside(ex, ey), amount > 0., "{amount}: {ex}");
            for p in [[0.2, 0.3], [0.5, 0.5], [0.9, 0.1]] {
                let [x, y] = g.source(p[0], p[1]);
                let back = g.view(x, y);
                assert!((back[0] - p[0]).abs() < 1e-4 && (back[1] - p[1]).abs() < 1e-4);
            }
        }
        // Off at 0 and before engine 4.
        assert!(Geometry::new(&im, &distorted(0.), 0).manual.is_none());
        let old = Recipe {
            engine: 3,
            ..distorted(0.5)
        };
        assert!(Geometry::new(&im, &old, 0).manual.is_none());
    }
    /// It applies in the frame as recorded, after the Transform takes an output position
    /// back to that frame, as Camera Raw's renders with Scale, Offset and Vertical show.
    #[test]
    fn manual_distortion_applies_after_the_transform_towards_the_source() {
        let im = photo();
        let mut transformed = Recipe::default();
        transformed.transform.scale = 0.8;
        transformed.transform.offset_x = 0.2;
        transformed.transform.vertical = 0.3;
        let both = Recipe {
            lens_manual_distortion: 0.5,
            ..transformed.clone()
        };
        let (t, g) = (
            Geometry::new(&im, &transformed, 0),
            Geometry::new(&im, &both, 0),
        );
        let manual = ManualDistortion::new(0.5, 300., 200.).unwrap();
        for p in [[0.2, 0.3], [0.6, 0.5], [0.9, 0.8]] {
            let [x, y] = t.source(p[0], p[1]);
            let [mx, my] = manual.source((x + 0.5) / 300., (y + 0.5) / 200.);
            let got = g.source(p[0], p[1]);
            assert!(
                (got[0] - (mx * 300. - 0.5)).abs() < 1e-3
                    && (got[1] - (my * 200. - 0.5)).abs() < 1e-3,
                "{got:?}"
            );
        }
    }
}
#[cfg(test)]
mod upright_tests {
    use super::*;
    #[test]
    fn clearing_the_analysis_turns_guided_off() {
        let mut u = Upright {
            mode: UprightMode::Guided,
            corrections: vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 6],
            ..Default::default()
        };
        u.clear_analysis();
        assert_eq!(u.mode, UprightMode::Off);
        u.mode = UprightMode::Vertical;
        u.clear_analysis();
        assert_eq!(u.mode, UprightMode::Vertical);
    }
}
#[cfg(test)]
mod singular_tests {
    use super::*;
    #[test]
    fn singular_upright_corrections_are_invalid() {
        let mut u = Upright {
            mode: UprightMode::Level,
            corrections: vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 4],
            ..Default::default()
        };
        assert!(u.validate());
        u.corrections[3] = [1., 0., 0., 0., 0., 0., 0., 0., 1.];
        assert!(!u.validate());
    }
}
