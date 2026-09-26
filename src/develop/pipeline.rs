use super::{Geometry, Recipe, Rendered, mul, srgb_encode};
use crate::color_math::srgb_decode;
use crate::{
    develop::curve::CurveLut,
    raw::{CameraImage, Metadata},
};
use anyhow::{Result, ensure};
use rayon::prelude::*;
fn srgb_to_lab(p: [f32; 3]) -> [f32; 3] {
    let a = mul(
        [
            [0.41222146, 0.53633255, 0.051445995],
            [0.2119035, 0.6806995, 0.10739696],
            [0.08830246, 0.28171885, 0.6299787],
        ],
        p,
    )
    .map(f32::cbrt);
    mul(
        [
            [0.21045426, 0.7936178, -0.004072047],
            [1.9779985, -2.4285922, 0.4505937],
            [0.025904037, 0.78277177, -0.80867577],
        ],
        a,
    )
}
fn lab_to_srgb(p: [f32; 3]) -> [f32; 3] {
    let a = mul(
        [
            [1., 0.39633778, 0.21580376],
            [1., -0.105561346, -0.06385417],
            [1., -0.08948418, -1.2914855],
        ],
        p,
    )
    .map(|v| v * v * v);
    mul(
        [
            [4.0767417, -3.3077116, 0.23096994],
            [-1.268438, 2.6097574, -0.3413194],
            [-0.0041960863, -0.7034186, 1.7076147],
        ],
        a,
    )
}
const TO_2020: [[f32; 3]; 3] = [
    [0.627404, 0.329283, 0.043313],
    [0.069097, 0.91954, 0.011362],
    [0.016391, 0.088013, 0.895595],
];
const FROM_2020: [[f32; 3]; 3] = [
    [1.660491, -0.587641, -0.07285],
    [-0.12455, 1.1329, -0.008349],
    [-0.018151, -0.100579, 1.11873],
];
fn luma(p: [f32; 3]) -> f32 {
    0.2627 * p[0] + 0.678 * p[1] + 0.0593 * p[2]
}
fn hue_weights(hue: f32) -> [f32; 8] {
    // Centers correspond to red, orange, yellow, green, cyan, blue, purple, magenta in Oklab.
    const CENTERS: [f32; 8] = [0.081, 0.151, 0.305, 0.395, 0.541, 0.733, 0.815, 0.912];
    let mut weights = [0.; 8];
    let hue = hue.rem_euclid(1.);
    for i in 0..8 {
        let left = CENTERS[i];
        let right = if i == 7 {
            CENTERS[0] + 1.
        } else {
            CENTERS[i + 1]
        };
        let h = if hue < left { hue + 1. } else { hue };
        if h >= left && h <= right {
            let t = (h - left) / (right - left);
            weights[i] = 1. - t;
            weights[(i + 1) % 8] = t;
            break;
        }
    }
    weights
}
pub(crate) fn profile_matrix(m: &Metadata, r: &Recipe) -> [[f32; 3]; 3] {
    r.profile
        .as_ref()
        .filter(|_| r.engine >= 3)
        .map_or(m.matrix, |p| p.camera_matrix(r.temperature))
}
fn process_pixel(
    p: [f32; 3],
    m: &Metadata,
    r: &Recipe,
    lut: &CurveSet,
    matrix: [[f32; 3]; 3],
    pos: [f32; 2],
) -> [f32; 3] {
    let (rgb, clipped_chroma) = tone_stage(p, m, r, lut, matrix);
    let rgb = match &lut.local {
        Some(local) => {
            let gain = local.gain(pos[0], pos[1], rgb);
            rgb.map(|v| v * gain)
        }
        None => rgb,
    };
    color_stage(rgb, clipped_chroma, r, lut)
}
/// Camera sample to linear display RGB after the camera profile's tone curve, plus the
/// legacy clipped-highlight chroma factor.
fn tone_stage(
    p: [f32; 3],
    m: &Metadata,
    r: &Recipe,
    lut: &CurveSet,
    matrix: [[f32; 3]; 3],
) -> ([f32; 3], f32) {
    let sensor_peak = (0..3)
        .map(|c| p[c] / m.wb[c].max(0.001))
        .fold(0f32, f32::max);
    let clipped_chroma = if r.engine < 3 {
        1. - ((sensor_peak - 0.94) / 0.06).clamp(0., 1.)
    } else {
        1.
    };
    let p = std::array::from_fn(|c| p[c] * r.wb[c]);
    let color = r.profile.as_ref().filter(|_| r.engine >= 3).map_or_else(
        || mul(matrix, p),
        |profile| profile.camera_color(p, matrix, r.temperature),
    );
    let color = if r.engine >= 3 && r.reference_calibration {
        lut.calibration.apply(color)
    } else if r.engine >= 3 {
        r.effects.calibrate(color)
    } else {
        color
    };
    let mut rgb = mul(TO_2020, color).map(|v| v * lut.exposure_gain);
    if let Some(ramp) = &lut.black_ramp {
        rgb = rgb.map(|v| ramp.eval(v));
    }
    // Engine 4 renders Dehaze as a measured curve in `apply_reference_curves`.
    if r.effects.dehaze != 0. && !lut.basic_curves {
        let a = r.effects.dehaze;
        rgb = rgb.map(|v| {
            if a > 0. {
                (v - a * 0.02) / (1. - a * 0.6)
            } else {
                v * (1. + a * 0.3) - a * 0.03
            }
        });
    }
    let y = luma(rgb).max(1e-8);
    let shadow = (-y * 6.).exp();
    let high = y / (y + 0.5);
    // Engine 4 renders Whites and Blacks as measured curves in `apply_reference_curves`.
    // Shadows and Highlights use the local operator in `local_tone.rs`.
    let (whites, blacks, shadows, highlights) = if lut.basic_curves {
        (0., 0., 0., 0.)
    } else {
        (r.whites, r.blacks, r.shadows, r.highlights)
    };
    let ev = shadows * shadow * 2.
        + highlights * high * 2.
        + whites * high.powi(3)
        + blacks * shadow.powi(3);
    let shaped = y * 2f32.powf(ev);
    let mapped = if r.engine < 3 {
        shaped * (2.2 * shaped + 0.05) / (shaped * (2.2 * shaped + 0.6) + 0.1)
    } else if r.profile_tone && r.profile.is_some() {
        shaped
    } else {
        // Scene-referred shoulder anchored at 18% middle gray. No per-channel clipping.
        let x = shaped.max(0.);
        x / (x + 0.82)
    };
    rgb = rgb.map(|v| v * mapped / y);
    let rgb = mul(FROM_2020, rgb);
    let rgb = if r.engine >= 3 {
        r.profile
            .as_ref()
            .map_or(rgb, |p| p.finish(rgb, r.profile_tone))
    } else {
        rgb
    };
    (rgb, clipped_chroma)
}
/// Basic curves, point curves, color controls and output encoding.
fn color_stage(rgb: [f32; 3], clipped_chroma: f32, r: &Recipe, lut: &CurveSet) -> [f32; 3] {
    let rgb = if r.reference_curves {
        apply_reference_curves(rgb, r, lut)
    } else if r.wide_gamut_curves {
        let p =
            mul(crate::camera_profiles::RGB_TO_PRO, rgb).map(|v| v.clamp(0., 1.).powf(1. / 2.2));
        let p = std::array::from_fn(|c| apply_curve(p[c], c, r, lut).powf(2.2));
        mul(crate::camera_profiles::PRO_TO_RGB, p)
    } else {
        rgb
    };
    // Lightroom grades after the tone curves: a faded point curve changes which tones
    // count as shadows.
    // Engine 4: the measured color mixer replaces the Oklab HSL/Saturation/Vibrance below.
    // Applied after the tone curves, which matches Lightroom references with point curves.
    let rgb = lut.mixer.as_ref().map_or(rgb, |m| m.apply(rgb));
    let rgb = lut.grade.as_ref().map_or(rgb, |g| g.apply(rgb));
    let mut lab = srgb_to_lab(rgb);
    lab[1] *= clipped_chroma;
    lab[2] *= clipped_chroma;
    if lut.color_adjustments {
        let chroma = lab[1].hypot(lab[2]);
        let hue = lab[2].atan2(lab[1]).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        let mut delta = [0.; 3];
        let weights = hue_weights(hue);
        let (hsl, saturation, vibrance) = if lut.basic_curves {
            ([[0.; 3]; 8], 0., 0.)
        } else {
            (r.hsl, r.saturation, r.vibrance)
        };
        for (band, weight) in hsl.iter().zip(weights) {
            for c in 0..3 {
                delta[c] += band[c] * weight;
            }
        }
        delta[2] *= (chroma / 0.04).clamp(0., 1.);
        let angle = (hue + delta[0] / 8.) * std::f32::consts::TAU;
        let vibrance = if r.reference_color {
            crate::develop::color::vibrance_gain(hue, chroma, vibrance)
        } else {
            1. + vibrance * (1. - (chroma / 0.3).clamp(0., 1.))
        };
        let sat = (1. + saturation) * vibrance * (1. + delta[1]);
        let luminance_response = if r.reference_color {
            0.5 * lab[0].clamp(0., 1.) * (1. - lab[0].clamp(0., 1.))
        } else {
            0.15
        };
        lab[0] = (lab[0] + delta[2] * luminance_response).clamp(0., 1.);
        lab[1] = angle.cos() * chroma * sat;
        lab[2] = angle.sin() * chroma * sat;
        lab = r.effects.defringe_color(lab, hue);
        if r.effects.monochrome {
            let shift: f32 = r
                .effects
                .gray_mix
                .iter()
                .zip(weights)
                .map(|(v, w)| v * w)
                .sum();
            lab[0] = (lab[0] + shift * 0.25).clamp(0., 1.);
            lab[1] = 0.;
            lab[2] = 0.;
        }
    } else {
        // Identity color controls need no hue angle, trigonometry or band weights.
        lab[0] = lab[0].clamp(0., 1.);
    }
    let rgb = if r.reference_color {
        let rgb = if lut.grade.is_some() {
            lab_to_srgb(lab)
        } else {
            crate::develop::color::grade(lab_to_srgb(lab), r)
        };
        lab = srgb_to_lab(rgb);
        rgb
    } else {
        let grade_l = (lab[0] + r.effects.balance * 0.35).clamp(0., 1.);
        let mut weights = [
            (1. - grade_l).powi(2),
            2. * grade_l * (1. - grade_l),
            grade_l.powi(2),
        ];
        if r.effects.blending != 0.5 {
            let power = 2f32.powf((0.5 - r.effects.blending) * 2.);
            weights = weights.map(|w| w.powf(power));
            let total: f32 = weights.iter().sum();
            weights = weights.map(|w| w / total.max(1e-6));
        }
        for (g, w) in r.grading.iter().zip(weights) {
            let a = g[0] * std::f32::consts::TAU;
            lab[1] += a.cos() * g[1] * w * 0.12;
            lab[2] += a.sin() * g[1] * w * 0.12;
            lab[0] += g[2] * w * 0.1;
        }
        let g = r.effects.global_grade;
        let a = g[0] * std::f32::consts::TAU;
        lab[1] += a.cos() * g[1] * 0.12;
        lab[2] += a.sin() * g[1] * 0.12;
        lab[0] += g[2] * 0.1;
        lab[0] = lab[0].clamp(0., 1.);
        lab_to_srgb(lab)
    };
    // Compress chroma toward neutral instead of clipping individual negative channels.
    let gray = lab[0].clamp(0., 1.).powi(3);
    let mut gamut = 1f32;
    for v in rgb {
        if v < 0. {
            gamut = gamut.min(gray / (gray - v).max(1e-8));
        }
        if v > 1. {
            gamut = gamut.min((1. - gray) / (v - gray).max(1e-8));
        }
    }
    std::array::from_fn(|c| {
        let v = rgb[c];
        let encoded = srgb_encode(gray + (v - gray) * gamut);
        if r.wide_gamut_curves || r.reference_curves {
            encoded.clamp(0., 1.)
        } else {
            apply_curve(encoded, c, r, lut)
        }
    })
}
struct CurveSet {
    exposure_gain: f32,
    /// Engine 4: Contrast, Whites and Blacks as measured Lightroom curves.
    basic_curves: bool,
    basic: Option<crate::develop::basic_tone::BasicTone>,
    /// Engine 4 Shadows/Highlights base level, built per image by `with_local`.
    local: Option<crate::develop::local_tone::LocalToneMap>,
    /// Engine 4 measured color mixer, Saturation and Vibrance.
    mixer: Option<crate::develop::color_mixer::ColorMixer>,
    /// Engine 4 measured color grading, when its settings are covered by the tables.
    grade: Option<crate::develop::color_grade::ColorGrade>,
    /// Engine 4: the DNG exposure ramp's black point (Adobe's default Shadows of 5).
    black_ramp: Option<ExposureRamp>,
    color_adjustments: bool,
    calibration: crate::develop::calibration::Calibration,
    master: CurveLut,
    channels: [CurveLut; 3],
}
impl CurveSet {
    /// Curves plus, for engine 4, the Shadows/Highlights map of this image.
    fn for_image(im: &CameraImage, r: &Recipe, matrix: [[f32; 3]; 3]) -> Self {
        let mut lut = Self::new(r);
        if lut.basic_curves {
            let local =
                crate::develop::local_tone::LocalToneMap::build(im, r.shadows, r.highlights, |p| {
                    tone_stage(p, &im.metadata, r, &lut, matrix).0
                });
            lut.local = local;
        }
        lut
    }
    fn new(r: &Recipe) -> Self {
        let basic_curves = r.engine >= 4 && r.reference_curves;
        Self {
            exposure_gain: 2f32.powf(r.exposure + r.camera_exposure),
            basic_curves,
            basic: basic_curves
                .then(|| {
                    crate::develop::basic_tone::BasicTone::new(
                        r.contrast,
                        r.whites,
                        r.blacks,
                        r.effects.dehaze,
                    )
                })
                .flatten(),
            local: None,
            mixer: basic_curves
                .then(|| crate::develop::color_mixer::ColorMixer::new(r))
                .flatten(),
            grade: (basic_curves && r.reference_color)
                .then(|| crate::develop::color_grade::ColorGrade::new(r))
                .flatten(),
            black_ramp: basic_curves.then(|| {
                ExposureRamp::new(DNG_SHADOWS_BLACK * 2f32.powf(r.exposure + r.camera_exposure))
            }),
            color_adjustments: r.vibrance != 0.
                || r.saturation != 0.
                || r.hsl != [[0.; 3]; 8]
                || r.effects.defringe != [0.; 2]
                || r.effects.monochrome,
            calibration: crate::develop::calibration::Calibration::new(
                r.effects.calibration,
                r.effects.shadow_tint,
            ),
            master: CurveLut::new(&r.curve),
            channels: std::array::from_fn(|c| CurveLut::new(&r.effects.channels[c])),
        }
    }
}
fn apply_reference_curves(rgb: [f32; 3], r: &Recipe, lut: &CurveSet) -> [f32; 3] {
    let p = mul(crate::camera_profiles::RGB_TO_PRO, rgb).map(|v| srgb_encode(v.clamp(0., 1.)));
    let p = lut.basic.as_ref().map_or(p, |b| b.apply(p));
    let contrast = if lut.basic_curves { 0. } else { r.contrast };
    let p = p.map(|v| {
        let x = ((v - r.black_point) / (r.white_point - r.black_point))
            .clamp(0., 1.)
            .powf(1. / r.midtone);
        let power = 2f32.powf(contrast);
        let low = x.powf(power);
        r.effects
            .parametric(low / (low + (1. - x).powf(power)).max(1e-8))
    });
    let lo = p.into_iter().fold(f32::INFINITY, f32::min);
    let hi = p.into_iter().fold(0f32, f32::max);
    let a = lut.master.evaluate(lo);
    let b = lut.master.evaluate(hi);
    let master = if hi - lo > 1e-8 {
        p.map(|v| a + (b - a) * (v - lo) / (hi - lo))
    } else {
        [a; 3]
    };
    let channels = std::array::from_fn(|c| srgb_decode(lut.channels[c].evaluate(master[c])));
    mul(crate::camera_profiles::PRO_TO_RGB, channels)
}

