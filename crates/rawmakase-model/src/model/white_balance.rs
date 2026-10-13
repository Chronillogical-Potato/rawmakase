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

/// Lightroom's Temp for a rendered file (`IncrementalTemperature`, −100…100) as an
/// offset in reciprocal temperature from 5000 K (200 mired) and a DNG tint drift:
/// `[Temp, mired offset, tint drift]`. Fitted in the DNG SDK's temperature model to
/// the ProPhoto gains that reproduce Lightroom Classic 15.6 (Camera Raw 18.7) renders
/// of synthetic JPEG charts (`scripts/corpus/rendered-white-balance.py`). Past +50
/// Lightroom keeps warming beyond the Planckian locus, which the drift carries; the
/// cool side's drift fitted to noise (±1.5) and is 0. Temp −100 with Tint of +25 or
/// more, and strong cooling with magenta (Temp −50, Tint +50, say), are outside what
/// was measured; the soft floor in `prophoto_white` keeps them finite.
const RENDERED_TEMPERATURE: [[f32; 3]; 27] = [
    [-100., 359.5, 0.],
    [-90., 304.5, 0.],
    [-75., 244.5, 0.],
    [-60., 185.5, 0.],
    [-50., 146., 0.],
    [-35., 100., 0.],
    [-25., 68.5, 0.],
    [-15., 40., 0.],
    [-10., 26., 0.],
    [-5., 13., 0.],
    [0., 0., 0.],
    [5., -13., 0.5],
    [10., -25.5, 0.5],
    [15., -38.5, 2.],
    [25., -63., 5.5],
    [35., -87.5, 10.],
    [40., -99.5, 13.],
    [45., -110.5, 15.5],
    [50., -119.5, 17.5],
    [55., -133., 21.5],
    [60., -142.5, 22.],
    [65., -151.5, 22.],
    [70., -160., 22.],
    [75., -168.5, 22.],
    [80., -177.5, 22.],
    [90., -193.5, 22.],
    [100., -200., 22.],
];
/// Lightroom's Tint for a rendered file (`IncrementalTint`) as a DNG tint at 5000 K,
/// fitted as [`RENDERED_TEMPERATURE`]: `[Tint, DNG tint]`.
const RENDERED_TINT: [[f32; 2]; 13] = [
    [-100., -166.],
    [-75., -106.],
    [-50., -59.],
    [-35., -36.75],
    [-25., -24.25],
    [-10., -8.5],
    [0., 0.],
    [10., 8.25],
    [25., 23.75],
    [35., 36.],
    [50., 57.],
    [75., 99.5],
    [100., 151.],
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

/// The linear ProPhoto RGB of the DNG temperature/tint white at `mired`, `tint`.
/// A white outside ProPhoto's gamut (Temp −100 with a positive Tint reaches one's
/// blue below zero) keeps every channel positive: below `FLOOR` a channel decays
/// exponentially with the value and slope it has there, so gains stay finite and
/// strictly ordered instead of flipping sign.
fn prophoto_white(mired: f32, tint: f32) -> [f32; 3] {
    const FLOOR: f32 = 0.02;
    let [x, y] = crate::camera_profiles::temperature::xy_reciprocal(mired, tint);
    crate::color::mul(
        crate::camera_profiles::XYZ_TO_PRO,
        [x / y, 1., (1. - x - y) / y],
    )
    .map(|v| {
        if v >= FLOOR {
            v
        } else {
            FLOOR * ((v - FLOOR) / FLOOR).exp()
        }
    })
}

/// The ProPhoto channel gains of a rendered file's Temp and Tint (−100…100), as
/// Lightroom applies them to the scene its rendering is of: a camera white balance
/// from the file's D50 white to the white Temp and Tint name, scaled so the smallest
/// gain is 1 — so the file's white stays white and the photo brightens, as in
/// Lightroom. Exactly `[1, 1, 1]` at 0, 0.
pub fn rendered_gains(temperature: f32, tint: f32) -> [f32; 3] {
    let [_, offset, drift] = interpolate(&RENDERED_TEMPERATURE, temperature);
    let [_, tint] = interpolate(&RENDERED_TINT, tint);
    let file = prophoto_white(200., 0.);
    let target = prophoto_white(200. + offset, drift + tint);
    let gains: [f32; 3] = std::array::from_fn(|c| file[c] / target[c]);
    let least = gains.into_iter().fold(f32::INFINITY, f32::min);
    gains.map(|g| g / least)
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

    /// log2 gains, normalised so the smallest channel is 0.
    fn log2(g: [f32; 3]) -> [f32; 3] {
        let l = g.map(|v| v.log2());
        let least = l.into_iter().fold(f32::INFINITY, f32::min);
        l.map(|v| v - least)
    }

    #[test]
    fn rendered_white_balance_is_neutral_at_zero() {
        assert_eq!(rendered_gains(0., 0.), [1.; 3]);
        assert_eq!(rendered_controls([1.; 3]), [0., 0.]);
    }

    /// Lightroom Classic 15.6 (Camera Raw 18.7) on synthetic charts: the per-channel
    /// ProPhoto log2 gains that reproduce its renders of a JPEG at each setting.
    #[test]
    fn rendered_gains_match_lightroom() {
        let cases: [(f32, f32, [f32; 3], f32); 7] = [
            (10., 0., [0.236, 0.196, 0.], 0.05),
            (25., 0., [0.563, 0.465, 0.004], 0.05),
            (-25., 0., [0.039, 0.138, 0.675], 0.05),
            (50., 0., [1.024, 0.846, 0.018], 0.05),
            (0., 25., [0.103, 0., 0.194], 0.05),
            (0., -50., [0.175, 0.443, 0.009], 0.05),
            (50., 50., [1.075, 0.640, 0.016], 0.08),
        ];
        for (t, n, want, tolerance) in cases {
            let got = log2(rendered_gains(t, n));
            assert!(
                (0..3).all(|c| (got[c] - want[c]).abs() <= tolerance),
                "Temp {t} Tint {n}: {got:?}, Lightroom {want:?}"
            );
        }
    }

    #[test]
    fn rendered_gains_stay_finite_with_one_as_the_smallest() {
        let mut previous = f32::NEG_INFINITY;
        for t in (-100..=100).step_by(5) {
            for n in (-100..=100).step_by(25) {
                let g = rendered_gains(t as f32, n as f32);
                assert!(
                    g.iter().all(|v| v.is_finite() && *v >= 1.),
                    "{t} {n}: {g:?}"
                );
                assert_eq!(g.into_iter().fold(f32::INFINITY, f32::min), 1., "{t} {n}");
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
