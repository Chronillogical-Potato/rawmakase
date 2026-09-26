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
}
impl Geometry {
    pub fn new(im: &CameraImage, r: &Recipe, max_edge: u32) -> Self {
        let m = &im.metadata;
        let cw = if m.crop_width > 0 && m.crop_width <= m.width {
            m.crop_width as f32 / m.width as f32
        } else {
            1.
        };
        let ch = if m.crop_height > 0 && m.crop_height <= m.height {
            m.crop_height as f32 / m.height as f32
        } else {
            1.
        };
        let base_turn = match m.flip {
            3 => 2,
            5 => 3,
            6 => 1,
            _ => 0,
        };
        let turns = (base_turn + r.rotation) % 4;
        let (ow, oh) = if turns % 2 == 1 {
            (im.height as f32 * ch, im.width as f32 * cw)
        } else {
            (im.width as f32 * cw, im.height as f32 * ch)
        };
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
            inset: [
                if m.crop_left.saturating_add(m.crop_width) <= m.width {
                    m.crop_left as f32 / m.width as f32
                } else {
                    (1. - cw) / 2.
                },
                if m.crop_top.saturating_add(m.crop_height) <= m.height {
                    m.crop_top as f32 / m.height as f32
                } else {
                    (1. - ch) / 2.
                },
                cw,
                ch,
            ],
            transform: (r.engine >= 4 && !r.transform.is_identity())
                .then(|| r.transform.inverse(ow / ow.max(oh), oh / ow.max(oh))),
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
}
