//! Lightroom's Point Color (Color Mixer › Point Color): up to eight sampled colors,
//! each with Hue, Saturation and Luminance shifts, a Variance and a selection range.
//!
//! Measured with Camera Raw 18.7 on synthetic charts (docs/color-mixer.md). Camera Raw
//! works in HSV of linear ProPhoto RGB, after the tone curves, where the color mixer
//! works: a swatch's hue is the HSV hue in sixths of a turn (0–6), its saturation the
//! HSV saturation and its luminance the HSV value. The saturation range is on HSV
//! saturation and the luminance range on sRGB-encoded value. Each range is a trapezoid
//! (outer and inner points), smoothed; Range scales the distance from the swatch before
//! the trapezoids are applied. A pixel's weight is the product of its hue, saturation
//! and luminance weights, faded out towards neutral. Within the selection, Hue turns
//! the HSV hue (±35° at ±100), Saturation and Luminance scale HSV saturation and value,
//! and Variance spreads or gathers hues, saturations and values around the swatch.
//! Swatches apply in turn, each to the result of the ones before.
use crate::color_math::{srgb_decode, srgb_encode};
use serde::{Deserialize, Serialize};
use std::f32::consts::TAU;

/// Camera Raw keeps at most eight swatches ("Only 8 samples are supported").
pub const MAX_SWATCHES: usize = 8;

/// One Point Color swatch, in Camera Raw's stored units (`crs:PointColors` and
/// `crs:ColorVariance`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PointColor {
    /// The sampled color: HSV hue in sixths of a turn (0–6), saturation (0–1) and
    /// value (linear, 0–1) of linear ProPhoto RGB.
    pub source: [f32; 3],
    /// Hue, Saturation and Luminance shifts, −1 to 1 (Lightroom's −100 to 100).
    pub shift: [f32; 3],
    /// Range, 0–1 (Lightroom's 0–100; 50 by default).
    pub range: f32,
    /// Hue range: outer and inner points, 0–1 across the swatch's hue window with the
    /// swatch at 0.5.
    pub hue_range: [f32; 4],
    /// Saturation range: outer and inner points on HSV saturation.
    pub saturation_range: [f32; 4],
    /// Luminance range: outer and inner points on sRGB-encoded HSV value.
    pub luminance_range: [f32; 4],
    /// Variance, −1 to 1 (Lightroom's −100 to 100).
    #[serde(default)]
    pub variance: f32,
    /// How the swatch renders: its adjustment, or (while Visualize Range is on, never
    /// saved) its selection.
    #[serde(skip)]
    pub view: SwatchView,
}

/// Point Color's Visualize Range: the selected swatch shows which colors it selects,
/// in color, with everything else gray.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SwatchView {
    #[default]
    Adjust,
    VisualizeRange,
}

/// Swatches for a Visualize Range render of the one at `index`: the swatches before it
/// as they are, then its selection. `None` when there is no such swatch.
pub fn visualize_range(list: &[PointColor], index: usize) -> Option<Vec<PointColor>> {
    let selected = list.get(index)?;
    let mut out = list[..index].to_vec();
    out.push(PointColor {
        view: SwatchView::VisualizeRange,
        ..*selected
    });
    Some(out)
}

