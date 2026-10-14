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

/// Lightroom's Temp for a rendered file (`IncrementalTemperature`, −100…100) as the
/// DNG SDK's reciprocal temperature of the white it names and a drift of its DNG
/// tint: `[Temp, mired, tint drift]`. Measured on Lightroom Classic 15.6 (Camera Raw
/// 18.7) renders of synthetic JPEG charts, as the white [`camera_white_balance`] needs
/// to reproduce them (`scripts/corpus/rendered-white-balance.py`). At 0 it is the
/// file's D50 white; Temp moves only the temperature, except a drift of a few units of
/// tint at the coldest Temp.
const RENDERED_TEMPERATURE: [[f32; 3]; 27] = [
    [-100., 523.2, -2.3],
    [-90., 470., -0.7],
    [-75., 415.6, -0.3],
    [-60., 367.7, -0.3],
    [-50., 336.1, 0.],
    [-35., 291.7, 0.],
    [-25., 263.8, 0.],
    [-15., 237.3, 0.],
    [-10., 224.6, 0.],
    [-5., 212.1, 0.],
    [0., 199.9, 0.],
    [5., 188.1, 0.],
    [10., 176.6, 0.],
    [15., 165.3, 0.],
    [25., 143.7, 0.],
    [35., 123.3, 0.],
    [40., 113.5, 0.],
    [45., 104., 0.],
    [50., 94.7, 0.],
    [55., 85.8, 0.],
    [60., 77.1, 0.],
    [65., 68.8, 0.],
    [70., 60.7, 0.],
    [75., 52.9, 0.],
    [80., 45.3, 0.],
    [90., 31.1, 0.],
    [100., 17.9, 0.],
];
/// Lightroom's Tint for a rendered file (`IncrementalTint`) as the DNG tint of the
/// white it names, measured as [`RENDERED_TEMPERATURE`] (Tint moves only the tint):
/// `[Tint, DNG tint]`.
const RENDERED_TINT: [[f32; 2]; 13] = [
    [-100., -151.11],
    [-75., -93.38],
    [-50., -48.06],
    [-35., -26.39],
    [-25., -14.05],
    [-10., 1.35],
    [0., 9.6],
    [10., 17.81],
    [25., 32.83],
    [35., 44.5],
    [50., 64.6],
    [75., 104.74],
    [100., 154.41],
];

/// `rows` (sorted by their first value) at `x`, linearly between rows, clamped to the ends.
fn interpolate<const K: usize>(rows: &[[f32; K]], x: f32) -> [f32; K] {
    let x = x.clamp(rows[0][0], rows[rows.len() - 1][0]);
    let i = rows
        .windows(2)
        .position(|w| x <= w[1][0])
        .unwrap_or(rows.len() - 2);
    let (a, b) = (rows[i], rows[i + 1]);
    let t = (x - a[0]) / (b[0] - a[0]);
    std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
}

/// The Bradford cone response matrix, from XYZ.
const BRADFORD: [[f32; 3]; 3] = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// The ProPhoto channel gains the DNG SDK gives a camera whose RGB is linear ProPhoto
/// (Lightroom's view of a rendered file's pseudo-scene) under the white at `mired`,
/// `tint`: the file's D50 white, Bradford-adapted from that white to D50, and scaled
/// by that white's largest ProPhoto channel, as the SDK normalises a camera neutral.
/// Strong cooling with magenta takes the white outside real colours; each of its cone
/// responses is kept at least 4% of the largest, below every measured white's (the
/// coldest, Temp −100, has 4.7%), so the gains stay finite and positive.
fn camera_white_balance(mired: f32, tint: f32) -> [f32; 3] {
    use crate::camera_profiles::XYZ_TO_PRO;
    use crate::color::{inverse, mul};
    let [x, y] = crate::camera_profiles::temperature::xy_reciprocal(mired, tint);
    let cone = mul(BRADFORD, [x / y, 1., (1. - x - y) / y]);
    let largest = cone.into_iter().fold(f32::NEG_INFINITY, f32::max);
    let cone = cone.map(|v| v.max(0.04 * largest));
    let to_xyz = inverse(BRADFORD);
    let d50 = mul(BRADFORD, mul(inverse(XYZ_TO_PRO), [1.; 3]));
    let adapted = mul(to_xyz, std::array::from_fn(|c| d50[c] * d50[c] / cone[c]));
    let scale = mul(XYZ_TO_PRO, mul(to_xyz, cone))
        .into_iter()
        .fold(f32::NEG_INFINITY, f32::max);
    mul(XYZ_TO_PRO, adapted).map(|v| v * scale)
}

