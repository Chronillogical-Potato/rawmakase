//! RAWmakase's camera table, `data/cameras.toml`: per-model values that the raw
//! file does not carry, one row per camera body. See docs/cameras.md.
use serde::Deserialize;
use std::sync::OnceLock;

/// Where a row's values come from.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// Read from a DNG that Adobe software wrote for this camera.
    AdobeDng,
    /// Read from the camera's Adobe DCP.
    AdobeProfile,
    /// Fitted to Camera Raw renders.
    Fitted,
    /// Measured by hand, as the row's `how` says.
    Measured,
}

/// Whether Camera Raw moves a Fujifilm body's baseline with the raw's exposure
/// midpoint shift (DR200/DR400, extended ISO).
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExposureShift {
    /// Every body measured so far after the X-Trans III generation.
    #[default]
    Followed,
    /// One value whatever the shift.
    Ignored,
}

/// One camera body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    /// Make and model as LibRaw reports them.
    pub make: String,
    pub model: String,
    /// Other model names for the same body.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Exposure Camera Raw adds to an unedited photo with Adobe Standard, in EV.
    pub baseline_exposure: f32,
    /// Fujifilm only: see [`ExposureShift`].
    #[serde(default)]
    pub fujifilm_exposure_shift: ExposureShift,
    /// Fujifilm only: the body's exposure midpoint shift at DR100 and base ISO, which
    /// the row holds at, when it is not the sensor's usual one (-0.72 EV for X-Trans,
    /// 0 for Bayer); the GFX bodies record -0.49.
    #[serde(default)]
    pub fujifilm_dr100_shift: Option<f32>,
    pub source: Source,
    /// When the values were checked (YYYY-MM-DD) and on what.
    pub checked: String,
    pub sample: String,
    /// How a measured value was measured.
    #[serde(default)]
    pub how: String,
}

impl Camera {
    fn is(&self, model: &str) -> bool {
        self.model.eq_ignore_ascii_case(model)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(model))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Table {
    camera: Vec<Camera>,
}

const TABLE: &str = include_str!("../data/cameras.toml");

fn parse(text: &str) -> Result<Vec<Camera>, toml::de::Error> {
    toml::from_str::<Table>(text).map(|t| t.camera)
}

/// Every row of the table.
pub fn all() -> &'static [Camera] {
    static CAMERAS: OnceLock<Vec<Camera>> = OnceLock::new();
    // A unit test parses the table, so a malformed row cannot ship.
    CAMERAS.get_or_init(|| parse(TABLE).expect("data/cameras.toml"))
}

/// The row for a camera, matching make and model (or an alias) without case.
fn find_in<'a>(rows: &'a [Camera], make: &str, model: &str) -> Option<&'a Camera> {
    rows.iter()
        .find(|c| c.make.eq_ignore_ascii_case(make) && c.is(model))
}

/// Where a baseline exposure comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaselineOrigin {
    /// The camera's own row.
    Listed(Source),
    /// The median of the rows of the camera's make.
    MakeMedian,
    /// The median of every row: the make has none.
    TableMedian,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Baseline {
    pub ev: f32,
    pub origin: BaselineOrigin,
    pub exposure_shift: ExposureShift,
    pub dr100_shift: Option<f32>,
}

/// Camera Raw's baseline exposure for a camera. A camera without a row takes the
/// median of its make's rows, which predicts held-out cameras better than the median
/// of all rows (see docs/cameras.md); a make without rows takes that overall median.
pub fn baseline_exposure(make: &str, model: &str) -> Baseline {
    baseline_in(all(), make, model)
}