impl PointColor {
    /// A swatch sampled from a color, with Lightroom's default ranges and no shifts.
    pub fn sampled(source: [f32; 3]) -> Self {
        let around = |c: f32| [c - 0.73, c - 0.18, c + 0.18, c + 0.73].map(|v| v.clamp(0., 1.));
        Self {
            source,
            shift: [0.; 3],
            range: 0.5,
            hue_range: [0., 1. / 3., 2. / 3., 1.],
            saturation_range: around(source[1]),
            luminance_range: around(srgb_encode(source[2])),
            variance: 0.,
            view: SwatchView::Adjust,
        }
    }
    /// The sampled color as linear ProPhoto RGB.
    pub fn source_prophoto(&self) -> [f32; 3] {
        let [h, s, v] = self.source;
        hsv_to_rgb(h / 6. * TAU, s, v)
    }
    /// Whether Camera Raw accepts the swatch. It drops a swatch whose values are out
    /// of range, whose hue range has no inner span, or whose saturation or luminance
    /// range does not hold the sampled color between its inner points.
    pub fn is_valid(&self) -> bool {
        let [h, s, v] = self.source;
        let ordered = |r: &[f32; 4]| {
            r.iter().all(|x| (0. ..=1.).contains(x)) && r.windows(2).all(|w| w[0] <= w[1])
        };
        let holds = |r: &[f32; 4], x: f32| r[1] <= x + 1e-4 && x <= r[2] + 1e-4;
        (0. ..6.).contains(&h)
            && (0. ..=1.).contains(&s)
            && (0. ..=1.).contains(&v)
            && self.shift.iter().all(|x| (-1. ..=1.).contains(x))
            && (0. ..=1.).contains(&self.range)
            && (-1. ..=1.).contains(&self.variance)
            && ordered(&self.hue_range)
            && self.hue_range[1] < self.hue_range[2]
            && ordered(&self.saturation_range)
            && ordered(&self.luminance_range)
            && holds(&self.saturation_range, s)
            && holds(&self.luminance_range, srgb_encode(v))
    }
    /// Whether the swatch changes anything.
    pub fn is_active(&self) -> bool {
        self.shift != [0.; 3] || self.variance != 0.
    }
}

/// Why the dropper did not add a swatch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleRefusal {
    /// Saturation below where every selection fades out.
    TooNeutral,
    TooDark,
    /// A swatch of this color is already there.
    AlreadySampled,
    /// Eight swatches already.
    Full,
}

impl SampleRefusal {
    /// What the status line says.
    pub fn message(self) -> &'static str {
        match self {
            Self::TooNeutral => "Too neutral to sample: click a more colorful area",
            Self::TooDark => "Too dark to sample: click a brighter area",
            Self::AlreadySampled => "That color already has a swatch",
            Self::Full => "Point Color holds up to 8 swatches",
        }
    }
}

/// Adds a swatch of `source` (as `point_color_pick` gives it) with the default
/// ranges, and returns its index.
pub fn add_sample(list: &mut Vec<PointColor>, source: [f32; 3]) -> Result<usize, SampleRefusal> {
    let [h, s, v] = source;
    if list.len() >= MAX_SWATCHES {
        return Err(SampleRefusal::Full);
    }
    if s < fit::NEUTRAL {
        return Err(SampleRefusal::TooNeutral);
    }
    if srgb_encode(v) < TOO_DARK {
        return Err(SampleRefusal::TooDark);
    }
    let close = |p: &PointColor| {
        let dh = (p.source[0] - h).rem_euclid(6.);
        dh.min(6. - dh) < 0.05
            && (p.source[1] - s).abs() < 0.02
            && (srgb_encode(p.source[2]) - srgb_encode(v)).abs() < 0.02
    };
    if list.iter().any(close) {
        return Err(SampleRefusal::AlreadySampled);
    }
    list.push(PointColor::sampled([h.rem_euclid(6.).min(5.9999), s, v]));
    Ok(list.len() - 1)
}

/// Encoded value below which the dropper refuses a color as too dark.
const TOO_DARK: f32 = 0.06;

/// Separates swatches (and variances) in the text form of `crs:PointColors` and
/// `crs:ColorVariance` that XMP and catalog imports pass on as one setting each.
pub const LIST_SEPARATOR: &str = "; ";