/// The ProPhoto channel gains of a rendered file's Temp and Tint (−100…100), as
/// Lightroom applies them to the scene its rendering is of: a camera white balance
/// from the file's D50 white to the white Temp and Tint name ([`camera_white_balance`]),
/// which brightens the photo as Lightroom does. Exactly `[1, 1, 1]` at 0, 0.
pub fn rendered_gains(temperature: f32, tint: f32) -> [f32; 3] {
    let white = |temperature: f32, tint: f32| {
        let [_, mired, drift] = interpolate(&RENDERED_TEMPERATURE, temperature);
        let [_, tint] = interpolate(&RENDERED_TINT, tint);
        camera_white_balance(mired, tint + drift)
    };
    let (gains, file) = (white(temperature, tint), white(0., 0.));
    std::array::from_fn(|c| gains[c] / file[c])
}

/// The rendered file's Temp and Tint whose [`rendered_gains`] are nearest `gains`
/// (least squares in log2, after scaling the smallest to 1), within −100…100: a
/// coarse grid, then a pattern search to 0.01. Gains no Temp and Tint reach (an
/// eyedropper's, say) give the nearest that do.
pub fn rendered_controls(gains: [f32; 3]) -> [f32; 2] {
    let log2 = |g: [f32; 3]| {
        let l = g.map(|v| {
            if v.is_finite() {
                v.max(1e-6).log2()
            } else {
                0.
            }
        });
        let least = l.into_iter().fold(f32::INFINITY, f32::min);
        l.map(|v| v - least)
    };
    let target = log2(gains);
    let cost = |t: f32, n: f32| {
        let g = log2(rendered_gains(t, n));
        (0..3).map(|c| (g[c] - target[c]).powi(2)).sum::<f32>()
    };
    let mut best = (cost(0., 0.), 0., 0.);
    for t in (-100..=100).step_by(5) {
        for n in (-100..=100).step_by(5) {
            let c = cost(t as f32, n as f32);
            if c < best.0 {
                best = (c, t as f32, n as f32);
            }
        }
    }
    let mut step = 2.5;
    while step > 0.01 {
        let (_, t0, n0) = best;
        for (dt, dn) in [(-step, 0.), (step, 0.), (0., -step), (0., step)] {
            let (t, n) = ((t0 + dt).clamp(-100., 100.), (n0 + dn).clamp(-100., 100.));
            let c = cost(t, n);
            if c < best.0 {
                best = (c, t, n);
            }
        }
        if (best.1, best.2) == (t0, n0) {
            step /= 2.;
        }
    }
    [best.1, best.2]
}

#[cfg(test)]
mod rendered_tests {
    use super::*;
    use crate::camera_data::{Metadata, Source};
    use crate::model::recipe::Recipe;

    #[test]
    fn rendered_white_balance_is_neutral_at_zero() {
        assert_eq!(rendered_gains(0., 0.), [1.; 3]);
        assert_eq!(rendered_controls([1.; 3]), [0., 0.]);
    }

    /// Lightroom Classic 15.6 (Camera Raw 18.7) on synthetic charts: the per-channel
    /// ProPhoto log2 gains of its renders of a JPEG at each setting, measured on the
    /// charts' unclipped greys as Lightroom's export over the file, each through `F⁻¹`.
    #[test]
    fn rendered_gains_match_lightroom() {
        let cases: [(f32, f32, [f32; 3], f32); 13] = [
            (10., 0., [0.236, 0.195, 0.001], 0.02),
            (25., 0., [0.564, 0.466, 0.004], 0.02),
            (50., 0., [1.026, 0.849, 0.013], 0.02),
            (100., 0., [1.603, 1.342, 0.029], 0.03),
            (-10., 0., [0.005, 0.047, 0.254], 0.02),
            (-25., 0., [0.035, 0.137, 0.677], 0.02),
            (-50., 0., [0.170, 0.322, 1.496], 0.03),
            (-75., 0., [0.510, 0.580, 2.497], 0.05),
            (0., 25., [0.102, 0., 0.195], 0.02),
            (0., -50., [0.173, 0.443, 0.006], 0.02),
            (50., 50., [1.076, 0.641, -0.006], 0.03),
            (-50., -50., [-0.027, 0.523, 0.738], 0.02),
            (-25., 25., [0.077, 0.050, 0.913], 0.02),
        ];
        for (t, n, want, tolerance) in cases {
            let got = rendered_gains(t, n).map(f32::log2);
            assert!(
                (0..3).all(|c| (got[c] - want[c]).abs() <= tolerance),
                "Temp {t} Tint {n}: {got:?}, Lightroom {want:?}"
            );
        }
    }

