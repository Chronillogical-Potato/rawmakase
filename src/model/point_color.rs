//! Point Color swatches as a recipe stores them (Color Mixer › Point Color): up to
//! eight sampled colors, each with Hue, Saturation and Luminance shifts, a Variance
//! and a selection range, in Camera Raw's units, and their text form in XMP and
//! catalogs. How they select and change colors is `develop::point_color`'s.
use crate::color::{hsv::hsv_to_rgb, srgb_encode};
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

/// Swatches for a Visualize Range render of the one at `index`: every swatch as it
/// is, the selected one also showing its selection. `None` when there is no such
/// swatch.
pub fn visualize_range(list: &[PointColor], index: usize) -> Option<Vec<PointColor>> {
    list.get(index)?;
    let mut out = list.to_vec();
    out[index].view = SwatchView::VisualizeRange;
    Some(out)
}

/// The swatches as they adjust the photo, without Visualize Range.
pub fn without_visualization(list: &mut [PointColor]) {
    for p in list {
        p.view = SwatchView::Adjust;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swatches_keep_their_stored_form() {
        let mut p = PointColor::sampled([1.5, 0.6, 0.3]);
        p.shift = [0.25, -0.5, 0.];
        p.view = SwatchView::VisualizeRange;
        let json = serde_json::to_value(p).unwrap();
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "hue_range",
                "luminance_range",
                "range",
                "saturation_range",
                "shift",
                "source",
                "variance"
            ]
        );
        // Visualize Range is never saved, and older swatches had no Variance.
        let mut stored = json;
        stored.as_object_mut().unwrap().remove("variance");
        let back: PointColor = serde_json::from_value(stored).unwrap();
        assert_eq!(back.view, SwatchView::Adjust);
        assert_eq!(back.variance, 0.);
        assert_eq!(back.shift, p.shift);
        assert!(back.is_valid() && back.is_active());
    }

    #[test]
    fn the_text_form_reads_back_what_it_writes() {
        let mut p = PointColor::sampled([4.25, 0.4, 0.5]);
        p.variance = -0.3;
        let text = format_list(&[p, p]);
        let read = parse_list(
            &text.points.join(LIST_SEPARATOR),
            Some(&text.variances.join(LIST_SEPARATOR)),
        )
        .unwrap();
        assert_eq!(read.len(), 2);
        for r in read {
            assert!((r.source[0] - 4.25).abs() < 1e-6 && (r.variance + 0.3).abs() < 1e-6);
        }
        assert!(parse_list("1, 2", None).is_err());
        // Camera Raw keeps eight swatches.
        assert_eq!(MAX_SWATCHES, 8);
        assert!(parse_list(&["-1"; 19].join(", "), None).unwrap().is_empty());
    }
}