/// Reads swatches from that text form: 19 comma-separated values per swatch, and one
/// variance per swatch (0 where missing). Lightroom's empty selection (all -1) and
/// swatches Camera Raw would drop are left out; malformed numbers are an error.
pub fn parse_list(points: &str, variances: Option<&str>) -> anyhow::Result<Vec<PointColor>> {
    use anyhow::{Context, ensure};
    let numbers = |text: &str| -> anyhow::Result<Vec<f32>> {
        text.split(',')
            .filter(|v| !v.trim().is_empty())
            .map(|v| {
                let x: f32 = v
                    .trim()
                    .parse()
                    .with_context(|| format!("Invalid Point Color value {v}"))?;
                ensure!(x.is_finite(), "Non-finite Point Color value");
                Ok(x)
            })
            .collect()
    };
    let variances: Vec<f32> = variances
        .map(|v| {
            v.split(';')
                .map(|x| numbers(x).map(|n| n.first().copied().unwrap_or(0.)))
                .collect()
        })
        .transpose()?
        .unwrap_or_default();
    let mut out = Vec::new();
    for (i, entry) in points
        .split(';')
        .filter(|e| !e.trim().is_empty())
        .enumerate()
    {
        let v = numbers(entry)?;
        ensure!(
            v.len() == 19,
            "A Point Color swatch has {} values, not 19",
            v.len()
        );
        if v.iter().all(|x| *x == -1.) {
            continue;
        }
        let p = PointColor {
            source: [v[0], v[1], v[2]],
            shift: [v[3], v[4], v[5]],
            range: v[6],
            hue_range: [v[7], v[8], v[9], v[10]],
            saturation_range: [v[11], v[12], v[13], v[14]],
            luminance_range: [v[15], v[16], v[17], v[18]],
            variance: variances.get(i).copied().unwrap_or(0.),
            view: SwatchView::Adjust,
        };
        if p.is_valid() && out.len() < MAX_SWATCHES {
            out.push(p);
        }
    }
    Ok(out)
}

/// Swatches as Camera Raw writes them: one `crs:PointColors` item and one
/// `crs:ColorVariance` item each.
pub struct ListText {
    pub points: Vec<String>,
    pub variances: Vec<String>,
}

/// The text form of `parse_list`, with six decimals as Camera Raw writes it.
pub fn format_list(list: &[PointColor]) -> ListText {
    let fixed = |x: &f32| format!("{x:.6}");
    let (points, variances) = list
        .iter()
        .map(|p| {
            let values: Vec<String> = p
                .source
                .iter()
                .chain(&p.shift)
                .chain(std::iter::once(&p.range))
                .chain(&p.hue_range)
                .chain(&p.saturation_range)
                .chain(&p.luminance_range)
                .map(fixed)
                .collect();
            (values.join(", "), fixed(&p.variance))
        })
        .unzip();
    ListText { points, variances }
}

/// Half the hue window at Range 50, in radians: where the hue range's outer points
/// sit by default.
pub const HUE_WINDOW: f32 = fit::HUE_WINDOW;

/// Fitted constants (docs/color-mixer.md#point-color).
mod fit {
    /// Half the hue window at Range 50, in radians.
    pub const HUE_WINDOW: f32 = 0.7236;
    /// How Range scales the distance from the swatch, per dimension (hue, saturation,
    /// luminance): 2^(k · (0.5 − Range)), with k below and above Range 50.
    pub const RANGE_BELOW: [f32; 3] = [0.767, 1.7637, 0.5482];
    pub const RANGE_ABOVE: [f32; 3] = [0.1186, 0.3886, 0.0598];
    /// Exponents of the smoothed ramps: hue, then saturation and luminance rising and
    /// falling.
    pub const HUE_RAMP: f32 = 1.0282;
    pub const SATURATION_RAMP: [f32; 2] = [1.8348, 0.9897];
    pub const LUMINANCE_RAMP: [f32; 2] = [2.7765, 1.1354];
    /// Saturation below which pixels fade out of every selection.
    pub const NEUTRAL: f32 = 0.0578;
    /// Hue turn at Hue ±100, radians (35°).
    pub const HUE_SHIFT: f32 = 0.6109;
    /// Saturation ±100 multiplies or divides saturation by 1.9.
    pub const SATURATION_SHIFT: f32 = 0.9;
    /// Luminance: log2 of the value factor is k1·|x| + k2·x².
    pub const LUMINANCE_SHIFT: [f32; 2] = [1.3932, -0.2366];
    /// Variance ±100 scales distances from the swatch in saturation and encoded value
    /// by 2^±k (hue distances by 1 ± Variance).
    pub const VARIANCE_SATURATION: f32 = 0.7904;
    pub const VARIANCE_LUMINANCE: f32 = 0.2123;
}

/// A swatch prepared for rendering. `params` is the flattened form the GPU port reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Swatch {
    hue: f32,
    saturation: f32,
    luminance: f32,
    scale: [f32; 3],
    hue_points: [f32; 4],
    saturation_points: [f32; 4],
    luminance_points: [f32; 4],
    hue_turn: f32,
    log_saturation: f32,
    log_value: f32,
    variance: f32,
    view: SwatchView,
}