fn apply_curve(encoded: f32, c: usize, r: &Recipe, lut: &CurveSet) -> f32 {
    let level = ((encoded - r.black_point) / (r.white_point - r.black_point))
        .clamp(0., 1.)
        .powf(1. / r.midtone);
    let contrast = if r.wide_gamut_curves && r.contrast != 0. {
        // A bounded S-curve preserves black/white endpoints and avoids the
        // premature clipping of the legacy affine contrast adjustment.
        let power = 2f32.powf(r.contrast);
        let low = level.powf(power);
        low / (low + (1. - level).powf(power)).max(1e-8)
    } else {
        ((level - 0.5) * (1. + r.contrast) + 0.5).clamp(0., 1.)
    };
    let master = lut.master.evaluate(r.effects.parametric(contrast));
    let curve = &r.effects.channels[c];
    if curve.points == [[0., 0.], [1., 1.]] {
        master
    } else {
        lut.channels[c].evaluate(master)
    }
}

fn sample(im: &CameraImage, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0., (im.width - 1) as f32);
    let y = y.clamp(0., (im.height - 1) as f32);
    let ix = x as u32;
    let iy = y as u32;
    let fx = x - ix as f32;
    let fy = y - iy as f32;
    let at = |x: u32, y: u32| {
        im.pixels[(y.min(im.height - 1) * im.width + x.min(im.width - 1)) as usize]
    };
    let (a, b, c, d) = (
        at(ix, iy),
        at(ix + 1, iy),
        at(ix, iy + 1),
        at(ix + 1, iy + 1),
    );
    std::array::from_fn(|i| {
        (a[i] * (1. - fx) + b[i] * fx) * (1. - fy) + (c[i] * (1. - fx) + d[i] * fx) * fy
    })
}
pub fn neutral_pick(im: &CameraImage, r: &Recipe, u: f32, v: f32) -> [f32; 3] {
    let g = Geometry::new(im, r, 0);
    let [x, y] = g.source(u, v);
    let mut sum = [0.; 3];
    for dy in -2..=2 {
        for dx in -2..=2 {
            let p = sample(im, x + dx as f32, y + dy as f32);
            for c in 0..3 {
                sum[c] += p[c];
            }
        }
    }
    std::array::from_fn(|c| (sum[1] / sum[c].max(1e-6)).clamp(0.01, 100.))
}
pub fn preview(im: &CameraImage, max: u32) -> CameraImage {
    if im.width.max(im.height) <= max {
        return im.clone();
    }
    let scale = max as f32 / im.width.max(im.height) as f32;
    let w = (im.width as f32 * scale).round() as u32;
    let h = (im.height as f32 * scale).round() as u32;
    let mut pixels = vec![[0.; 3]; w as usize * h as usize];
    // Box integration keeps fine detail from aliasing while reducing the sensor image.
    pixels.par_iter_mut().enumerate().for_each(|(i, out)| {
        let x = i as u32 % w;
        let y = i as u32 / w;
        let x0 = x * im.width / w;
        let x1 = ((x + 1) * im.width / w).max(x0 + 1);
        let y0 = y * im.height / h;
        let y1 = ((y + 1) * im.height / h).max(y0 + 1);
        for yy in y0..y1 {
            for xx in x0..x1 {
                let p = im.pixels[(yy * im.width + xx) as usize];
                for c in 0..3 {
                    out[c] += p[c];
                }
            }
        }
        let n = ((x1 - x0) * (y1 - y0)) as f32;
        for v in out {
            *v /= n;
        }
    });
    CameraImage {
        recovered: Default::default(),
        width: w,
        height: h,
        pixels,
        metadata: im.metadata.clone(),
        fast: im.fast,
        scale_factor: im.scale_factor,
        scale_clipped: im.scale_clipped,
    }
}
fn detail_sample(im: &CameraImage, x: f32, y: f32, r: &Recipe) -> [f32; 3] {
    let p = sample(im, x, y);
    if r.noise_luma == 0. && r.noise_chroma == 0. {
        return p;
    }
    let center = (p[0] + 2. * p[1] + p[2]) / 4.;
    let mut sum = [0.; 3];
    let mut total = 0.;
    for dy in -1..=1 {
        for dx in -1..=1 {
            let q = sample(im, x + dx as f32, y + dy as f32);
            let lum = (q[0] + 2. * q[1] + q[2]) / 4.;
            let detail = (r.effects.luma_detail + r.effects.chroma_detail) * 0.5;
            let threshold = 0.0025 * 2f32.powf((0.5 - detail) * 4.);
            let w = 1. / (1. + (lum - center).powi(2) / threshold);
            for c in 0..3 {
                sum[c] += q[c] * w;
            }
            total += w;
        }
    }
    let avg = sum.map(|v| v / total);
    let avgl = (avg[0] + 2. * avg[1] + avg[2]) / 4.;
    std::array::from_fn(|c| {
        center
            + (avgl - center) * r.noise_luma * (1. - r.effects.luma_contrast * 0.5)
            + (p[c] - center) * (1. - r.noise_chroma)
            + (avg[c] - avgl) * r.noise_chroma * (0.5 + r.effects.chroma_smoothness)
    })
}
/// Black level of the DNG SDK's exposure ramp at its default Shadows setting of 5
/// (5 × 0.001, in scene-linear units before exposure).
const DNG_SHADOWS_BLACK: f32 = 0.0015;
/// dng_function_exposure_ramp with white at 1: values below `black` go to zero through
/// a quadratic toe, the rest are stretched back to full range.
struct ExposureRamp {
    black: f32,
    slope: f32,
    radius: f32,
    q: f32,
}
impl ExposureRamp {
    fn new(black: f32) -> Self {
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
    fn eval(&self, x: f32) -> f32 {
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
/// Built-in vignetting over the camera image. Radius 1 is the half diagonal; `x`, `y`
/// use sample coordinates, where pixel `i` is centred at `i`.
pub(crate) struct VignetteField<'a> {
    lens: &'a crate::lens::LensCorrection,
    center: [f32; 2],
    half: f32,
    /// Lightroom's profile Vignetting amount (1 = 100%).
    amount: f32,
}
impl<'a> VignetteField<'a> {
    pub(crate) fn new(im: &'a CameraImage, r: &Recipe) -> Option<Self> {
        let lens = r
            .lens_correction(&im.metadata)
            .filter(|l| l.vignetting.is_some())?;
        let (w, h) = (im.width as f32, im.height as f32);
        Some(Self {
            lens,
            center: [w * 0.5, h * 0.5],
            half: (w * w + h * h).sqrt() * 0.5,
            amount: r.lens_vignetting,
        })
    }
    pub(crate) fn gain(&self, x: f32, y: f32) -> f32 {
        let dx = x + 0.5 - self.center[0];
        let dy = y + 0.5 - self.center[1];
        self.lens
            .vignetting_gain((dx * dx + dy * dy).sqrt() / self.half)
            .powf(self.amount)
    }
}
/// Built-in lens correction applied while sampling the camera image, so no corrected
/// intermediate is stored: vignetting gain in linear camera space, then distortion and
/// lateral chromatic aberration as per-channel radial remapping.
struct LensWarp<'a> {
    lens: &'a crate::lens::LensCorrection,
    center: [f32; 2],
    half: f32,
    fill: f32,
    /// Lightroom's profile Distortion amount (1 = 100%).
    amount: f32,
    vignetting: Option<VignetteField<'a>>,
}
impl<'a> LensWarp<'a> {
    fn new(im: &'a CameraImage, r: &Recipe) -> Option<Self> {
        let lens = r.lens_correction(&im.metadata)?;
        let (w, h) = (im.width as f32, im.height as f32);
        Some(Self {
            lens,
            center: [w * 0.5, h * 0.5],
            half: (w * w + h * h).sqrt() * 0.5,
            fill: lens.fill_scale_with(r.lens_distortion),
            amount: r.lens_distortion,
            vignetting: VignetteField::new(im, r),
        })
    }
    fn sample(&self, im: &CameraImage, x: f32, y: f32, r: &Recipe) -> [f32; 3] {
        let dx = (x + 0.5 - self.center[0]) * self.fill;
        let dy = (y + 0.5 - self.center[1]) * self.fill;
        let scale = self
            .lens
            .radial_scale_with((dx * dx + dy * dy).sqrt() / self.half, self.amount);
        let at = |c: usize| {
            [
                self.center[0] + dx * scale[c] - 0.5,
                self.center[1] + dy * scale[c] - 0.5,
            ]
        };
        let [gx, gy] = at(1);
        let p = if scale[0] == scale[1] && scale[2] == scale[1] {
            detail_sample(im, gx, gy, r)
        } else {
            std::array::from_fn(|c| {
                let [sx, sy] = at(c);
                detail_sample(im, sx, sy, r)[c]
            })
        };
        let gain = self.vignetting.as_ref().map_or(1., |v| v.gain(gx, gy));
        p.map(|v| v * gain)
    }
}
/// Shared full/preview renderer. Geometry is sampled in rows; no full-sized intermediate color image.
pub fn render(im: &CameraImage, r: &Recipe, max_edge: u32) -> Result<Rendered> {
    if r.engine < 3 {
        return render_legacy(im, r, max_edge);
    }
    crate::develop::quality::render(im, r, max_edge, None)
}
pub fn render_region(im: &CameraImage, r: &Recipe, region: [u32; 4]) -> Result<Rendered> {
    if r.engine < 3 {
        return render_region_legacy(im, r, region);
    }
    crate::develop::quality::render(im, r, 0, Some(region))
}
pub(crate) fn render_base(
    im: &CameraImage,
    r: &Recipe,
    region: [u32; 4],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Rendered> {
    let mut base = r.clone();
    base.sharpening = 0.;
    render_region_inner(im, &base, region, cancel)
}

pub fn render_legacy(im: &CameraImage, r: &Recipe, max_edge: u32) -> Result<Rendered> {
    r.validate()?;
    render_legacy_inner(im, &r.resolved(&im.metadata), max_edge)
}
fn render_legacy_inner(im: &CameraImage, r: &Recipe, max_edge: u32) -> Result<Rendered> {
    let matrix = profile_matrix(&im.metadata, r);
    let lut = CurveSet::for_image(im, r, matrix);
    let full_geometry = Geometry::new(im, r, 0);
    if max_edge > 0 && full_geometry.width.max(full_geometry.height) > max_edge {
        let mut base = r.clone();
        base.sharpening = 0.;
        let full = render_legacy_inner(im, &base, 0)?;
        let g = Geometry::new(im, r, max_edge);
        let mut pixels = vec![[0.; 3]; g.width as usize * g.height as usize];
        pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
            let x = i as u32 % g.width;
            let y = i as u32 / g.width;
            let x0 = x * full.width / g.width;
            let x1 = ((x + 1) * full.width / g.width).max(x0 + 1);
            let y0 = y * full.height / g.height;
            let y1 = ((y + 1) * full.height / g.height).max(y0 + 1);
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let q = full.pixels[(yy * full.width + xx) as usize];
                    for c in 0..3 {
                        p[c] += q[c];
                    }
                }
            }
            let count = ((x1 - x0) * (y1 - y0)) as f32;
            for v in p {
                *v /= count;
            }
        });
        sharpen(&mut pixels, g.width, g.height, r.sharpening);
        return Ok(Rendered {
            width: g.width,
            height: g.height,
            pixels,
        });
    }
    let g = full_geometry;
    let warp = LensWarp::new(im, r);
    let mut pixels = vec![[0.; 3]; g.width as usize * g.height as usize];
    pixels.par_iter_mut().enumerate().for_each(|(i, out)| {
        let x = i as u32 % g.width;
        let y = i as u32 / g.width;
        let [sx, sy] = g.source(
            (x as f32 + 0.5) / g.width as f32,
            (y as f32 + 0.5) / g.height as f32,
        );
        if g.outside(sx, sy) {
            *out = [1.; 3];
            return;
        }
        let p = match &warp {
            Some(w) => w.sample(im, sx, sy, r),
            None => detail_sample(im, sx, sy, r),
        };
        *out = process_pixel(p, &im.metadata, r, &lut, matrix, [sx, sy]);
    });
    sharpen(&mut pixels, g.width, g.height, r.sharpening);
    Ok(Rendered {
        width: g.width,
        height: g.height,
        pixels,
    })
}
fn sharpen(pixels: &mut Vec<[f32; 3]>, width: u32, height: u32, amount: f32) {
    if amount <= 0. {
        return;
    }
    let src = &*pixels;
    let mut dst = vec![[0.; 3]; pixels.len()];
    dst.par_iter_mut().enumerate().for_each(|(i, out)| {
        let x = i as u32 % width;
        let y = i as u32 / width;
        let mut avg = [0.; 3];
        let mut n = 0.;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let xx = (x as i64 + dx).clamp(0, width as i64 - 1) as u32;
                let yy = (y as i64 + dy).clamp(0, height as i64 - 1) as u32;
                let q = src[(yy * width + xx) as usize];
                for c in 0..3 {
                    avg[c] += q[c];
                }
                n += 1.;
            }
        }
        for c in 0..3 {
            out[c] = (src[i][c] + (src[i][c] - avg[c] / n) * amount).clamp(0., 1.);
        }
    });
    *pixels = dst;
}