fn baseline_in(rows: &[Camera], make: &str, model: &str) -> Baseline {
    if let Some(c) = find_in(rows, make, model) {
        return Baseline {
            ev: c.baseline_exposure,
            origin: BaselineOrigin::Listed(c.source),
            exposure_shift: c.fujifilm_exposure_shift,
            dr100_shift: c.fujifilm_dr100_shift,
        };
    }
    let same_make = rows
        .iter()
        .filter(|c| c.make.eq_ignore_ascii_case(make))
        .map(|c| c.baseline_exposure);
    if let Some(ev) = median(same_make) {
        return Baseline {
            ev,
            origin: BaselineOrigin::MakeMedian,
            exposure_shift: ExposureShift::Followed,
            dr100_shift: None,
        };
    }
    Baseline {
        ev: median(rows.iter().map(|c| c.baseline_exposure)).unwrap_or(0.),
        origin: BaselineOrigin::TableMedian,
        exposure_shift: ExposureShift::Followed,
        dr100_shift: None,
    }
}

fn median(values: impl Iterator<Item = f32>) -> Option<f32> {
    let mut v: Vec<f32> = values.collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(f32::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_row_parses_and_is_complete() {
        let rows = parse(TABLE).unwrap();
        assert!(!rows.is_empty());
        let mut names = std::collections::BTreeSet::new();
        for c in &rows {
            let name = format!("{} {}", c.make, c.model);
            assert!(!c.make.is_empty() && !c.model.is_empty(), "{name}");
            assert!(c.baseline_exposure.abs() <= 3., "{name}");
            let d: Vec<&str> = c.checked.split('-').collect();
            assert!(
                d.len() == 3 && d.iter().all(|p| p.parse::<u32>().is_ok()) && d[0].len() == 4,
                "{name}: checked must be YYYY-MM-DD"
            );
            assert!(!c.sample.is_empty(), "{name}: say what it was checked on");
            assert!(
                c.source != Source::Measured || !c.how.is_empty(),
                "{name}: a measured row says how"
            );
            for model in std::iter::once(&c.model).chain(&c.aliases) {
                assert!(
                    names.insert(format!("{} {}", c.make, model).to_lowercase()),
                    "{} {model} is listed twice",
                    c.make
                );
            }
        }
    }
    #[test]
    fn unknown_fields_are_rejected() {
        let row = "[[camera]]\nmake = \"A\"\nmodel = \"B\"\nbaseline_exposure = 0.1\n\
                   source = \"fitted\"\nchecked = \"2026-10-05\"\nsample = \"s\"\n";
        assert!(parse(row).is_ok());
        assert!(parse(&format!("{row}baseline = 0.2\n")).is_err());
        assert!(parse(&row.replace("fitted", "guessed")).is_err());
    }
    fn row(make: &str, model: &str, ev: f32) -> Camera {
        Camera {
            make: make.into(),
            model: model.into(),
            aliases: vec![],
            baseline_exposure: ev,
            fujifilm_exposure_shift: ExposureShift::Followed,
            fujifilm_dr100_shift: None,
            source: Source::Fitted,
            checked: "2026-10-05".into(),
            sample: "test".into(),
            how: String::new(),
        }
    }
    #[test]
    fn unlisted_cameras_take_their_makes_median_then_the_tables() {
        let mut rows = vec![
            row("Sony", "A", 0.2),
            row("Sony", "B", 0.4),
            row("Sony", "C", 0.35),
            row("Canon", "D", 0.6),
        ];
        rows[0].aliases = vec!["Alpha".into()];
        let listed = baseline_in(&rows, "SONY", "alpha");
        assert_eq!(listed.ev, 0.2);
        assert_eq!(listed.origin, BaselineOrigin::Listed(Source::Fitted));
        let make = baseline_in(&rows, "Sony", "E");
        assert_eq!((make.ev, make.origin), (0.35, BaselineOrigin::MakeMedian));
        let table = baseline_in(&rows, "Pentax", "K-3");
        assert_eq!(
            (table.ev, table.origin),
            (0.375, BaselineOrigin::TableMedian)
        );
        assert_eq!(baseline_in(&[], "Pentax", "K-3").ev, 0.);
    }
}
