//! Fields over the image the geometry stage evaluates: the exposure ramp, vignetting and the lens warp.
use super::*;
use crate::develop::recipe::RenderedRecipe;

/// Black level of the DNG SDK's exposure ramp at its default Shadows setting of 5
/// (5 × 0.001, in scene-linear units before exposure).
const DNG_SHADOWS_BLACK: f32 = 0.0015;
/// The ramp's black before exposure: none under a profile whose DefaultBlackRender
/// is None, as Camera Raw renders it.
pub(super) fn default_black(r: &Recipe) -> f32 {
    match r.profile.as_ref().map(|p| p.black_render()) {
        Some(crate::camera_profiles::BlackRender::None) => 0.,
        _ => DNG_SHADOWS_BLACK,
    }
}
/// dng_function_exposure_ramp with white at 1: values below `black` go to zero through
/// a quadratic toe, the rest are stretched back to full range.
pub(super) struct ExposureRamp {
    pub(super) black: f32,
    pub(super) slope: f32,
    pub(super) radius: f32,
    pub(super) q: f32,
}
impl ExposureRamp {
    pub(super) fn new(black: f32) -> Self {
        let black = black.clamp(0., 0.5);
        let slope = 1. / (1. - black);
        let radius = (0.5 * black).min(1. / 16. / slope);
        Self {
            black,
            slope,
            radius,
            q: if radius > 0. {
                slope / (4. * radius)
            } else {
                0.
            },
        }
    }
    pub(super) fn eval(&self, x: f32) -> f32 {
        if x <= self.black - self.radius {
            0.
        } else if x >= self.black + self.radius {
            (x - self.black) * self.slope
        } else {
            let y = x - (self.black - self.radius);
            self.q * y * y
        }
    }
}
/// Vignetting over the camera image: the lens profile's at its Vignetting amount, and
/// measured manual Vignetting. Radius 1 is the half diagonal; `x`, `y` use sample
/// coordinates, where pixel `i` is centred at `i`.
pub(crate) struct VignetteField<'a> {
    table: VignetteTable<'a>,
    pub(super) center: [f32; 2],
    pub(super) half: f32,
    /// The power the table is raised to: Lightroom's profile Vignetting amount (1 =
    /// 100%), or 1 for a combined table, which holds it already.
    pub(super) amount: f32,
}
enum VignetteTable<'a> {
    Lens(&'a crate::optics::Radial),
    Combined(crate::optics::Radial),
}
impl<'a> VignetteField<'a> {
    pub(crate) fn new(im: &'a CameraImage, r: &Recipe) -> Option<Self> {
        let lens = r
            .lens_correction(&im.metadata)
            .and_then(|l| l.vignetting.as_ref());
        let (w, h) = (im.width as f32, im.height as f32);
        let half = (w * w + h * h).sqrt() * 0.5;
        let (table, amount) = match (lens, r.manual_vignette()) {
            (_, Some(manual)) => {
                // Manual Vignetting spans the photo frame, inside the camera's default crop.
                let inset = crate::model::image_frame::ImageFrame::new(im).inset;
                let frame = (w * inset[2]).hypot(h * inset[3]) * 0.5;
                (
                    VignetteTable::Combined(crate::develop::effects::combined_table(
                        lens,
                        r.lens_vignetting,
                        &manual,
                        half / frame,
                    )),
                    1.,
                )
            }
            (Some(lens), None) => (VignetteTable::Lens(lens), r.lens_vignetting),
            (None, None) => return None,
        };
        Some(Self {
            table,
            center: [w * 0.5, h * 0.5],
            half,
            amount,
        })
    }
    pub(super) fn table(&self) -> &crate::optics::Radial {
        match &self.table {
            VignetteTable::Lens(t) => t,
            VignetteTable::Combined(t) => t,
        }
    }
    pub(crate) fn gain(&self, x: f32, y: f32) -> f32 {
        let dx = x + 0.5 - self.center[0];
        let dy = y + 0.5 - self.center[1];
        self.table()
            .eval((dx * dx + dy * dy).sqrt() / self.half)
            .powf(self.amount)
    }
}
/// Built-in lens correction applied while sampling the camera image, so no corrected
/// intermediate is stored: vignetting gain in linear camera space, then distortion and
/// lateral chromatic aberration as per-channel radial remapping.
pub(super) struct LensWarp<'a> {
    pub(super) map: crate::develop::image_space::LensMap<'a>,
    pub(super) vignetting: Option<VignetteField<'a>>,
}
impl<'a> LensWarp<'a> {
    pub(super) fn new(im: &'a CameraImage, r: &Recipe) -> Option<Self> {
        Some(Self {
            map: crate::develop::image_space::LensMap::new(im, r)?,
            vignetting: VignetteField::new(im, r),
        })
    }
    /// Where `sample` reads red, green and blue for sample coordinates `x`, `y`.
    pub(super) fn positions(&self, x: f32, y: f32) -> [[f32; 2]; 3] {
        let m = &self.map;
        let ([dx, dy], scale) = m.scales(x, y);
        scale.map(|k| [m.center[0] + dx * k - 0.5, m.center[1] + dy * k - 0.5])
    }
    pub(super) fn sample(&self, im: Source, x: f32, y: f32, r: &Recipe, spread: f32) -> [f32; 3] {
        let m = &self.map;
        let ([dx, dy], scale) = m.scales(x, y);
        let at = |c: usize| {
            [
                m.center[0] + dx * scale[c] - 0.5,
                m.center[1] + dy * scale[c] - 0.5,
            ]
        };
        let [gx, gy] = at(1);
        let p = if scale[0] == scale[1] && scale[2] == scale[1] {
            footprint_sample(im, gx, gy, r, spread)
        } else {
            std::array::from_fn(|c| {
                let [sx, sy] = at(c);
                footprint_sample(im, sx, sy, r, spread)[c]
            })
        };
        let gain = self.vignetting.as_ref().map_or(1., |v| v.gain(gx, gy));
        p.map(|v| v * gain)
    }
}