/// Length of one swatch in `PointColors::params`.
pub(crate) const SWATCH_PARAMS: usize = 23;
/// Length of the fitted constants that precede the swatches in `PointColors::params`.
pub(crate) const CONSTANT_PARAMS: usize = 8;

impl Swatch {
    fn new(p: &PointColor) -> Self {
        let scale = std::array::from_fn(|d| {
            let k = if p.range < 0.5 {
                fit::RANGE_BELOW[d]
            } else {
                fit::RANGE_ABOVE[d]
            };
            (k * (0.5 - p.range)).exp2()
        });
        let [dh, ds, dl] = p.shift;
        let l = dl.abs();
        Self {
            hue: p.source[0] / 6. * TAU,
            saturation: p.source[1],
            luminance: srgb_encode(p.source[2]),
            scale,
            hue_points: p.hue_range.map(|x| (x - 0.5) * 2. * fit::HUE_WINDOW),
            saturation_points: p.saturation_range,
            luminance_points: p.luminance_range,
            hue_turn: dh * fit::HUE_SHIFT,
            log_saturation: (1. + fit::SATURATION_SHIFT * ds.abs()).log2() * ds.signum(),
            log_value: (fit::LUMINANCE_SHIFT[0] * l + fit::LUMINANCE_SHIFT[1] * l * l)
                * dl.signum(),
            variance: p.variance,
            view: p.view,
        }
    }
    fn params(&self) -> [f32; SWATCH_PARAMS] {
        let mut out = [0.; SWATCH_PARAMS];
        out[0] = self.hue;
        out[1] = self.saturation;
        out[2] = self.luminance;
        out[3..6].copy_from_slice(&self.scale);
        out[6..10].copy_from_slice(&self.hue_points);
        out[10..14].copy_from_slice(&self.saturation_points);
        out[14..18].copy_from_slice(&self.luminance_points);
        out[18] = self.hue_turn;
        out[19] = self.log_saturation;
        out[20] = self.log_value;
        out[21] = self.variance;
        out[22] = match self.view {
            SwatchView::Adjust => 0.,
            SwatchView::VisualizeRange => 1.,
        };
        out
    }
}

/// The active swatches of a recipe, ready to apply.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PointColors {
    swatches: Vec<Swatch>,
}

impl PointColors {
    pub(crate) fn new(list: &[PointColor]) -> Option<Self> {
        let swatches: Vec<_> = list
            .iter()
            .take(MAX_SWATCHES)
            .filter(|p| p.is_valid() && (p.is_active() || p.view == SwatchView::VisualizeRange))
            .map(Swatch::new)
            .collect();
        (!swatches.is_empty()).then_some(Self { swatches })
    }
    /// Flattened for the GPU: the fitted constants (`CONSTANT_PARAMS`), then
    /// `SWATCH_PARAMS` values per swatch.
    pub(crate) fn params(&self) -> Vec<f32> {
        let constants = [
            fit::HUE_RAMP,
            fit::SATURATION_RAMP[0],
            fit::SATURATION_RAMP[1],
            fit::LUMINANCE_RAMP[0],
            fit::LUMINANCE_RAMP[1],
            fit::NEUTRAL,
            fit::VARIANCE_SATURATION,
            fit::VARIANCE_LUMINANCE,
        ];
        debug_assert_eq!(constants.len(), CONSTANT_PARAMS);
        constants
            .into_iter()
            .chain(self.swatches.iter().flat_map(|s| s.params()))
            .collect()
    }
    pub(crate) fn len(&self) -> usize {
        self.swatches.len()
    }
    /// `p` is linear ProPhoto RGB. Swatches apply in turn, each to the result of
    /// the ones before, as Camera Raw's do: a second swatch selects the color the
    /// first has made.
    pub(crate) fn apply_prophoto(&self, p: [f32; 3]) -> [f32; 3] {
        self.swatches.iter().fold(p, |p, w| w.apply(p))
    }
}

