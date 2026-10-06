//! The develop settings controls move, described once: their label, the range a
//! slider or dial covers, the range a recipe may hold, how one dial tick moves
//! them and how their value is shown. Sliders, the control socket, MIDI and
//! History read them here rather than keeping their own copies.
//!
//! A control's interactive range can be narrower than the values a recipe may
//! hold: the Exposure slider spans ±5 EV, while an imported edit can carry up to
//! ±8. Relative input must not quietly pull such a value into the narrower range.
use super::{EXPOSURE_LIMIT, Recipe, TEMPERATURE_MAX, TEMPERATURE_MIN, TINT_LIMIT};
use std::ops::RangeInclusive;

/// The Crop panel's Angle limit either way, in degrees, as `Recipe::validate` allows.
pub const STRAIGHTEN_LIMIT: f32 = 45.;

/// A develop setting that sliders, dials and commands move.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParameterId {
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Texture,
    Clarity,
    Dehaze,
    Vibrance,
    Saturation,
    Temperature,
    Tint,
    /// The Crop panel's Angle, in degrees.
    Straighten,
    /// Detail: Sharpening.
    SharpeningAmount,
    /// In pixels.
    SharpeningRadius,
    SharpeningDetail,
    SharpeningMasking,
    /// Detail: Noise Reduction.
    LuminanceNoise,
    LuminanceDetail,
    LuminanceContrast,
    ColorNoise,
    ColorNoiseDetail,
    ColorNoiseSmoothness,
    /// Effects: Post-Crop Vignetting.
    VignetteAmount,
    VignetteMidpoint,
    VignetteFeather,
    /// Effects: Grain.
    GrainAmount,
    GrainSize,
    GrainRoughness,
    /// Transform, stored along the photo's own axes; the panel shows them along the
    /// displayed photo's.
    TransformVertical,
    TransformHorizontal,
    /// In degrees.
    TransformRotate,
    TransformAspect,
    TransformScale,
    TransformOffsetX,
    TransformOffsetY,
}

/// How one dial tick or `turn` step moves a setting.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tick {
    /// By this much, in the setting's own units.
    Linear(f32),
    /// By this many mireds, as the Temp slider moves: evenly in mireds, so a
    /// tick is a similar change of colour anywhere on the scale; clockwise is
    /// warmer.
    Mireds(f32),
}

/// How a setting's value is shown and typed: in its own unit, or scaled to
/// −100..100.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Display {
    /// What the stored value is multiplied by to show it.
    pub scale: f32,
    pub decimals: usize,
    /// Whether positive values show a `+`.
    pub signed: bool,
}

/// Everything controls need to know about a setting.
#[derive(Clone, Debug, PartialEq)]
pub struct Descriptor {
    pub id: ParameterId,
    /// The slider's label, and the name of its History step.
    pub label: &'static str,
    /// What a slider or dial covers, in the recipe's units.
    pub interactive: RangeInclusive<f32>,
    /// What a recipe may hold, and what a typed value or the `set` command may set.
    pub valid: RangeInclusive<f32>,
    pub tick: Tick,
    /// What dragging its slider snaps to, as Lightroom's 0.05 EV for Exposure;
    /// `None` moves freely.
    pub drag_step: Option<f32>,
    pub display: Display,
}

const UNIT: Display = Display {
    scale: 100.,
    decimals: 0,
    signed: true,
};
const HUNDREDTHS: Display = Display {
    scale: 1.,
    decimals: 2,
    signed: true,
};

/// A 0..100 slider stored as 0..1.
const fn amount(id: ParameterId, label: &'static str) -> Descriptor {
    Descriptor {
        id,
        label,
        interactive: 0. ..=1.,
        valid: 0. ..=1.,
        tick: Tick::Linear(0.01),
        drag_step: None,
        display: Display {
            scale: 100.,
            decimals: 0,
            signed: false,
        },
    }
}

/// A −100..100 slider stored as −1..1.
const fn percent(id: ParameterId, label: &'static str) -> Descriptor {
    Descriptor {
        id,
        label,
        interactive: -1. ..=1.,
        valid: -1. ..=1.,
        tick: Tick::Linear(0.01),
        drag_step: None,
        display: UNIT,
    }
}

