//! Fallback white balance for cameras without calibrated DCP matrices, and Lightroom's
//! named white balance presets.
use crate::{
    camera_data::Metadata,
    color::{inverse, mul},
};
pub fn illuminant_camera(t: f32, m: &Metadata) -> [f32; 3] {
    // Planckian locus approximation in CIE xy; extrapolation avoided by UI range.
    let t = t.clamp(2000., 15000.);
    let x = if t <= 4000. {
        -0.2661239e9 / t.powi(3) - 0.234358e6 / t.powi(2) + 0.8776956e3 / t + 0.179910
    } else {
        -3.0258469e9 / t.powi(3) + 2.107038e6 / t.powi(2) + 0.2226347e3 / t + 0.240390
    };
    let y = if t <= 2222. {
        -1.1063814 * x.powi(3) - 1.3481102 * x * x + 2.1855583 * x - 0.2021968
    } else if t <= 4000. {
        -0.9549476 * x.powi(3) - 1.3741859 * x * x + 2.091_37 * x - 0.1674887
    } else {
        3.081758 * x.powi(3) - 5.873_387 * x * x + 3.75113 * x - 0.3700148
    };
    let rgb = mul(
        [
            [3.2404542, -1.5371385, -0.4985314],
            [-0.969266, 1.8760108, 0.041556],
            [0.0556434, -0.2040259, 1.0572252],
        ],
        [x / y, 1., (1. - x - y) / y],
    );
    let balanced = mul(inverse(m.matrix), rgb);
    std::array::from_fn(|c| balanced[c] / m.daylight_wb[c])
}
pub fn estimate_temperature(m: &Metadata) -> f32 {
    (2000..=15000)
        .step_by(50)
        .min_by(|a, b| {
            let error = |t: i32| {
                let p = illuminant_camera(t as f32, m);
                (0..3)
                    .map(|c| (p[1] / p[c].max(0.01) / m.wb[c]).ln().powi(2))
                    .sum::<f32>()
            };
            error(*a).total_cmp(&error(*b))
        })
        .unwrap_or(6500) as f32
}

/// Lightroom's named white balance presets for RAW files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamedWhiteBalance {
    Daylight,
    Cloudy,
    Shade,
    Tungsten,
    Fluorescent,
    Flash,
}

/// A white balance as Temperature (kelvin) and Tint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TemperatureTint {
    pub temperature: f32,
    pub tint: f32,
}

impl NamedWhiteBalance {
    pub const ALL: [NamedWhiteBalance; 6] = [
        NamedWhiteBalance::Daylight,
        NamedWhiteBalance::Cloudy,
        NamedWhiteBalance::Shade,
        NamedWhiteBalance::Tungsten,
        NamedWhiteBalance::Fluorescent,
        NamedWhiteBalance::Flash,
    ];
    /// The menu name, also Lightroom's `WhiteBalance` value.
    pub fn name(self) -> &'static str {
        match self {
            NamedWhiteBalance::Daylight => "Daylight",
            NamedWhiteBalance::Cloudy => "Cloudy",
            NamedWhiteBalance::Shade => "Shade",
            NamedWhiteBalance::Tungsten => "Tungsten",
            NamedWhiteBalance::Fluorescent => "Fluorescent",
            NamedWhiteBalance::Flash => "Flash",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|w| w.name() == name)
    }
    /// The values Lightroom sets for RAW files.
    pub fn values(self) -> TemperatureTint {
        let (temperature, tint) = match self {
            NamedWhiteBalance::Daylight => (5500., 10.),
            NamedWhiteBalance::Cloudy => (6500., 10.),
            NamedWhiteBalance::Shade => (7500., 10.),
            NamedWhiteBalance::Tungsten => (2850., 0.),
            NamedWhiteBalance::Fluorescent => (3800., 21.),
            NamedWhiteBalance::Flash => (5500., 0.),
        };
        TemperatureTint { temperature, tint }
    }
}

/// log2 linear-RGB gains of relative Temp +100 and Tint +100: Lightroom's white
/// balance for rendered files, and masks' Temp and Tint without a profile model.
pub const RELATIVE_WHITE_BALANCE: [[f32; 3]; 2] = [[0.25, 0., -0.3], [0.12, -0.2, 0.12]];