    #[test]
    fn rendered_gains_stay_finite_and_positive() {
        let mut previous = f32::NEG_INFINITY;
        for t in (-100..=100).step_by(5) {
            for n in (-100..=100).step_by(25) {
                let g = rendered_gains(t as f32, n as f32);
                assert!(
                    g.iter().all(|v| v.is_finite() && (0.5..=32.).contains(v)),
                    "{t} {n}: {g:?}"
                );
            }
            // Warmer is always redder against blue.
            let g = rendered_gains(t as f32, 0.);
            let warmth = (g[0] / g[2]).log2();
            assert!(warmth >= previous, "{t}: {warmth} < {previous}");
            previous = warmth;
        }
    }

    #[test]
    fn rendered_controls_invert_rendered_gains() {
        for t in [-90., -50., -20., 0., 15., 40., 80.] {
            for n in [-60., -10., 0., 30., 70.] {
                let [t2, n2] = rendered_controls(rendered_gains(t, n));
                assert!(
                    (t2 - t).abs() <= 1. && (n2 - n).abs() <= 1.,
                    "{t},{n} -> {t2},{n2}"
                );
            }
        }
    }

    #[test]
    fn rendered_controls_of_unreachable_gains_clamp() {
        for gains in [
            [1., 1., 64.],
            [64., 1., 1.],
            [1., 64., 1.],
            [0., 0., 0.],
            [f32::NAN, 1., 1.],
        ] {
            let [t, n] = rendered_controls(gains);
            assert!(t.is_finite() && n.is_finite(), "{gains:?}");
            assert!(
                (-100. ..=100.).contains(&t) && (-100. ..=100.).contains(&n),
                "{gains:?}"
            );
        }
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
        assert!(r.wb[0] > 1. && r.wb[2] <= r.wb[0]);
        assert_eq!(r.wb, rendered_gains(40., 0.));
        assert_eq!((r.temperature, r.tint), (6500., 0.));
        r.sync_white_balance_controls(&m);
        assert!((r.incremental_temperature - 40.).abs() < 0.5);
        r.reset_white_balance(&m);
        assert_eq!(
            (r.incremental_temperature, r.incremental_tint, r.wb),
            (0., 0., [1.; 3])
        );
    }

    /// Every Temp and Tint a rendered photo can be given, the cool magenta corner
    /// outside what was measured among them, keeps a recipe that validates, renders
    /// and saves.
    #[test]
    fn every_rendered_temp_and_tint_gives_a_valid_recipe() {
        let m = Metadata {
            source: Source::Rendered,
            ..Default::default()
        };
        for t in (-100..=100).step_by(5) {
            for n in (-100..=100).step_by(5) {
                let mut r = Recipe::for_rendered(&m);
                r.incremental_temperature = t as f32;
                r.incremental_tint = n as f32;
                r.update_wb(&m);
                assert!(r.validate().is_ok(), "Temp {t} Tint {n}: {:?}", r.wb);
            }
        }
    }

    /// Copying Treatment & Profile without White Balance, or picking a profile,
    /// re-syncs the controls: gains that are already those of the controls keep
    /// them exactly, and with them the WB menu's Auto.
    #[test]
    fn a_rendered_photos_matching_gains_keep_its_temp_and_tint() {
        let m = Metadata {
            source: Source::Rendered,
            ..Default::default()
        };
        let mut r = Recipe::for_rendered(&m);
        r.incremental_temperature = -77.;
        r.incremental_tint = 30.;
        r.wb = rendered_gains(-77., 30.);
        r.auto_white_balance = Some([-77., 30.]);
        let before = r.clone();
        r.sync_white_balance_controls(&m);
        assert_eq!(
            (r.incremental_temperature, r.incremental_tint, r.wb),
            (-77., 30., before.wb)
        );
        assert_eq!(r.auto_white_balance, Some([-77., 30.]));
        r.profile_changed(&m);
        assert_eq!((r.incremental_temperature, r.incremental_tint), (-77., 30.));
    }

    #[test]
    fn picked_gains_on_a_rendered_photo_become_those_its_controls_show() {
        let m = Metadata {
            source: Source::Rendered,
            ..Default::default()
        };
        let mut r = Recipe::for_rendered(&m);
        // An eyedropper's gains, off the model's reach (and green ≠ 1).
        r.wb = [1.4, 0.9, 1.2];
        r.sync_white_balance_controls(&m);
        let shown = rendered_gains(r.incremental_temperature, r.incremental_tint);
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