/// Render a rectangle of the full output at one sample per output pixel.
pub fn render_region_legacy(im: &CameraImage, r: &Recipe, region: [u32; 4]) -> Result<Rendered> {
    render_region_inner(
        im,
        &r.with_profile_adjustments(),
        region,
        &std::sync::atomic::AtomicBool::new(false),
    )
}
fn render_region_inner(
    im: &CameraImage,
    r: &Recipe,
    region: [u32; 4],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Rendered> {
    r.validate()?;
    let g = Geometry::new(im, r, 0);
    let matrix = profile_matrix(&im.metadata, r);
    let lut = CurveSet::for_image(im, r, matrix);
    let [x0, y0, w, h] = region;
    ensure!(
        w > 0 && h > 0 && x0 + w <= g.width && y0 + h <= g.height,
        "Invalid viewport region"
    );
    let mut pixels = vec![[0.; 3]; w as usize * h as usize];
    let warp = LensWarp::new(im, r);
    let at = |x: u32, y: u32| {
        let [sx, sy] = g.source(
            (x as f32 + 0.5) / g.width as f32,
            (y as f32 + 0.5) / g.height as f32,
        );
        if g.outside(sx, sy) {
            return [1.; 3];
        }
        let p = match &warp {
            Some(w) => w.sample(im, sx, sy, r),
            None => detail_sample(im, sx, sy, r),
        };
        process_pixel(p, &im.metadata, r, &lut, matrix, [sx, sy])
    };
    pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let x = x0 + i as u32 % w;
        let y = y0 + i as u32 / w;
        *p = at(x, y);
        if r.sharpening > 0. {
            let mut avg = [0.; 3];
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let q = at(
                        (x as i64 + dx).clamp(0, g.width as i64 - 1) as u32,
                        (y as i64 + dy).clamp(0, g.height as i64 - 1) as u32,
                    );
                    for c in 0..3 {
                        avg[c] += q[c] / 9.;
                    }
                }
            }
            for c in 0..3 {
                p[c] = (p[c] + (p[c] - avg[c]) * r.sharpening).clamp(0., 1.);
            }
        }
    });
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Render superseded"
    );
    Ok(Rendered {
        width: w,
        height: h,
        pixels,
    })
}

#[cfg(test)]
mod tests;