/// Gains for relative Temp and Tint in −100..=100.
pub fn relative_gains(temperature: f32, tint: f32) -> [f32; 3] {
    let [t, n] = RELATIVE_WHITE_BALANCE;
    std::array::from_fn(|c| (temperature / 100. * t[c] + tint / 100. * n[c]).exp2())
}

/// The relative Temp and Tint whose gains are nearest to `gains` (least squares
/// in log2, after normalising to green), clamped to −100..=100.
pub fn relative_controls(gains: [f32; 3]) -> [f32; 2] {
    // Gains matter only relative to green: normalise to green, then solve the 2x2
    // normal equations of min |a*t + b*n - target|^2 over red and blue, where t and
    // n are the table's rows minus their green entry and a, b are controls / 100.
    let g = gains[1].max(1e-6);
    let target: [f32; 3] = std::array::from_fn(|c| (gains[c].max(1e-6) / g).log2());
    let [t, n] = RELATIVE_WHITE_BALANCE;
    let row = |v: [f32; 3]| [v[0] - v[1], v[2] - v[1]];
    let (t, n, y) = (row(t), row(n), [target[0], target[2]]);
    let dot = |a: [f32; 2], b: [f32; 2]| a[0] * b[0] + a[1] * b[1];
    let det = dot(t, t) * dot(n, n) - dot(t, n) * dot(t, n);
    let (a, b) = if det.abs() < 1e-9 {
        (0., 0.)
    } else {
        (
            (dot(t, y) * dot(n, n) - dot(n, y) * dot(t, n)) / det,
            (dot(n, y) * dot(t, t) - dot(t, y) * dot(t, n)) / det,
        )
    };
    [a, b].map(|v| (v * 100.).clamp(-100., 100.))
}

#[cfg(test)]
mod relative_tests {
    use super::*;
    use crate::camera_data::{Metadata, Source};
    use crate::model::recipe::Recipe;

    #[test]
    fn relative_white_balance_round_trips_through_gains() {
        for (t, n) in [(0., 0.), (30., 0.), (-60., 25.), (100., -100.)] {
            let back = relative_controls(relative_gains(t, n));
            assert!(
                (back[0] - t).abs() < 0.5 && (back[1] - n).abs() < 0.5,
                "{t},{n} -> {back:?}"
            );
        }
        assert_eq!(relative_gains(0., 0.), [1.; 3]);
    }

    #[test]
    fn a_rendered_photo_keeps_white_balance_relative() {
        let m = Metadata {
            source: Source::Rendered,
            ..Default::default()
        };
        let mut r = Recipe::for_rendered(&m);
        r.incremental_temperature = 40.;
        r.update_wb(&m);
        assert!(r.wb[0] > 1. && r.wb[2] < 1.);
        assert_eq!((r.temperature, r.tint), (6500., 0.));
        r.sync_white_balance_controls(&m);
        assert!((r.incremental_temperature - 40.).abs() < 0.5);
        r.reset_white_balance(&m);
        assert_eq!(
            (r.incremental_temperature, r.incremental_tint, r.wb),
            (0., 0., [1.; 3])
        );
    }

    #[test]
    fn picked_gains_on_a_rendered_photo_become_those_its_controls_show() {
        let m = Metadata {
            source: Source::Rendered,
            ..Default::default()
        };
        let mut r = Recipe::for_rendered(&m);
        // An eyedropper's gains, off the relative model's plane (and green ≠ 1).
        r.wb = [1.4, 0.9, 1.2];
        r.sync_white_balance_controls(&m);
        let shown = relative_gains(r.incremental_temperature, r.incremental_tint);
        assert_eq!(r.wb, shown);
        // So the next Temp edit moves on from them rather than jumping.
        let mut moved = r.clone();
        moved.update_wb(&m);
        assert_eq!(moved.wb, r.wb);
    }

    #[test]
    fn a_raw_recipe_saves_without_relative_white_balance() {
        let text = serde_json::to_string(&Recipe::default()).unwrap();
        assert!(!text.contains("incremental"));
    }

    #[test]
    fn relative_white_balance_is_validated_for_every_photo() {
        let r = Recipe {
            incremental_tint: 101.,
            ..Recipe::default()
        };
        assert!(r.validate().is_err());
    }
}
