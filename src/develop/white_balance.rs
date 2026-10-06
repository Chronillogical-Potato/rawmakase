//! Fallback white balance for cameras without calibrated DCP matrices, and Lightroom's
//! named white balance presets.
use crate::{
    color::{inverse, mul},
    raw::Metadata,
};
pub(super) fn illuminant_camera(t: f32, m: &Metadata) -> [f32; 3] {
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
pub(super) fn estimate_temperature(m: &Metadata) -> f32 {
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
