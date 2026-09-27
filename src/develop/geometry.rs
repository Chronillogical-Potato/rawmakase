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
impl Transform {
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
    /// of the long edge. The forward (source-to-output) matrices were fitted to Camera
    /// Raw 18.6 renders of each slider (docs/transform.md): Vertical is
    /// [[1, 0, 0], [0, k, 0], [0, -v, 1]] and Horizontal its transpose counterpart,
    /// with k(s) = 1 + 0.347 s² + 0.334 s⁴; Aspect scales y by 2^(0.137 a) and x by the
    /// inverse; offsets move by 0.811 of the image size, positive Y upward.
    fn inverse(&self, width: f32, height: f32) -> [[f32; 3]; 3] {
        let k = |s: f32| 1. + 0.347 * s * s + 0.334 * s.powi(4);
        let vertical = [
            [1., 0., 0.],
            [0., k(self.vertical), 0.],
            [0., -self.vertical, 1.],
        ];
        let horizontal = [
            [k(self.horizontal), 0., 0.],
            [0., 1., 0.],
            [-self.horizontal, 0., 1.],
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
        let forward = mat(offset, mat(scale, mat(rotate, mat(horizontal, vertical))));
        crate::color_math::inverse(forward)
    }
}
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
    /// Output-to-source Transform homography, when not the identity.
    transform: Option<[[f32; 3]; 3]>,
    /// Its inverse, source to output.
    forward: Option<[[f32; 3]; 3]>,
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
        let transform = (r.engine >= 4 && !r.transform.is_identity())
            .then(|| r.transform.inverse(ow / ow.max(oh), oh / ow.max(oh)));
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
        }
    }
    /// Whether a transformed output position has no source pixel; Lightroom shows
    /// white there.
    pub fn outside(&self, x: f32, y: f32) -> bool {
        self.transform.is_some()
            && (x < -0.5
                || y < -0.5
                || x > self.source_width as f32 - 0.5
                || y > self.source_height as f32 - 0.5)
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
    pub fn source(&self, u: f32, v: f32) -> [f32; 2] {
        let mut x = self.crop[0] + u * (self.crop[2] - self.crop[0]);
        let mut y = self.crop[1] + v * (self.crop[3] - self.crop[1]);
        x = (x - 0.5) * self.oriented_width / self.zoom;
        y = (y - 0.5) * self.oriented_height / self.zoom;
        let (s, c) = self.angle.sin_cos();
        let nx = (c * x + s * y) / self.oriented_width + 0.5;
        let ny = (-s * x + c * y) / self.oriented_height + 0.5;
        let (mut x, mut y) = (nx, ny);
        if let Some(h) = &self.transform {
            let long = self.oriented_width.max(self.oriented_height);
            let px = (x - 0.5) * self.oriented_width / long;
            let py = (y - 0.5) * self.oriented_height / long;
            let w = h[2][0] * px + h[2][1] * py + h[2][2];
            // Points behind the virtual camera have no source; send them off-image.
            let w = if w > 1e-6 { w } else { 1e-6 };
            x = (h[0][0] * px + h[0][1] * py + h[0][2]) / w * long / self.oriented_width + 0.5;
            y = (h[1][0] * px + h[1][1] * py + h[1][2]) / w * long / self.oriented_height + 0.5;
        }
        if self.flip_x {
            x = 1. - x;
        }
        if self.flip_y {
            y = 1. - y;
        }
        let (x, y) = match self.turns {
            1 => (y, 1. - x),
            2 => (1. - x, 1. - y),
            3 => (1. - y, x),
            _ => (x, y),
        };
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
        let [mut x, mut y] = super::image_space::turn((4 - self.turns) % 4, x, y);
        if self.flip_x {
            x = 1. - x;
        }
        if self.flip_y {
            y = 1. - y;
        }
        if let Some(f) = &self.forward {
            let long = self.oriented_width.max(self.oriented_height);
            let px = (x - 0.5) * self.oriented_width / long;
            let py = (y - 0.5) * self.oriented_height / long;
            let w = f[2][0] * px + f[2][1] * py + f[2][2];
            let w = if w.abs() > 1e-6 { w } else { 1e-6 };
            x = (f[0][0] * px + f[0][1] * py + f[0][2]) / w * long / self.oriented_width + 0.5;
            y = (f[1][0] * px + f[1][1] * py + f[1][2]) / w * long / self.oriented_height + 0.5;
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