const DESCRIPTORS: [Descriptor; 37] = [
    Descriptor {
        id: ParameterId::Exposure,
        label: "Exposure",
        interactive: -5. ..=5.,
        valid: -EXPOSURE_LIMIT..=EXPOSURE_LIMIT,
        tick: Tick::Linear(0.02),
        drag_step: Some(0.05),
        display: HUNDREDTHS,
    },
    percent(ParameterId::Contrast, "Contrast"),
    percent(ParameterId::Highlights, "Highlights"),
    percent(ParameterId::Shadows, "Shadows"),
    percent(ParameterId::Whites, "Whites"),
    percent(ParameterId::Blacks, "Blacks"),
    percent(ParameterId::Texture, "Texture"),
    percent(ParameterId::Clarity, "Clarity"),
    percent(ParameterId::Dehaze, "Dehaze"),
    percent(ParameterId::Vibrance, "Vibrance"),
    percent(ParameterId::Saturation, "Saturation"),
    Descriptor {
        id: ParameterId::Temperature,
        label: "Temp",
        interactive: TEMPERATURE_MIN..=TEMPERATURE_MAX,
        valid: TEMPERATURE_MIN..=TEMPERATURE_MAX,
        tick: Tick::Mireds(4.),
        drag_step: None,
        display: Display {
            scale: 1.,
            decimals: 0,
            signed: false,
        },
    },
    Descriptor {
        id: ParameterId::Tint,
        label: "Tint",
        interactive: -TINT_LIMIT..=TINT_LIMIT,
        valid: -TINT_LIMIT..=TINT_LIMIT,
        tick: Tick::Linear(1.),
        drag_step: None,
        display: Display {
            scale: 1.,
            decimals: 0,
            signed: true,
        },
    },
    Descriptor {
        id: ParameterId::Straighten,
        label: "Angle",
        interactive: -STRAIGHTEN_LIMIT..=STRAIGHTEN_LIMIT,
        valid: -STRAIGHTEN_LIMIT..=STRAIGHTEN_LIMIT,
        // Fine enough to level a horizon.
        tick: Tick::Linear(0.1),
        drag_step: None,
        display: HUNDREDTHS,
    },
    Descriptor {
        // Lightroom's 0..150.
        display: Display {
            scale: 150.,
            decimals: 0,
            signed: false,
        },
        ..amount(ParameterId::SharpeningAmount, "Amount")
    },
    Descriptor {
        id: ParameterId::SharpeningRadius,
        label: "Radius",
        interactive: 0.5..=3.,
        valid: 0.5..=3.,
        tick: Tick::Linear(0.1),
        drag_step: None,
        display: Display {
            scale: 1.,
            decimals: 1,
            signed: false,
        },
    },
    amount(ParameterId::SharpeningDetail, "Detail"),
    amount(ParameterId::SharpeningMasking, "Masking"),
    amount(ParameterId::LuminanceNoise, "Luminance"),
    amount(ParameterId::LuminanceDetail, "Detail"),
    amount(ParameterId::LuminanceContrast, "Contrast"),
    amount(ParameterId::ColorNoise, "Color"),
    amount(ParameterId::ColorNoiseDetail, "Detail"),
    amount(ParameterId::ColorNoiseSmoothness, "Smoothness"),
    percent(ParameterId::VignetteAmount, "Amount"),
    amount(ParameterId::VignetteMidpoint, "Midpoint"),
    amount(ParameterId::VignetteFeather, "Feather"),
    amount(ParameterId::GrainAmount, "Amount"),
    amount(ParameterId::GrainSize, "Size"),
    amount(ParameterId::GrainRoughness, "Roughness"),
    percent(ParameterId::TransformVertical, "Vertical"),
    percent(ParameterId::TransformHorizontal, "Horizontal"),
    Descriptor {
        id: ParameterId::TransformRotate,
        label: "Rotate",
        interactive: -10. ..=10.,
        valid: -10. ..=10.,
        tick: Tick::Linear(0.1),
        drag_step: None,
        display: Display {
            scale: 1.,
            decimals: 1,
            signed: true,
        },
    },
    percent(ParameterId::TransformAspect, "Aspect"),
    Descriptor {
        id: ParameterId::TransformScale,
        label: "Scale",
        interactive: 0.5..=1.5,
        valid: 0.5..=1.5,
        tick: Tick::Linear(0.01),
        drag_step: None,
        display: Display {
            scale: 100.,
            decimals: 0,
            signed: false,
        },
    },
    percent(ParameterId::TransformOffsetX, "Offset X"),
    percent(ParameterId::TransformOffsetY, "Offset Y"),
];

