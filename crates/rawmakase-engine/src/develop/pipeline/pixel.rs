//! The per-pixel stages: tone, the colour mixer and colour grading, then the finish into sRGB.
use super::*;
use crate::develop::effects::EffectsRendering;
use crate::model::operators::GamutModel;

/// A pixel's mask adjustments and the render's constants for them.
#[derive(Clone, Copy)]
pub(crate) struct Local<'a> {
    pub(crate) delta: &'a LocalDelta,
    pub(crate) math: &'a LocalMath,
}
pub(super) fn process_pixel(
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
pub(super) fn tone_stage(
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
                default_black(r) * (r.exposure + r.camera_exposure + exposure).exp2(),
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
pub(super) fn color_stage(
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
            lab[0] = match &lut.gray_grid {
                // The gray's Oklab lightness is the cube root of its luminance.
                Some(grid) => crate::develop::black_white::gray(grid, lab_to_srgb(lab))
                    .cbrt()
                    .clamp(0., 1.),
                None => (lab[0] + gray_mix_shift(shift, chroma)).clamp(0., 1.),
            };
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
pub(super) fn finish_color(mut lab: [f32; 3], r: &Recipe, lut: &CurveSet) -> [f32; 3] {
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
    let rgb = r.gamut_model.into_srgb(rgb, lab[0]);
    std::array::from_fn(|c| {
        let encoded = srgb_encode(rgb[c]);
        if r.wide_gamut_curves || r.reference_curves {
            encoded.clamp(0., 1.)
        } else {
            apply_curve(encoded, c, r, lut)
        }
    })
}

/// How a gamut model brings colors into sRGB.
pub(crate) trait GamutMapping {
    /// Linear sRGB inside 0–1; `lightness` is the color's Oklab lightness.
    fn into_srgb(self, rgb: [f32; 3], lightness: f32) -> [f32; 3];
}
impl GamutMapping for GamutModel {
    fn into_srgb(self, rgb: [f32; 3], lightness: f32) -> [f32; 3] {
        match self {
            Self::Clip => rgb.map(|v| v.clamp(0., 1.)),
            Self::Compress => {
                // Compress chroma toward neutral instead of clipping single channels.
                let gray = lightness.clamp(0., 1.).powi(3);
                let mut gamut = 1f32;
                for v in rgb {
                    if v < 0. {
                        gamut = gamut.min(gray / (gray - v).max(1e-8));
                    }
                    if v > 1. {
                        gamut = gamut.min((1. - gray) / (v - gray).max(1e-8));
                    }
                }
                rgb.map(|v| gray + (v - gray) * gamut)
            }
        }
    }
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