impl Swatch {
    fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let q = p.map(|v| v.max(0.));
        let max = q.into_iter().fold(0f32, f32::max);
        if max <= 1e-6 {
            return p;
        }
        let [h, s, v] = rgb_to_hsv(q);
        let ev = srgb_encode(v.min(1.));
        let d = wrap(h - self.hue);
        let weight = smoothed(d * self.scale[0], &self.hue_points, [fit::HUE_RAMP; 2])
            * smoothed(
                self.saturation + (s - self.saturation) * self.scale[1],
                &self.saturation_points,
                fit::SATURATION_RAMP,
            )
            * smoothed(
                self.luminance + (ev - self.luminance) * self.scale[2],
                &self.luminance_points,
                fit::LUMINANCE_RAMP,
            )
            * (s / fit::NEUTRAL).min(1.);
        if self.view == SwatchView::VisualizeRange {
            let out = hsv_to_rgb(h, s * weight, v);
            return std::array::from_fn(|c| out[c] + (p[c] - q[c]));
        }
        if weight <= 0. {
            return p;
        }
        let turn = weight * (self.hue_turn + self.variance * d);
        let mut log_s = weight * self.log_saturation;
        let mut log_v = weight * self.log_value;
        if self.variance != 0. {
            let spread =
                |x: f32, c: f32, k: f32| (c + (x - c) * (k * self.variance).exp2()).clamp(1e-4, 1.);
            let s2 = spread(s, self.saturation, fit::VARIANCE_SATURATION);
            let v2 = srgb_decode(spread(ev, self.luminance, fit::VARIANCE_LUMINANCE));
            log_s += weight * (s2 / s.max(1e-4)).log2();
            log_v += weight * (v2 / srgb_decode(ev).max(1e-6)).log2();
        }
        let out = hsv_to_rgb(h + turn, (s * log_s.exp2()).clamp(0., 1.), v * log_v.exp2());
        // Channels clip at white: a brightened orange turns towards yellow rather
        // than past white. What HSV cannot carry (negative channels) stays as an
        // offset.
        std::array::from_fn(|c| out[c].min(q[c].max(1.)) + (p[c] - q[c]))
    }
}

/// Hue difference in radians, −π to π.
fn wrap(d: f32) -> f32 {
    (d + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3. - 2. * t)
}

/// A trapezoid over `x` with outer points `p[0]`, `p[3]` and inner points `p[1]`,
/// `p[2]`, its rising and falling ramps smoothed and raised to `ramp`.
fn smoothed(x: f32, p: &[f32; 4], ramp: [f32; 2]) -> f32 {
    let rise = if x >= p[1] {
        1.
    } else if x <= p[0] {
        0.
    } else {
        smoothstep((x - p[0]) / (p[1] - p[0])).powf(ramp[0])
    };
    let fall = if x <= p[2] {
        1.
    } else if x >= p[3] {
        0.
    } else {
        smoothstep((p[3] - x) / (p[3] - p[2])).powf(ramp[1])
    };
    rise.min(fall)
}

/// HSV with hue in radians.
pub(crate) fn rgb_to_hsv(p: [f32; 3]) -> [f32; 3] {
    let max = p.into_iter().fold(f32::NEG_INFINITY, f32::max);
    let min = p.into_iter().fold(f32::INFINITY, f32::min);
    let d = max - min;
    let sextant = if d <= 0. {
        0.
    } else if max == p[0] {
        ((p[1] - p[2]) / d).rem_euclid(6.)
    } else if max == p[1] {
        (p[2] - p[0]) / d + 2.
    } else {
        (p[0] - p[1]) / d + 4.
    };
    let s = if max > 0. { d / max } else { 0. };
    [sextant / 6. * TAU, s, max]
}

pub(crate) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h6 = (h / TAU).rem_euclid(1.) * 6.;
    let c = v * s;
    let x = c * (1. - (h6 % 2. - 1.).abs());
    let m = v - c;
    let [r, g, b] = match h6 as usize {
        0 => [c, x, 0.],
        1 => [x, c, 0.],
        2 => [0., c, x],
        3 => [0., x, c],
        4 => [x, 0., c],
        _ => [c, 0., x],
    };
    [r + m, g + m, b + m]
}

#[cfg(test)]
mod tests;
