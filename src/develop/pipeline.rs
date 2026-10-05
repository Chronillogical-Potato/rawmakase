use super::masks::{
    LocalDelta, LocalMath, MaskWeights,
    local::{self, slot},
};
use super::{Geometry, Recipe, Rendered, mul, srgb_encode};
use crate::color_math::srgb_decode;
use crate::{
    develop::curve::{CurveLut, refine_saturation},
    raw::{CameraImage, Metadata},
};
use anyhow::{Result, ensure};
use rayon::prelude::*;
use std::sync::Arc;
pub(crate) fn srgb_to_lab(p: [f32; 3]) -> [f32; 3] {
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
/// The black & white mix's change to Oklab lightness, for the mix's hue-weighted
/// sum of slider values (-1..=1) at a color of this Oklab chroma. Fitted to Camera
/// Raw 18.7 renders of the synthetic chart: neutrals stay as they are, the change
/// grows with chroma, and darkening is stronger than brightening.
pub(crate) fn gray_mix_shift(mix: f32, chroma: f32) -> f32 {
    mix * chroma
        * if mix > 0. {
            GRAY_MIX_BRIGHTEN
        } else {
            GRAY_MIX_DARKEN
        }
}
const GRAY_MIX_BRIGHTEN: f32 = 1.78;
const GRAY_MIX_DARKEN: f32 = 4.37;
pub(crate) fn hue_weights(hue: f32) -> [f32; 8] {
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
/// A pixel's mask adjustments and the render's constants for them.
#[derive(Clone, Copy)]
pub(crate) struct Local<'a> {
    pub(crate) delta: &'a LocalDelta,
    pub(crate) math: &'a LocalMath,
}
fn process_pixel(
    p: [f32; 3],
    m: &Metadata,
    r: &Recipe,
    lut: &CurveSet,
    matrix: [[f32; 3]; 3],
    pos: [f32; 2],
    local: Option<Local>,
) -> [f32; 3] {
    let (rgb, clipped_chroma) = tone_stage(p, m, r, lut, matrix, local);
    let rgb = match &lut.local {
        Some(map) => {
            let sliders = local
                .filter(|l| local::uses(l.delta, &[slot::SHADOWS, slot::HIGHLIGHTS]))
                .map(|l| {
                    [
                        r.shadows + l.delta[slot::SHADOWS],
                        r.highlights + l.delta[slot::HIGHLIGHTS],
                    ]
                });
            let gain = match sliders {
                Some(sliders) => map.gain_with(pos[0], pos[1], rgb, sliders),
                None => map.gain(pos[0], pos[1], rgb),
            };
            rgb.map(|v| v * gain)
        }
        None => rgb,
    };
    color_stage(rgb, clipped_chroma, r, lut, local.map(|l| l.delta))
}
/// Camera sample to linear display RGB after the camera profile's tone curve, plus the
/// legacy clipped-highlight chroma factor.
fn tone_stage(
    p: [f32; 3],
    m: &Metadata,
    r: &Recipe,
    lut: &CurveSet,
    matrix: [[f32; 3]; 3],
    local: Option<Local>,
) -> ([f32; 3], f32) {
    let sensor_peak = (0..3)
        .map(|c| p[c] / m.wb[c].max(0.001))
        .fold(0f32, f32::max);
    let clipped_chroma = if r.engine < 3 {
        1. - ((sensor_peak - 0.94) / 0.06).clamp(0., 1.)
    } else {
        1.
    };
    let wb = local.map_or([1.; 3], |l| l.math.white_balance_gain(l.delta));
    let p = std::array::from_fn(|c| p[c] * r.wb[c] * wb[c]);
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
    let exposure = local.map_or(0., |l| l.delta[slot::EXPOSURE]);
    let mut rgb = mul(TO_2020, color).map(|v| v * lut.exposure_gain * exposure.exp2());
    if let Some(l) = local {
        for (c, v) in rgb.iter_mut().enumerate() {
            *v *= l.delta[slot::COLOR + c].exp2();
        }
    }
    if let Some(ramp) = &lut.black_ramp {
        if exposure != 0. {
            // The ramp's black point follows exposure, as for the global slider.
            let ramp = ExposureRamp::new(
                DNG_SHADOWS_BLACK * (r.exposure + r.camera_exposure + exposure).exp2(),
            );
            rgb = rgb.map(|v| ramp.eval(v));
        } else {
            rgb = rgb.map(|v| ramp.eval(v));
        }
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
/// The tone curves, the color mixer and Point Color: linear display RGB as Point
/// Color leaves it, and Visualize Range's selection.
fn mixer_stage(
    rgb: [f32; 3],
    r: &Recipe,
    lut: &CurveSet,
    local: Option<&LocalDelta>,
) -> crate::develop::point_color::Rendered {
    let sampled = |color| crate::develop::point_color::Rendered {
        color,
        selection: None,
    };
    if lut.output == PixelOutput::CurveInput {
        let [red, green, blue] = curve_input(rgb, r, lut, local);
        return sampled([0.299 * red + 0.587 * green + 0.114 * blue; 3]);
    }
    let rgb = if r.reference_curves {
        apply_reference_curves(rgb, r, lut, local)
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
    if lut.output == PixelOutput::MixerInput {
        return sampled(mul(crate::camera_profiles::RGB_TO_PRO, rgb));
    }
    let rgb = lut.mixer.as_ref().map_or(rgb, |m| m.apply(rgb));
    // Point Color works where the mixer does, in HSV of linear ProPhoto RGB.
    match &lut.point_colors {
        Some(p) => {
            let out = p.render_prophoto(mul(crate::camera_profiles::RGB_TO_PRO, rgb));
            crate::develop::point_color::Rendered {
                color: mul(crate::camera_profiles::PRO_TO_RGB, out.color),
                selection: out.selection,
            }
        }
        None => crate::develop::point_color::Rendered {
            color: rgb,
            selection: None,
        },
    }
}
/// Basic curves, point curves, color controls and output encoding.
fn color_stage(
    rgb: [f32; 3],
    clipped_chroma: f32,
    r: &Recipe,
    lut: &CurveSet,
    local: Option<&LocalDelta>,
) -> [f32; 3] {
    let mixed = mixer_stage(rgb, r, lut, local);
    let rgb = mixed.color;
    match lut.output {
        PixelOutput::PointColor => return mul(crate::camera_profiles::RGB_TO_PRO, rgb),
        PixelOutput::CurveInput | PixelOutput::MixerInput => return rgb,
        PixelOutput::Display | PixelOutput::ColorInput => {}
    }
    // A look's RGB table: after the colour mixer, before colour grading, as Camera
    // Raw 18.7 applies it (also after the user's tone curves and Saturation). Before
    // engine 4 the colour controls come later, in Oklab, and the table after them.
    let rgb = match &lut.rgb_table {
        Some(t) if lut.basic_curves => t.apply(rgb),
        _ => rgb,
    };
    let rgb = lut.grade.as_ref().map_or(rgb, |g| g.apply(rgb));
    let mut lab = srgb_to_lab(rgb);
    if let Some(d) = local {
        lab = local::hue_saturation(d, lab);
    }
    lab[1] *= clipped_chroma;
    lab[2] *= clipped_chroma;
    if lut.output == PixelOutput::ColorInput {
        return lab;
    }
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
        lab = legacy_rgb_table(lab, lut);
        if r.effects.monochrome {
            let shift: f32 = r
                .effects
                .gray_mix
                .iter()
                .zip(weights)
                .map(|(v, w)| v * w)
                .sum();
            lab[0] = (lab[0] + gray_mix_shift(shift, chroma)).clamp(0., 1.);
            lab[1] = 0.;
            lab[2] = 0.;
        }
    } else {
        // Identity color controls need no hue angle, trigonometry or band weights.
        lab = legacy_rgb_table(lab, lut);
        lab[0] = lab[0].clamp(0., 1.);
    }
    let out = finish_color(lab, r, lut);
    // Visualize Range grays what the swatch leaves out after every color control, so
    // none of them tints it.
    mixed
        .selection
        .map_or(out, |w| crate::develop::point_color::visualize(out, w))
}
/// Before engine 4 the colour controls run in Oklab, after the place of the measured
/// mixer: a look's RGB table follows them there, before Monochrome. Engine 3's point
/// curves stay last, in encoded output, as that renderer has always applied them.
fn legacy_rgb_table(lab: [f32; 3], lut: &CurveSet) -> [f32; 3] {
    match &lut.rgb_table {
        Some(t) if !lut.basic_curves => srgb_to_lab(t.apply(lab_to_srgb(lab))),
        _ => lab,
    }
}
/// The colour stage after the colour controls and Defringe: legacy and table colour
/// grading, gamut compression and, before engine 4, the per-channel curves. Returns
/// encoded sRGB.
fn finish_color(mut lab: [f32; 3], r: &Recipe, lut: &CurveSet) -> [f32; 3] {
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

/// What the per-pixel stage hands back.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum PixelOutput {
    /// The finished, encoded color.
    #[default]
    Display,
    /// Linear ProPhoto RGB after the swatches already there: what Point Color's
    /// dropper samples.
    PointColor,
    /// The parametric curve's input, as the luma of the three channels it curves
    /// (Rec. 601 weights, as Refine Saturation's): what the Tone Curve's Targeted
    /// Adjustment Tool samples, in every channel.
    CurveInput,
    /// Linear ProPhoto RGB where the color mixer works, after the tone curves: what
    /// the Color Mixer's Targeted Adjustment Tool samples.
    MixerInput,
    /// Oklab where the color controls and the black & white mix take a color's hue
    /// to weigh their bands.
    ColorInput,
}
struct CurveSet {
    /// Where the stage stops.
    output: PixelOutput,
    exposure_gain: f32,
    /// Engine 4: Contrast, Whites and Blacks as measured Lightroom curves.
    basic_curves: bool,
    basic: Option<crate::develop::basic_tone::BasicTone>,
    /// Engine 4 Shadows/Highlights base level, built per image by `with_local`.
    local: Option<crate::develop::local_tone::LocalToneMap>,
    /// Engine 4 measured color mixer, Saturation and Vibrance.
    mixer: Option<crate::develop::color_mixer::ColorMixer>,
    /// Engine 4 Point Color swatches.
    point_colors: Option<crate::develop::point_color::PointColors>,
    /// Engine 4 measured color grading, when its settings are covered by the tables.
    grade: Option<crate::develop::color_grade::ColorGrade>,
    /// Engine 4: the DNG exposure ramp's black point (Adobe's default Shadows of 5).
    black_ramp: Option<ExposureRamp>,
    color_adjustments: bool,
    /// The profile look's RGB table, at the recipe's Profile Amount.
    rgb_table: Option<crate::camera_profiles::RgbLook>,
    calibration: crate::develop::calibration::Calibration,
    /// The Contrast curve of the global slider and the masks' (engine 4).
    contrast: crate::develop::basic_tone::ContrastCurve,
    /// Engine 4's measured parametric curve, when the recipe uses it and a region is set.
    parametric: Option<crate::develop::parametric::ParametricCurve>,
    master: CurveLut,
    channels: [CurveLut; 3],
}
impl CurveSet {
    /// Curves plus, for engine 4, the Shadows/Highlights map of this image; built
    /// also when `local_tone` (masks change Shadows or Highlights).
    fn for_image(im: Source, r: &Recipe, matrix: [[f32; 3]; 3], local_tone: bool) -> Self {
        let mut lut = Self::with_contrast_pivot(im, r, matrix);
        if lut.basic_curves {
            let local = crate::develop::local_tone::LocalToneMap::build(
                im,
                r.shadows,
                r.highlights,
                local_tone,
                |p| tone_stage(p, &im.metadata, r, &lut, matrix, None).0,
            );
            lut.local = local;
        }
        lut
    }
    /// Curves with Contrast at this image's pivot, when the recipe measures it.
    fn with_contrast_pivot(im: Source, r: &Recipe, matrix: [[f32; 3]; 3]) -> Self {
        let mut lut = Self::new(r);
        if measures_contrast_pivot(r) {
            lut.contrast =
                crate::develop::basic_tone::ContrastCurve::Pivot(contrast_pivot(im, r, matrix));
            lut.basic = crate::develop::basic_tone::BasicTone::new(
                r.contrast,
                r.whites,
                r.blacks,
                r.effects.dehaze,
                lut.contrast,
            );
        }
        lut
    }
    fn new(r: &Recipe) -> Self {
        let basic_curves = r.engine >= 4 && r.reference_curves;
        let contrast = match r.contrast_model {
            crate::develop::basic_tone::ContrastModel::Original => {
                crate::develop::basic_tone::ContrastCurve::Original
            }
            crate::develop::basic_tone::ContrastModel::Adaptive => {
                crate::develop::basic_tone::ContrastCurve::Pivot(
                    crate::develop::basic_tone::TYPICAL_PIVOT,
                )
            }
        };
        Self {
            output: PixelOutput::Display,
            exposure_gain: 2f32.powf(r.exposure + r.camera_exposure),
            basic_curves,
            basic: basic_curves
                .then(|| {
                    crate::develop::basic_tone::BasicTone::new(
                        r.contrast,
                        r.whites,
                        r.blacks,
                        r.effects.dehaze,
                        contrast,
                    )
                })
                .flatten(),
            local: None,
            contrast,
            mixer: basic_curves
                .then(|| crate::develop::color_mixer::ColorMixer::new(r))
                .flatten(),
            // Camera Raw leaves Point Color out of black & white renders.
            point_colors: (basic_curves && !r.effects.monochrome)
                .then(|| crate::develop::point_color::PointColors::new(&r.point_colors))
                .flatten(),
            grade: (basic_curves && r.reference_color)
                .then(|| crate::develop::color_grade::ColorGrade::new(r))
                .flatten(),
            black_ramp: basic_curves.then(|| {
                ExposureRamp::new(DNG_SHADOWS_BLACK * 2f32.powf(r.exposure + r.camera_exposure))
            }),
            rgb_table: r
                .profile
                .as_ref()
                .filter(|_| r.engine >= 3)
                .and_then(|p| p.enhanced.as_ref()?.rgb().cloned()),
            color_adjustments: r.vibrance != 0.
                || r.saturation != 0.
                || r.hsl != [[0.; 3]; 8]
                || r.effects.defringe != [0.; 2]
                || r.effects.monochrome,
            calibration: crate::develop::calibration::Calibration::new(
                r.effects.calibration,
                r.effects.shadow_tint,
            ),
            parametric: (basic_curves
                && r.parametric_model == crate::develop::parametric::ParametricModel::Measured)
                .then(|| {
                    crate::develop::parametric::ParametricCurve::new(
                        r.effects.parametric,
                        r.effects.splits,
                    )
                })
                .flatten(),
            master: CurveLut::new(&r.curve),
            channels: std::array::from_fn(|c| CurveLut::new(&r.effects.channels[c])),
        }
    }
}
/// Whether the recipe's Contrast pivots where the photo's own measure puts it.
pub(crate) fn measures_contrast_pivot(r: &Recipe) -> bool {
    r.engine >= 4
        && r.reference_curves
        && r.contrast_model == crate::develop::basic_tone::ContrastModel::Adaptive
        && (r.contrast != 0.
            || r.masks
                .iter()
                .any(|m| m.is_active() && m.adjust.contrast != 0.))
}
/// Camera Raw's Contrast pivot for this photo, from its reduced copy rendered as the
/// recipe's profile, white balance and calibration render it, at the camera's
/// exposure: the user's Exposure does not move it (measured on the chart).
fn contrast_pivot(im: Source, r: &Recipe, matrix: [[f32; 3]; 3]) -> f32 {
    // Clarity's and Texture's gain is a user adjustment, and depends on the preview
    // size: the photo is measured without it.
    let small = match (im.reduced, im.gain) {
        (Some(small), None) => std::borrow::Cow::Borrowed(small),
        _ => std::borrow::Cow::Owned(preview_source(
            Source::new(im.image, None),
            super::local_tone::MAP_EDGE,
        )),
    };
    let default = Recipe {
        exposure: 0.,
        ..r.clone()
    };
    let lut = CurveSet::new(&default);
    let encoded: Vec<[f32; 3]> = small
        .pixels
        .par_iter()
        .map(|p| {
            tone_stage(*p, &im.metadata, &default, &lut, matrix, None)
                .0
                .map(|v| srgb_encode(v.clamp(0., 1.)))
        })
        .collect();
    crate::develop::basic_tone::photo_pivot(&crate::develop::basic_tone::blocks(
        &encoded,
        small.width as usize,
        small.height as usize,
    ))
}
fn apply_reference_curves(
    rgb: [f32; 3],
    r: &Recipe,
    lut: &CurveSet,
    local: Option<&LocalDelta>,
) -> [f32; 3] {
    let p = curve_input(rgb, r, lut, local);
    let p = match &lut.parametric {
        Some(curve) => curve.apply(p),
        // The original curve, the identity when no region is set.
        None => p.map(|x| r.effects.parametric(x)),
    };
    let lo = p.into_iter().fold(f32::INFINITY, f32::min);
    let hi = p.into_iter().fold(0f32, f32::max);
    let a = lut.master.evaluate(lo);
    let b = lut.master.evaluate(hi);
    let master = if hi - lo > 1e-8 {
        p.map(|v| a + (b - a) * (v - lo) / (hi - lo))
    } else {
        [a; 3]
    };
    let master = refine_saturation(p, master, r.curve_saturation);
    let channels = std::array::from_fn(|c| srgb_decode(lut.channels[c].evaluate(master[c])));
    mul(crate::camera_profiles::PRO_TO_RGB, channels)
}

/// The parametric curve's input in each channel of encoded ProPhoto RGB: the basic
/// tone curves, a mask's tone, Levels and (before engine 4) Contrast applied.
fn curve_input(rgb: [f32; 3], r: &Recipe, lut: &CurveSet, local: Option<&LocalDelta>) -> [f32; 3] {
    let p = mul(crate::camera_profiles::RGB_TO_PRO, rgb).map(|v| srgb_encode(v.clamp(0., 1.)));
    let p = lut.basic.as_ref().map_or(p, |b| b.apply(p));
    let p = match local {
        Some(d) if local::uses(d, &local::TONE_SLOTS) => local::tone(d, p, lut.contrast),
        _ => p,
    };
    let contrast = if lut.basic_curves { 0. } else { r.contrast };
    p.map(|v| {
        let x = ((v - r.black_point) / (r.white_point - r.black_point))
            .clamp(0., 1.)
            .powf(1. / r.midtone);
        let power = 2f32.powf(contrast);
        let low = x.powf(power);
        low / (low + (1. - x).powf(power)).max(1e-8)
    })
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

/// Camera pixels as the pipeline samples them: the image, times the per-pixel local-tone
/// gain when Clarity, Texture or (before engine 4) Shadows and Highlights are active.
#[derive(Clone, Copy)]
pub(crate) struct Source<'a> {
    image: &'a CameraImage,
    gain: Option<&'a [f32]>,
    /// These pixels reduced for the Shadows/Highlights map, when already made.
    pub(crate) reduced: Option<&'a CameraImage>,
}
impl<'a> Source<'a> {
    pub(crate) fn new(image: &'a CameraImage, gain: Option<&'a [f32]>) -> Self {
        Self {
            image,
            gain,
            reduced: None,
        }
    }
    fn px(&self, i: usize) -> [f32; 3] {
        let p = self.image.pixels[i];
        match self.gain {
            Some(gain) => p.map(|v| v * gain[i]),
            None => p,
        }
    }
}
impl std::ops::Deref for Source<'_> {
    type Target = CameraImage;
    fn deref(&self) -> &CameraImage {
        self.image
    }
}
impl<'a> From<&'a CameraImage> for Source<'a> {
    fn from(image: &'a CameraImage) -> Self {
        Self::new(image, None)
    }
}
/// A camera image and its local-tone gain, as the pixel stages take them.
pub(crate) struct Toned {
    pub(crate) image: std::sync::Arc<CameraImage>,
    /// The image's size relative to the full-resolution photo.
    pub(crate) scale: f32,
    pub(crate) gain: Option<std::sync::Arc<Vec<f32>>>,
    /// What the gain was computed from, when it came from the stage cache.
    pub(crate) gain_key: Option<super::stage_cache::LocalKey>,
    /// The toned image reduced for the Shadows/Highlights map, kept in the stage cache
    /// so edits do not reduce the full-resolution image again.
    pub(crate) reduced: Option<std::sync::Arc<CameraImage>>,
}
impl Toned {
    pub(crate) fn source(&self) -> Source<'_> {
        Source {
            reduced: self.reduced.as_deref(),
            ..Source::new(&self.image, self.gain.as_deref().map(Vec::as_slice))
        }
    }
}
fn sample(im: Source, x: f32, y: f32) -> [f32; 3] {
    let x = x.clamp(0., (im.width - 1) as f32);
    let y = y.clamp(0., (im.height - 1) as f32);
    let ix = x as u32;
    let iy = y as u32;
    let fx = x - ix as f32;
    let fy = y - iy as f32;
    let at =
        |x: u32, y: u32| im.px((y.min(im.height - 1) * im.width + x.min(im.width - 1)) as usize);
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
/// Lightroom's Fringe Color Selector on the shown colour `rgb` (encoded sRGB): the
/// Purple or Green range is pointed at it (see [`Effects::pick_fringe`]). Defringe
/// tests the hue before the HSL Hue sliders of engines before 4, colour grading and
/// legacy channel curves change it, so the picked colour is the one those stages
/// render closest to the shown colour.
///
/// [`Effects::pick_fringe`]: super::effects::Effects::pick_fringe
pub fn pick_fringe(r: &mut Recipe, m: &Metadata, rgb: [f32; 3]) -> Option<usize> {
    let hue_of = |lab: [f32; 3]| {
        lab[2].atan2(lab[1]).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU
    };
    let lab = srgb_to_lab(rgb.map(crate::color_math::srgb_decode));
    let (shown, chroma) = (hue_of(lab), lab[1].hypot(lab[2]));
    let effective = r.resolved(m).into_owned();
    let lut = CurveSet::new(&effective);
    let turns = !lut.basic_curves && effective.hsl.iter().any(|band| band[0] != 0.);
    // The shown colour of one Defringe sees at lightness `l`, hue `h` and chroma `c`.
    let rendered = |[l, h, c]: [f32; 3]| {
        let shift: f32 = if turns {
            effective
                .hsl
                .iter()
                .zip(hue_weights(h))
                .map(|(band, w)| band[0] * w)
                .sum()
        } else {
            0.
        };
        let angle = (h + shift / 8.) * std::f32::consts::TAU;
        let out = finish_color([l, angle.cos() * c, angle.sin() * c], &effective, &lut);
        srgb_to_lab(out.map(crate::color_math::srgb_decode))
    };
    let miss = |p: [f32; 3]| {
        let q = rendered(p);
        (0..3).map(|i| (q[i] - lab[i]).powi(2)).sum::<f32>()
    };
    let best = |candidates: &mut dyn Iterator<Item = [f32; 3]>| {
        candidates.min_by(|a, b| miss(*a).total_cmp(&miss(*b)))
    };
    // A coarse search over lightness, hue and chroma, then a finer one around the best.
    let coarse = [1. / 20., 1. / 120., 1. / 50.];
    let [l, h, c] = best(&mut (0..=20).flat_map(|i| {
        (0..120).flat_map(move |j| {
            (0..=20).map(move |k| {
                [
                    i as f32 * coarse[0],
                    j as f32 * coarse[1],
                    k as f32 * coarse[2],
                ]
            })
        })
    }))
    .unwrap_or([lab[0], shown, chroma]);
    let fine = |i: i32, step: f32| i as f32 * step / 5.;
    let [_, hue, chroma] = best(&mut (-5..=5).flat_map(|i| {
        (-5..=5).flat_map(move |j| {
            (-5..=5).map(move |k| {
                [
                    (l + fine(i, coarse[0])).clamp(0., 1.),
                    (h + fine(j, coarse[1])).rem_euclid(1.),
                    (c + fine(k, coarse[2])).max(0.),
                ]
            })
        })
    }))
    .unwrap_or([l, h, c]);
    r.effects.pick_fringe_hue(hue, chroma)
}
pub fn neutral_pick(im: &CameraImage, r: &Recipe, u: f32, v: f32) -> [f32; 3] {
    let g = Geometry::new(im, r, 0);
    let [x, y] = g.source(u, v);
    let mut sum = [0.; 3];
    for dy in -2..=2 {
        for dx in -2..=2 {
            let p = sample(im.into(), x + dx as f32, y + dy as f32);
            for c in 0..3 {
                sum[c] += p[c];
            }
        }
    }
    std::array::from_fn(|c| (sum[1] / sum[c].max(1e-6)).clamp(0.01, 100.))
}
pub fn preview(im: &CameraImage, max: u32) -> CameraImage {
    preview_source(im.into(), max)
}
pub(crate) fn preview_source(im: Source, max: u32) -> CameraImage {
    if im.width.max(im.height) <= max {
        let mut out = im.image.clone();
        if im.gain.is_some() {
            out.pixels = (0..out.pixels.len()).map(|i| im.px(i)).collect();
        }
        return out;
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
                let p = im.px((yy * im.width + xx) as usize);
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
/// A detail sample averaged over an output pixel's footprint: four taps at ±`spread`
/// source pixels, which with bilinear sampling approximate a box filter.
fn footprint_sample(im: Source, x: f32, y: f32, r: &Recipe, spread: f32) -> [f32; 3] {
    if spread == 0. {
        return detail_sample(im, x, y, r);
    }
    let mut sum = [0.; 3];
    for (dx, dy) in [(-1., -1.), (1., -1.), (-1., 1.), (1., 1.)] {
        let p = detail_sample(im, x + dx * spread, y + dy * spread, r);
        for c in 0..3 {
            sum[c] += p[c] * 0.25;
        }
    }
    sum
}
/// Tap offset for [`footprint_sample`] when one output pixel covers `footprint`
/// source pixels: the four taps add the variance a box of that width has beyond a
/// single bilinear sample's.
pub(crate) fn footprint_spread(footprint: f32) -> f32 {
    (footprint * footprint / 12. - 1. / 6.).max(0.).sqrt()
}
fn detail_sample(im: Source, x: f32, y: f32, r: &Recipe) -> [f32; 3] {
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
/// Vignetting over the camera image: the lens profile's at its Vignetting amount, and
/// measured manual Vignetting. Radius 1 is the half diagonal; `x`, `y` use sample
/// coordinates, where pixel `i` is centred at `i`.
pub(crate) struct VignetteField<'a> {
    table: VignetteTable<'a>,
    center: [f32; 2],
    half: f32,
    /// The power the table is raised to: Lightroom's profile Vignetting amount (1 =
    /// 100%), or 1 for a combined table, which holds it already.
    amount: f32,
}
enum VignetteTable<'a> {
    Lens(&'a crate::lens::Radial),
    Combined(crate::lens::Radial),
}
impl<'a> VignetteField<'a> {
    pub(crate) fn new(im: &'a CameraImage, r: &Recipe) -> Option<Self> {
        let lens = r
            .lens_correction(&im.metadata)
            .and_then(|l| l.vignetting.as_ref());
        let (table, amount) = match (lens, r.manual_vignette()) {
            (_, Some(manual)) => (
                VignetteTable::Combined(super::effects::combined_table(
                    lens,
                    r.lens_vignetting,
                    &manual,
                )),
                1.,
            ),
            (Some(lens), None) => (VignetteTable::Lens(lens), r.lens_vignetting),
            (None, None) => return None,
        };
        let (w, h) = (im.width as f32, im.height as f32);
        Some(Self {
            table,
            center: [w * 0.5, h * 0.5],
            half: (w * w + h * h).sqrt() * 0.5,
            amount,
        })
    }
    fn table(&self) -> &crate::lens::Radial {
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
struct LensWarp<'a> {
    map: super::image_space::LensMap<'a>,
    vignetting: Option<VignetteField<'a>>,
}
impl<'a> LensWarp<'a> {
    fn new(im: &'a CameraImage, r: &Recipe) -> Option<Self> {
        Some(Self {
            map: super::image_space::LensMap::new(im, r)?,
            vignetting: VignetteField::new(im, r),
        })
    }
    /// Where `sample` reads red, green and blue for sample coordinates `x`, `y`.
    fn positions(&self, x: f32, y: f32) -> [[f32; 2]; 3] {
        let m = &self.map;
        let ([dx, dy], scale) = m.scales(x, y);
        scale.map(|k| [m.center[0] + dx * k - 0.5, m.center[1] + dy * k - 0.5])
    }
    fn sample(&self, im: Source, x: f32, y: f32, r: &Recipe, spread: f32) -> [f32; 3] {
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
/// Parameters of the GPU per-pixel stage for `im` and the resolved recipe `r`, or
/// `None` when the port does not cover it. The Shadows/Highlights map's tone pass over
/// the reduced photo also runs on the GPU; the map is then built from its luminance.
pub(crate) fn gpu_pixel_params(
    im: Source,
    r: &Recipe,
    backend: &mut super::preview_renderer::Backend,
    cancel: &std::sync::atomic::AtomicBool,
) -> Option<pixel_params::PixelParams> {
    if !pixel_params::needs_map(r) {
        return pixel_params::pixel_params(im, r);
    }
    let tone = pixel_params::tone_params(im, r)?;
    let small = match im.reduced {
        Some(small) => std::borrow::Cow::Borrowed(small),
        None => std::borrow::Cow::Owned(preview_source(im, super::local_tone::MAP_EDGE)),
    };
    let Some(toned) = backend.run(cancel, |gpu| {
        gpu.scoped(|gpu| gpu.develop_pixels(&small.pixels, &tone, cancel))
    }) else {
        return pixel_params::pixel_params(im, r);
    };
    let lum = toned
        .into_iter()
        .map(super::local_tone::luminance)
        .collect();
    let map = super::local_tone::LocalToneMap::from_luminance(
        lum,
        [small.width, small.height],
        [im.width, im.height],
        r.shadows,
        r.highlights,
    );
    Some(pixel_params::with_map(tone, &map))
}
/// Lens correction for `gpu/local.wgsl`, from `S_LENS` to `S_VIGNETTING_AMOUNT`, with
/// radial tables appended to `tables` (each: knots, then values) at offsets counted
/// from `base`. Offsets are -1 for absent tables.
pub(crate) fn lens_gpu_params(
    im: &CameraImage,
    r: &Recipe,
    base: usize,
    tables: &mut Vec<f32>,
) -> [f32; 15] {
    let mut push = |radial: Option<&crate::lens::Radial>| match radial {
        Some(radial) => {
            let at = base + tables.len();
            tables.extend(&radial.knots);
            tables.extend(&radial.values);
            [at as f32, radial.knots.len() as f32]
        }
        None => [-1., 0.],
    };
    let mut out = [0.; 15];
    let Some(warp) = LensWarp::new(im, r) else {
        out[5..13].copy_from_slice(&[-1., 0., -1., 0., -1., 0., -1., 0.]);
        return out;
    };
    let lens = warp.map.lens;
    let distortion = push(lens.distortion.as_ref());
    let [red, blue] = match warp.map.chromatic.or(lens.chromatic.as_ref()) {
        Some([red, blue]) => [push(Some(red)), push(Some(blue))],
        None => [[-1., 0.]; 2],
    };
    let vignetting = push(warp.vignetting.as_ref().map(VignetteField::table));
    let m = &warp.map;
    // 2 marks a measured aberration, evaluated at the distorted radius.
    let mode = if m.chromatic.is_some() { 2. } else { 1. };
    out[..5].copy_from_slice(&[mode, m.center[0], m.center[1], m.half, m.fill]);
    out[5] = m.amount;
    out[6..8].copy_from_slice(&distortion);
    out[8..10].copy_from_slice(&red);
    out[10..12].copy_from_slice(&blue);
    out[12..14].copy_from_slice(&vignetting);
    out[14] = warp.vignetting.as_ref().map_or(0., |v| v.amount);
    out
}
/// The photo pixels (x, y, width, height) that sampling `region` of the output `g`
/// reads, found by mapping the region's edges through geometry and lens correction and
/// adding the bilinear, noise-reduction and footprint taps. Empty when none map inside.
pub(crate) fn source_bounds(
    im: &CameraImage,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
) -> [u32; 4] {
    let warp = LensWarp::new(im, r);
    let [x0, y0, w, h] = region;
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    let mut add = |x: u32, y: u32| {
        let [sx, sy] = g.source(
            (x as f32 + 0.5) / g.width as f32,
            (y as f32 + 0.5) / g.height as f32,
        );
        let points = match &warp {
            Some(warp) => warp.positions(sx, sy),
            None => [[sx, sy]; 3],
        };
        for p in points {
            for c in 0..2 {
                lo[c] = lo[c].min(p[c]);
                hi[c] = hi[c].max(p[c]);
            }
        }
    };
    for x in x0..x0 + w {
        add(x, y0);
        add(x, y0 + h - 1);
    }
    for y in y0..y0 + h {
        add(x0, y);
        add(x0 + w - 1, y);
    }
    if !(lo[0] <= hi[0] && lo[1] <= hi[1]) {
        return [0; 4];
    }
    let pad = 2. + spread.ceil();
    let size = [im.width, im.height];
    let [a, b] = std::array::from_fn(|c| {
        let top = (size[c] - 1) as f32;
        (
            (lo[c] - pad).clamp(0., top).floor() as u32,
            (hi[c] + pad).clamp(0., top).ceil() as u32,
        )
    });
    [a.0, b.0, a.1 - a.0 + 1, b.1 - b.0 + 1]
}
/// Vignetting (lens profile and manual) for `gpu/logs.wgsl`: centre, half diagonal, amount and the
/// radial table (knots, then values), or `None`.
pub(crate) fn vignetting_gpu_params(im: &CameraImage, r: &Recipe) -> Option<([f32; 4], Vec<f32>)> {
    let v = VignetteField::new(im, r)?;
    let radial = v.table();
    let mut table = radial.knots.clone();
    table.extend(&radial.values);
    Some(([v.center[0], v.center[1], v.half, v.amount], table))
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
/// Unsharpened render of `region` of the output described by `g`, and the mask weights
/// the finishing stages need. A `spread` above zero averages each sample over a
/// footprint (see [`footprint_spread`]).
///
/// With a `cache`, the geometry, lens-warp and noise-reduction samples are kept, so a
/// following render that only changes color and tone reruns the per-pixel stage alone.
/// A `backend` with a GPU runs that stage there when the port covers the recipe.
pub(crate) fn render_base(
    toned: &Toned,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
    cancel: &std::sync::atomic::AtomicBool,
    stages: Option<&mut super::preview_renderer::Stages>,
) -> Result<(Rendered, Option<Arc<MaskWeights>>)> {
    let mut base = r.clone();
    base.sharpening = 0.;
    let im = toned.source();
    let masked = base.masks.iter().any(super::masks::MaskGroup::is_active);
    let Some(stages) = stages else {
        if !masked {
            return Ok((
                render_region_inner(im, &base, g, region, spread, cancel)?,
                None,
            ));
        }
        base.validate()?;
        let samples = Arc::new(sample_region(im, &base, g, region, spread, cancel)?);
        let weights = mask_weights(
            toned,
            &base,
            g,
            region,
            spread,
            Some(&samples),
            None,
            cancel,
        )?;
        let samples = detail(toned, &base, samples, weights.as_deref(), None, cancel)?;
        let out = develop_samples(im, &base, &samples, cancel, weights.as_deref())?;
        return Ok((out, weights));
    };
    base.validate()?;
    let key = super::stage_cache::SampleKey::new(toned, &base, g, region, spread);
    let samples = stages.cache.samples.get_or_try(key, Samples::bytes, || {
        sample_region(im, &base, g, region, spread, cancel)
    })?;
    let weights = mask_weights(
        toned,
        &base,
        g,
        region,
        spread,
        Some(&samples),
        Some(&mut *stages),
        cancel,
    )?;
    let samples = detail(
        toned,
        &base,
        samples,
        weights.as_deref(),
        Some(&mut *stages.cache),
        cancel,
    )?;
    let backend = &mut *stages.backend;
    if backend.has_gpu()
        && let Some(mut params) = pixel_params::pixel_params(im, &base)
        && params.set_masks(im, &base, weights.as_deref())
        && let Some(out) = backend.develop(&samples, &params, cancel)
    {
        return Ok((out, weights));
    }
    let out = develop_samples(im, &base, &samples, cancel, weights.as_deref())?;
    Ok((out, weights))
}
/// As [`render_base`] followed by sharpening, spatial effects and display, all on the
/// GPU into a texture for the stages' display. `None` without a GPU or display, or when
/// the port does not cover `r` (the tonal recipe); `finished` is the full recipe.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_display(
    toned: &Toned,
    r: &Recipe,
    finished: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
    finish: &crate::develop::gpu::Finish,
    cancel: &std::sync::atomic::AtomicBool,
    stages: &mut super::preview_renderer::Stages,
) -> Result<Option<crate::develop::gpu::Frame>> {
    let Some(display) = stages.display else {
        return Ok(None);
    };
    if !stages.backend.has_gpu() {
        return Ok(None);
    }
    let mut base = r.clone();
    base.sharpening = 0.;
    base.validate()?;
    let im = toned.source();
    let Some(mut params) = gpu_pixel_params(im, &base, stages.backend, cancel) else {
        return Ok(None);
    };
    let key = super::stage_cache::SampleKey::new(toned, &base, g, region, spread);
    let samples = stages.cache.samples.get_or_try(key, Samples::bytes, || {
        sample_region(im, &base, g, region, spread, cancel)
    })?;
    let weights = mask_weights(
        toned,
        &base,
        g,
        region,
        spread,
        Some(&samples),
        Some(&mut *stages),
        cancel,
    )?;
    // Local Sharpness and Noise finish on the CPU.
    if weights
        .as_ref()
        .is_some_and(|w| w.uses(&[slot::SHARPNESS, slot::NOISE]))
        || !params.set_masks(im, &base, weights.as_deref())
    {
        return Ok(None);
    }
    let samples = detail(
        toned,
        &base,
        samples,
        weights.as_deref(),
        Some(&mut *stages.cache),
        cancel,
    )?;
    Ok(stages
        .backend
        .present(&samples, &params, finished, finish, display, cancel))
}
/// Weights of the recipe's active masks over `region` of `g`, rendered from `toned`'s
/// image, through the stage cache when there is one. Range components first develop
/// `samples` without local adjustments; without samples they cannot be evaluated and
/// the result is `None`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn mask_weights(
    toned: &Toned,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
    samples: Option<&Arc<Samples>>,
    stages: Option<&mut super::preview_renderer::Stages>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Option<Arc<MaskWeights>>> {
    use super::masks::{Selection, Weigher};
    if !r.masks.iter().any(super::masks::MaskGroup::is_active) {
        return Ok(None);
    }
    let ranges = r
        .masks
        .iter()
        .any(|m| m.is_active() && m.components.iter().any(|c| c.shape.is_range()));
    let key = super::stage_cache::MaskKey::new(toned, r, g, region, spread, ranges);
    let mut stages = stages;
    if let Some(s) = stages.as_deref_mut()
        && let Some(hit) = s.cache.masks.get(&key)
    {
        return Ok(Some(Arc::new(hit.with_deltas(&r.masks))));
    }
    let weigher = Weigher::cached(
        &toned.image,
        &r.masks,
        Selection::Active,
        stages.as_deref_mut().map(|s| &mut s.cache.rasters),
    );
    let global = if weigher.needs_range() {
        let Some(samples) = samples else {
            return Ok(None);
        };
        let mut plain = r.clone();
        plain.masks.clear();
        // Range masks select from the photo as it renders, never as Visualize Range
        // grays it.
        crate::develop::point_color::without_visualization(&mut plain.point_colors);
        let im = toned.source();
        let gpu = stages.as_deref_mut().and_then(|s| {
            let params = pixel_params::pixel_params(im, &plain)?;
            s.backend.develop(samples, &params, cancel)
        });
        Some(match gpu {
            Some(out) => out,
            None => develop_samples(im, &plain, samples, cancel, None)?,
        })
    } else {
        None
    };
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Render superseded"
    );
    let weights = Arc::new(weigher.weights(&toned.image, r, g, region, global.as_ref()));
    if let Some(s) = stages {
        s.cache.masks.insert(key, weights.clone(), weights.bytes());
    }
    Ok(Some(weights))
}
/// Local Texture and Clarity: the samples scaled by the local-contrast detail of the
/// camera image at their positions, as the global sliders' gain does.
fn detail(
    toned: &Toned,
    r: &Recipe,
    samples: Arc<Samples>,
    weights: Option<&MaskWeights>,
    cache: Option<&mut super::stage_cache::StageCache>,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Arc<Samples>> {
    let Some(weights) = weights.filter(|w| w.uses(&[slot::TEXTURE, slot::CLARITY])) else {
        return Ok(samples);
    };
    let texture = weights.uses(&[slot::TEXTURE]);
    let blurs = super::quality::blurs(&toned.image, r, toned.scale, texture, cache, cancel)?;
    let exposure = r.exposure + r.camera_exposure;
    let (w, h) = (toned.image.width as usize, toned.image.height as usize);
    let mut out = Samples {
        width: samples.width,
        height: samples.height,
        pixels: samples.pixels.clone(),
        positions: samples.positions.clone(),
    };
    out.pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
        let Some(d) = weights.delta(i) else {
            return;
        };
        let (clarity, tex) = (d[slot::CLARITY], d[slot::TEXTURE]);
        let [x, y] = samples.positions[i];
        if (clarity == 0. && tex == 0.) || x.is_nan() {
            return;
        }
        let gain = blurs.detail_gain(x, y, w, h, exposure, clarity, tex);
        *p = p.map(|v| v * gain);
    });
    Ok(Arc::new(out))
}
/// Camera samples of an output region after geometry, lens correction and noise
/// reduction, with their source positions; `NAN` positions lie outside the photo.
pub(crate) struct Samples {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Vec<[f32; 3]>,
    pub(crate) positions: Vec<[f32; 2]>,
}
impl Samples {
    fn bytes(&self) -> usize {
        self.pixels.len() * 20
    }
}
fn sample_region(
    im: Source,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Samples> {
    let [x0, y0, w, h] = region;
    ensure!(
        w > 0
            && h > 0
            && x0.checked_add(w).is_some_and(|r| r <= g.width)
            && y0.checked_add(h).is_some_and(|b| b <= g.height),
        "Invalid viewport region"
    );
    let warp = LensWarp::new(&im, r);
    let (pixels, positions) = (0..w as usize * h as usize)
        .into_par_iter()
        .map(|i| {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return ([0.; 3], [f32::NAN; 2]);
            }
            let x = x0 + i as u32 % w;
            let y = y0 + i as u32 / w;
            let [sx, sy] = g.source(
                (x as f32 + 0.5) / g.width as f32,
                (y as f32 + 0.5) / g.height as f32,
            );
            if g.outside(sx, sy) {
                return ([1.; 3], [f32::NAN; 2]);
            }
            let p = match &warp {
                Some(w) => w.sample(im, sx, sy, r, spread),
                None => footprint_sample(im, sx, sy, r, spread),
            };
            (p, [sx, sy])
        })
        .unzip();
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Render superseded"
    );
    Ok(Samples {
        width: w,
        height: h,
        pixels,
        positions,
    })
}
/// The per-pixel color and tone stage over prepared samples, with the masks'
/// adjustments where `weights` has them.
pub(crate) fn develop_samples(
    im: Source,
    r: &Recipe,
    samples: &Samples,
    cancel: &std::sync::atomic::AtomicBool,
    weights: Option<&MaskWeights>,
) -> Result<Rendered> {
    develop_samples_to(im, r, samples, cancel, weights, PixelOutput::Display)
}
/// `region` of the output as a dropper samples it at the stage `output` names,
/// through the same sampling, lens correction, retouching and masks as the render.
pub(crate) fn stage_samples(
    toned: &Toned,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    output: PixelOutput,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Rendered> {
    let im = toned.source();
    let samples = Arc::new(sample_region(im, r, g, region, 0., cancel)?);
    let weights = mask_weights(toned, r, g, region, 0., Some(&samples), None, cancel)?;
    let samples = detail(toned, r, samples, weights.as_deref(), None, cancel)?;
    develop_samples_to(im, r, &samples, cancel, weights.as_deref(), output)
}
fn develop_samples_to(
    im: Source,
    r: &Recipe,
    samples: &Samples,
    cancel: &std::sync::atomic::AtomicBool,
    weights: Option<&MaskWeights>,
    output: PixelOutput,
) -> Result<Rendered> {
    let matrix = profile_matrix(&im.metadata, r);
    let local_tone = weights.is_some_and(|w| w.uses(&[slot::SHADOWS, slot::HIGHLIGHTS]));
    let mut lut = CurveSet::for_image(im, r, matrix, local_tone);
    lut.output = output;
    let math = weights.map(|_| LocalMath::new(&im.metadata, r));
    let mut pixels = vec![[0.; 3]; samples.pixels.len()];
    pixels.par_iter_mut().enumerate().for_each(|(i, out)| {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let pos = samples.positions[i];
        let delta = weights.and_then(|w| w.delta(i));
        let local = delta
            .as_ref()
            .zip(math.as_ref())
            .map(|(delta, math)| Local { delta, math });
        *out = if pos[0].is_nan() {
            [1.; 3]
        } else {
            process_pixel(samples.pixels[i], &im.metadata, r, &lut, matrix, pos, local)
        };
    });
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Render superseded"
    );
    Ok(Rendered {
        width: samples.width,
        height: samples.height,
        pixels,
    })
}

pub fn render_legacy(im: &CameraImage, r: &Recipe, max_edge: u32) -> Result<Rendered> {
    r.validate()?;
    let im = legacy_retouched(im, r);
    render_legacy_inner(&im, &r.resolved(&im.metadata), max_edge)
}
/// The camera image with the recipe's red eye corrections and spots applied, for the
/// older engines, which develop without highlight recovery or the retouch cache.
fn legacy_retouched<'a>(im: &'a CameraImage, r: &Recipe) -> std::borrow::Cow<'a, CameraImage> {
    let shown = r.as_rendered();
    let ops = super::retouch::Retouching::of(&shown);
    if ops.is_empty() {
        std::borrow::Cow::Borrowed(im)
    } else {
        std::borrow::Cow::Owned(super::retouch::apply(im, ops))
    }
}
fn render_legacy_inner(im: &CameraImage, r: &Recipe, max_edge: u32) -> Result<Rendered> {
    let matrix = profile_matrix(&im.metadata, r);
    let lut = CurveSet::for_image(im.into(), r, matrix, false);
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
            Some(w) => w.sample(im.into(), sx, sy, r, 0.),
            None => detail_sample(im.into(), sx, sy, r),
        };
        *out = process_pixel(p, &im.metadata, r, &lut, matrix, [sx, sy], None);
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
    let im = legacy_retouched(im, r);
    let im = im.as_ref();
    let r = r.resolved(&im.metadata);
    render_region_inner(
        im.into(),
        &r,
        &Geometry::new(im, &r, 0),
        region,
        0.,
        &std::sync::atomic::AtomicBool::new(false),
    )
}
fn render_region_inner(
    im: Source,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Rendered> {
    r.validate()?;
    let matrix = profile_matrix(&im.metadata, r);
    let lut = CurveSet::for_image(im, r, matrix, false);
    let [x0, y0, w, h] = region;
    ensure!(
        w > 0
            && h > 0
            && x0.checked_add(w).is_some_and(|r| r <= g.width)
            && y0.checked_add(h).is_some_and(|b| b <= g.height),
        "Invalid viewport region"
    );
    let mut pixels = vec![[0.; 3]; w as usize * h as usize];
    let warp = LensWarp::new(&im, r);
    let at = |x: u32, y: u32| {
        let [sx, sy] = g.source(
            (x as f32 + 0.5) / g.width as f32,
            (y as f32 + 0.5) / g.height as f32,
        );
        if g.outside(sx, sy) {
            return [1.; 3];
        }
        let p = match &warp {
            Some(w) => w.sample(im, sx, sy, r, spread),
            None => footprint_sample(im, sx, sy, r, spread),
        };
        process_pixel(p, &im.metadata, r, &lut, matrix, [sx, sy], None)
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

pub(crate) mod pixel_params;
#[cfg(test)]
mod tests;