impl ParameterId {
    pub const ALL: [Self; 37] = [
        Self::Exposure,
        Self::Contrast,
        Self::Highlights,
        Self::Shadows,
        Self::Whites,
        Self::Blacks,
        Self::Texture,
        Self::Clarity,
        Self::Dehaze,
        Self::Vibrance,
        Self::Saturation,
        Self::Temperature,
        Self::Tint,
        Self::Straighten,
        Self::SharpeningAmount,
        Self::SharpeningRadius,
        Self::SharpeningDetail,
        Self::SharpeningMasking,
        Self::LuminanceNoise,
        Self::LuminanceDetail,
        Self::LuminanceContrast,
        Self::ColorNoise,
        Self::ColorNoiseDetail,
        Self::ColorNoiseSmoothness,
        Self::VignetteAmount,
        Self::VignetteMidpoint,
        Self::VignetteFeather,
        Self::GrainAmount,
        Self::GrainSize,
        Self::GrainRoughness,
        Self::TransformVertical,
        Self::TransformHorizontal,
        Self::TransformRotate,
        Self::TransformAspect,
        Self::TransformScale,
        Self::TransformOffsetX,
        Self::TransformOffsetY,
    ];
    pub fn descriptor(self) -> &'static Descriptor {
        &DESCRIPTORS[self as usize]
    }
    /// The recipe field this setting is.
    pub fn value_mut(self, r: &mut Recipe) -> &mut f32 {
        match self {
            Self::Exposure => &mut r.exposure,
            Self::Contrast => &mut r.contrast,
            Self::Highlights => &mut r.highlights,
            Self::Shadows => &mut r.shadows,
            Self::Whites => &mut r.whites,
            Self::Blacks => &mut r.blacks,
            Self::Texture => &mut r.effects.texture,
            Self::Clarity => &mut r.effects.clarity,
            Self::Dehaze => &mut r.effects.dehaze,
            Self::Vibrance => &mut r.vibrance,
            Self::Saturation => &mut r.saturation,
            Self::Temperature => &mut r.temperature,
            Self::Tint => &mut r.tint,
            Self::Straighten => &mut r.straighten,
            Self::SharpeningAmount => &mut r.sharpening,
            Self::SharpeningRadius => &mut r.sharpening_radius,
            Self::SharpeningDetail => &mut r.sharpening_detail,
            Self::SharpeningMasking => &mut r.sharpening_masking,
            Self::LuminanceNoise => &mut r.noise_luma,
            Self::LuminanceDetail => &mut r.effects.luma_detail,
            Self::LuminanceContrast => &mut r.effects.luma_contrast,
            Self::ColorNoise => &mut r.noise_chroma,
            Self::ColorNoiseDetail => &mut r.effects.chroma_detail,
            Self::ColorNoiseSmoothness => &mut r.effects.chroma_smoothness,
            Self::VignetteAmount => &mut r.effects.vignette,
            Self::VignetteMidpoint => &mut r.effects.vignette_midpoint,
            Self::VignetteFeather => &mut r.effects.vignette_feather,
            Self::GrainAmount => &mut r.effects.grain,
            Self::GrainSize => &mut r.effects.grain_size,
            Self::GrainRoughness => &mut r.effects.grain_roughness,
            Self::TransformVertical => &mut r.transform.vertical,
            Self::TransformHorizontal => &mut r.transform.horizontal,
            Self::TransformRotate => &mut r.transform.rotate,
            Self::TransformAspect => &mut r.transform.aspect,
            Self::TransformScale => &mut r.transform.scale,
            Self::TransformOffsetX => &mut r.transform.offset_x,
            Self::TransformOffsetY => &mut r.transform.offset_y,
        }
    }
    /// `value` in the units the slider shows, rounded to thousandths.
    pub fn shown(self, value: f32) -> f64 {
        let scale = f64::from(self.descriptor().display.scale);
        (f64::from(value) * scale * 1000.).round() / 1000.
    }
    /// `value` as the slider shows it: "+0.50", "-35", "6500".
    pub fn text(self, value: f32) -> String {
        let display = self.descriptor().display;
        format_value(
            f64::from(value * display.scale),
            display.decimals,
            display.signed,
        )
    }
    /// The value of a number typed in the units the slider shows, within what a
    /// recipe may hold.
    pub fn from_shown(self, shown: f32) -> f32 {
        let d = self.descriptor();
        (shown / d.display.scale).clamp(*d.valid.start(), *d.valid.end())
    }
    /// `value` moved by `ticks` dial ticks (clockwise positive); see [`nudged`]
    /// for values outside the interactive range.
    pub fn turned(self, value: f32, ticks: i32) -> f32 {
        let d = self.descriptor();
        let t = ticks as f32;
        match d.tick {
            Tick::Linear(step) => nudged(value, step * t, d.interactive.clone()),
            Tick::Mireds(step) => {
                let (low, high) = (*d.interactive.start(), *d.interactive.end());
                let mired = (1e6 / value - step * t).max(1e6 / high);
                (1e6 / mired).clamp(low, high)
            }
        }
    }
}

