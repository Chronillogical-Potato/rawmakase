//! Parameter names and units shared by application command adapters.
use crate::app::inspector::BANDS;
use crate::app::widgets::slider_text;
use crate::develop::{Recipe, TEMPERATURE_MAX, TEMPERATURE_MIN, TINT_LIMIT};

/// The Angle slider's limit either way, in degrees, as `Recipe::validate` allows.
const STRAIGHTEN_LIMIT: f32 = 45.;
/// Degrees a dial tick or `turn` step moves the Angle: fine enough to level a horizon.
const STRAIGHTEN_TICK: f32 = 0.1;

/// The Color Mixer's channels, in the order of `Recipe::hsl`.
const MIXER_CHANNELS: [&str; 3] = ["Hue", "Saturation", "Luminance"];

/// A slider a dial can turn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::app) enum Param {
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
    /// A Color Mixer colour band (0 Red .. 7 Magenta), on the channel the
    /// panel's Hue / Sat / Lum selector shows.
    Band(usize),
    /// One channel (0 Hue, 1 Saturation, 2 Luminance) of a Color Mixer band,
    /// whichever channel the panel shows.
    Hsl(usize, usize),
    /// A band's gray mix, which Black & White uses in place of the mixer.
    Gray(usize),
}
impl Param {
    /// The sliders with a name of their own, as `midi.json` and the control
    /// socket spell them.
    pub(in crate::app) const NAMED: [(&'static str, Self); 14] = [
        ("exposure", Self::Exposure),
        ("contrast", Self::Contrast),
        ("highlights", Self::Highlights),
        ("shadows", Self::Shadows),
        ("whites", Self::Whites),
        ("blacks", Self::Blacks),
        ("texture", Self::Texture),
        ("clarity", Self::Clarity),
        ("dehaze", Self::Dehaze),
        ("vibrance", Self::Vibrance),
        ("saturation", Self::Saturation),
        ("temperature", Self::Temperature),
        ("tint", Self::Tint),
        ("straighten", Self::Straighten),
    ];
    /// `exposure`, `band3` (the channel the panel shows), `band3.sat` or
    /// `band3.gray`; bands count from 1 (Red). `temp` and `angle` are aliases.
    pub(in crate::app) fn parse(name: &str) -> Option<Self> {
        let name = name.to_ascii_lowercase();
        match name.as_str() {
            "temp" => return Some(Self::Temperature),
            "angle" => return Some(Self::Straighten),
            _ => {}
        }
        if let Some((_, param)) = Self::NAMED.iter().find(|(n, _)| *n == name) {
            return Some(*param);
        }
        let (band, channel) = match name.strip_prefix("band")?.split_once('.') {
            Some((band, channel)) => (band, Some(channel)),
            None => (name.strip_prefix("band")?, None),
        };
        let i = match band.parse::<usize>() {
            Ok(n) if (1..=8).contains(&n) => n - 1,
            _ => return None,
        };
        Some(match channel {
            None => Self::Band(i),
            Some("hue") => Self::Hsl(i, 0),
            Some("sat" | "saturation") => Self::Hsl(i, 1),
            Some("lum" | "luminance") => Self::Hsl(i, 2),
            Some("gray" | "grey") => Self::Gray(i),
            Some(_) => return None,
        })
    }
    /// The name `parse` reads back: `exposure`, `band3`, `band3.sat`.
    pub(in crate::app) fn spec(self) -> String {
        if let Some((name, _)) = Self::NAMED.iter().find(|(_, p)| *p == self) {
            return (*name).into();
        }
        match self {
            Self::Band(i) => format!("band{}", i + 1),
            Self::Hsl(i, c) => format!("band{}.{}", i + 1, ["hue", "sat", "lum"][c]),
            Self::Gray(i) => format!("band{}.gray", i + 1),
            _ => unreachable!("every other slider has a name"),
        }
    }
    /// The name the slider and its History step carry.
    pub(in crate::app) fn label(self, channel: usize) -> String {
        match self {
            Self::Band(i) => return format!("{} {}", BANDS[i], MIXER_CHANNELS[channel]),
            Self::Hsl(i, c) => return format!("{} {}", BANDS[i], MIXER_CHANNELS[c]),
            Self::Gray(i) => return format!("{} Gray", BANDS[i]),
            _ => {}
        }
        match self {
            Self::Exposure => "Exposure",
            Self::Contrast => "Contrast",
            Self::Highlights => "Highlights",
            Self::Shadows => "Shadows",
            Self::Whites => "Whites",
            Self::Blacks => "Blacks",
            Self::Texture => "Texture",
            Self::Clarity => "Clarity",
            Self::Dehaze => "Dehaze",
            Self::Vibrance => "Vibrance",
            Self::Saturation => "Saturation",
            Self::Temperature => "Temp",
            Self::Tint => "Tint",
            Self::Straighten => "Angle",
            Self::Band(_) | Self::Hsl(..) | Self::Gray(_) => unreachable!(),
        }
        .to_string()
    }
    pub(in crate::app) fn value(self, r: &mut Recipe, channel: usize) -> &mut f32 {
        match self {
            // Black & White swaps the mixer for one gray mix per band.
            Self::Band(i) if r.effects.monochrome => &mut r.effects.gray_mix[i],
            Self::Band(i) => &mut r.hsl[i][channel],
            Self::Hsl(i, c) => &mut r.hsl[i][c],
            Self::Gray(i) => &mut r.effects.gray_mix[i],
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
        }
    }
    pub(in crate::app) fn is_white_balance(self) -> bool {
        matches!(self, Self::Temperature | Self::Tint)
    }
    /// The slider's number as it shows: EV, kelvin, tint units or degrees, else
    /// -100..100.
    pub(in crate::app) fn shown(self, r: &mut Recipe, channel: usize) -> f64 {
        let scale = match self {
            Self::Exposure | Self::Temperature | Self::Tint | Self::Straighten => 1.,
            _ => 100.,
        };
        (f64::from(*self.value(r, channel)) * scale * 1000.).round() / 1000.
    }
    /// Sets the slider to `shown`, a number as `shown` returns it, and returns
    /// the text for the History step.
    pub(in crate::app) fn set(self, r: &mut Recipe, shown: f32, channel: usize) -> String {
        let v = self.value(r, channel);
        match self {
            Self::Exposure => {
                *v = shown.clamp(-5., 5.);
                slider_text(f64::from(*v), 2, true)
            }
            Self::Temperature => {
                *v = shown.clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
                slider_text(f64::from(*v), 0, false)
            }
            Self::Tint => {
                *v = shown.clamp(-TINT_LIMIT, TINT_LIMIT);
                slider_text(f64::from(*v), 0, true)
            }
            Self::Straighten => {
                *v = shown.clamp(-STRAIGHTEN_LIMIT, STRAIGHTEN_LIMIT);
                slider_text(f64::from(*v), 2, true)
            }
            _ => {
                *v = (shown / 100.).clamp(-1., 1.);
                slider_text(f64::from(*v * 100.), 0, true)
            }
        }
    }
    /// Moves the slider by `ticks` (clockwise positive) and returns the value as
    /// the slider shows it, for the History step.
    pub(in crate::app) fn turn(self, r: &mut Recipe, ticks: i32, channel: usize) -> String {
        let t = ticks as f32;
        let v = self.value(r, channel);
        match self {
            Self::Exposure => {
                *v = (*v + 0.02 * t).clamp(-5., 5.);
                slider_text(f64::from(*v), 2, true)
            }
            // Evenly in mireds, as the slider does; clockwise is warmer.
            Self::Temperature => {
                let mired = (1e6 / *v - 4. * t).max(1e6 / TEMPERATURE_MAX);
                *v = (1e6 / mired).clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
                slider_text(f64::from(*v), 0, false)
            }
            Self::Tint => {
                *v = (*v + t).clamp(-TINT_LIMIT, TINT_LIMIT);
                slider_text(f64::from(*v), 0, true)
            }
            Self::Straighten => {
                *v = (*v + STRAIGHTEN_TICK * t).clamp(-STRAIGHTEN_LIMIT, STRAIGHTEN_LIMIT);
                slider_text(f64::from(*v), 2, true)
            }
            _ => {
                *v = (*v + 0.01 * t).clamp(-1., 1.);
                slider_text(f64::from(*v * 100.), 0, true)
            }
        }
    }
}

impl Param {
    pub(in crate::app) fn all() -> Vec<(String, Self)> {
        Self::NAMED
            .iter()
            .map(|(n, p)| ((*n).into(), *p))
            .chain((0..8).flat_map(|i| {
                (0..4).map(move |c| {
                    let p = if c == 3 {
                        Self::Gray(i)
                    } else {
                        Self::Hsl(i, c)
                    };
                    (p.spec(), p)
                })
            }))
            .collect()
    }
    pub(in crate::app) fn range(self, mask: bool) -> (f32, f32) {
        match self {
            Self::Exposure if mask => (-4., 4.),
            Self::Exposure => (-5., 5.),
            Self::Temperature if !mask => (TEMPERATURE_MIN, TEMPERATURE_MAX),
            Self::Tint if !mask => (-TINT_LIMIT, TINT_LIMIT),
            Self::Straighten => (-STRAIGHTEN_LIMIT, STRAIGHTEN_LIMIT),
            _ => (-100., 100.),
        }
    }
    /// Preserve both endpoints and the neutral centre of bipolar controls.
    pub(in crate::app) fn control_value(self, value: u8, mask: bool) -> f32 {
        let (min, max) = self.range(mask);
        let value = f32::from(value.min(127));
        if min < 0. && max > 0. {
            if value <= 64. {
                min * (64. - value) / 64.
            } else {
                max * (value - 64.) / 63.
            }
        } else {
            min + (max - min) * value / 127.
        }
    }
    pub(in crate::app) fn capabilities() -> Vec<super::reply::Parameter> {
        let items: Vec<_> = Self::all()
            .into_iter()
            .map(|(name, param)| {
                let (min, max) = param.range(false);
                let (mask_min, mask_max) = param.range(true);
                let unit = match param {
                    Self::Exposure => "EV",
                    Self::Temperature => "kelvin",
                    Self::Tint => "tint",
                    Self::Straighten => "degrees",
                    _ => "percent",
                };
                let local = param
                    .local_shown(&crate::develop::masks::LocalAdjust::default())
                    .is_some();
                super::reply::Parameter {
                    name,
                    unit,
                    min,
                    max,
                    mask: local,
                    mask_unit: if param == Self::Exposure {
                        "EV"
                    } else {
                        "percent"
                    },
                    mask_min,
                    mask_max,
                }
            })
            .collect();
        items
    }
    fn local_value(self, a: &mut crate::develop::masks::LocalAdjust) -> Option<&mut f32> {
        Some(match self {
            Self::Exposure => &mut a.exposure,
            Self::Temperature => &mut a.temperature,
            Self::Tint => &mut a.tint,
            Self::Contrast => &mut a.contrast,
            Self::Highlights => &mut a.highlights,
            Self::Shadows => &mut a.shadows,
            Self::Whites => &mut a.whites,
            Self::Blacks => &mut a.blacks,
            Self::Texture => &mut a.texture,
            Self::Clarity => &mut a.clarity,
            Self::Dehaze => &mut a.dehaze,
            Self::Saturation => &mut a.saturation,
            _ => return None,
        })
    }
    pub(in crate::app) fn local_shown(self, a: &crate::develop::masks::LocalAdjust) -> Option<f32> {
        let mut copy = *a;
        self.local_value(&mut copy)
            .map(|v| *v * if self == Self::Exposure { 1. } else { 100. })
    }
    pub(in crate::app) fn local_set(
        self,
        a: &mut crate::develop::masks::LocalAdjust,
        shown: Option<f32>,
        ticks: i32,
    ) -> super::Result<String> {
        let v = self.local_value(a).ok_or_else(|| {
            super::Error::new(
                "unsupported_parameter",
                "This parameter is not available on masks",
            )
        })?;
        let exposure = self == Self::Exposure;
        let scale = if exposure { 1. } else { 100. };
        let limit = if exposure { 4. } else { 1. };
        *v = shown
            .map_or(
                *v + ticks as f32 * if exposure { 0.02 } else { 0.01 },
                |n| n / scale,
            )
            .clamp(-limit, limit);
        Ok(slider_text(
            f64::from(*v * scale),
            if exposure { 2 } else { 0 },
            true,
        ))
    }
}