/// A number as sliders show it: `decimals` places, and a `+` on positive values
/// when `signed`.
pub fn format_value(v: f64, decimals: usize, signed: bool) -> String {
    let text = format!("{v:.decimals$}");
    if signed && v > 0. && !text.trim_start_matches(['0', '.']).is_empty() {
        format!("+{text}")
    } else {
        text
    }
}

/// `value` moved by `delta` by relative input (a dial tick, an arrow key over a
/// slider) on a control whose interactive range is `range`.
///
/// Inside the range, the result is clamped to it. A value outside it moves by
/// `delta` toward the range, without snapping to the nearer bound, and a move
/// further away leaves it as it is. Relative input therefore never pushes a value
/// further out, and never discards the part of it beyond the bound.
pub fn nudged(value: f32, delta: f32, range: RangeInclusive<f32>) -> f32 {
    let (low, high) = (*range.start(), *range.end());
    let moved = value + delta;
    if value > high {
        if delta < 0. { moved.max(low) } else { value }
    } else if value < low {
        if delta > 0. { moved.min(high) } else { value }
    } else {
        moved.clamp(low, high)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_parameter_has_its_own_descriptor() {
        for id in ParameterId::ALL {
            assert_eq!(id.descriptor().id, id);
        }
    }

    #[test]
    fn a_recipe_is_valid_exactly_within_each_valid_range() {
        for id in ParameterId::ALL {
            let d = id.descriptor();
            assert!(
                d.valid.contains(d.interactive.start()) && d.valid.contains(d.interactive.end()),
                "{id:?}"
            );
            for (value, valid) in [
                (*d.valid.start(), true),
                (*d.valid.end(), true),
                (d.valid.start() - 0.01, false),
                (d.valid.end() + 0.01, false),
            ] {
                let mut r = Recipe::default();
                *id.value_mut(&mut r) = value;
                assert_eq!(r.validate().is_ok(), valid, "{id:?} at {value}");
            }
        }
    }

    #[test]
    fn values_show_and_read_back_in_slider_units() {
        assert_eq!(ParameterId::Contrast.text(0.35), "+35");
        assert_eq!(ParameterId::Exposure.text(-0.5), "-0.50");
        assert_eq!(ParameterId::Temperature.text(6500.), "6500");
        assert_eq!(ParameterId::Contrast.from_shown(150.), 1.);
        assert_eq!(ParameterId::Exposure.from_shown(9.), EXPOSURE_LIMIT);
        assert_eq!(ParameterId::Contrast.shown(0.35), 35.);
    }

    #[test]
    fn relative_input_never_moves_a_value_further_out_or_snaps_it() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-5;
        // (value, delta) -> result, on a ±5 control.
        for (value, delta, expected) in [
            (0., 0.02, 0.02),
            (4.99, 0.02, 5.),
            (-4.99, -0.02, -5.),
            (6., -0.02, 5.98),
            (6., 0.02, 6.),
            (6., -20., -5.),
            (-6., 0.02, -5.98),
            (-6., -0.02, -6.),
        ] {
            let result = nudged(value, delta, -5. ..=5.);
            assert!(close(result, expected), "{value} {delta:+}: {result}");
        }
    }
}
