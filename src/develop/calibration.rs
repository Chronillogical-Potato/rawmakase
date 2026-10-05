//! Independent, reference-calibrated approximation of primary adjustments.
//! Matrices act on color differences, preserving neutral gray exactly. Coefficients
//! were estimated from isolated Adobe Standard/X100F controls; see the validation
//! report for the tested cameras/settings. These are not Adobe's private algorithms.
//! New edits use `CalibrationModel::Measured`: the change Camera Raw 18.7 makes at
//! −100, −50, +50 and +100 of each primary slider, fitted through RAWmakase's own
//! pipeline on a dense synthetic chart and interpolated between those positions
//! (docs/rendering-quality.md#camera-calibration).
use serde::{Deserialize, Serialize};

/// Which fit renders Camera Calibration's primary Hue and Saturation sliders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalibrationModel {
    /// Coefficients from isolated Adobe Standard/X100F controls, scaled linearly:
    /// what recipes saved before the measured fit keep, so they render as they did.
    #[default]
    Original,
    /// Camera Raw 18.7's change at four slider positions, interpolated.
    Measured,
}
impl CalibrationModel {
    pub(crate) fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// A neutral-preserving change to the calibration matrix: row `r` gains
/// `a[r]`, `−(a[r] + b[r])` and `b[r]` in its three columns.
#[derive(Clone, Copy)]
struct Adjustment {
    a: [f32; 3],
    b: [f32; 3],
}
impl Adjustment {
    const ZERO: Self = Self {
        a: [0.; 3],
        b: [0.; 3],
    };
    fn lerp(self, other: Self, t: f32) -> Self {
        let mix = |x: [f32; 3], y: [f32; 3]| std::array::from_fn(|r| x[r] + (y[r] - x[r]) * t);
        Self {
            a: mix(self.a, other.a),
            b: mix(self.b, other.b),
        }
    }
    /// At slider position `value` (−1–1) of a measured primary slider.
    fn measured(primary: usize, kind: usize, value: f32) -> Self {
        let [m100, m50, p50, p100] = MEASURED[primary][kind];
        let knots = [
            (-1., m100),
            (-0.5, m50),
            (0., Self::ZERO),
            (0.5, p50),
            (1., p100),
        ];
        let v = value.clamp(-1., 1.);
        let i = knots.iter().rposition(|(x, _)| *x <= v).unwrap().min(3);
        let ((x0, a0), (x1, a1)) = (knots[i], knots[i + 1]);
        a0.lerp(a1, (v - x0) / (x1 - x0))
    }
}

/// Camera Raw 18.7 on a dense synthetic chart (the synthetic camera's embedded
/// profile, D65): per primary (red, green, blue) and slider (Hue, Saturation), the
/// change at −100, −50, +50 and +100.
#[rustfmt::skip]
const MEASURED: [[[Adjustment; 4]; 2]; 3] = [
    [
        [
            Adjustment { a: [0.0257, -0.1685, 0.2119], b: [0.0985, -0.0398, -0.2825] },
            Adjustment { a: [0.0040, -0.0857, 0.1019], b: [0.0506, -0.0145, -0.1443] },
            Adjustment { a: [-0.0446, 0.0866, -0.1087], b: [-0.0354, 0.0326, 0.1309] },
            Adjustment { a: [-0.0738, 0.1752, -0.2156], b: [-0.0786, 0.0518, 0.2664] },
        ],
        [
            Adjustment { a: [-0.1960, 0.2131, 0.2123], b: [0.0576, 0.0569, -0.3492] },
            Adjustment { a: [-0.1063, 0.1054, 0.1039], b: [0.0329, 0.0339, -0.1763] },
            Adjustment { a: [0.0643, -0.1029, -0.1085], b: [-0.0169, -0.0148, 0.1614] },
            Adjustment { a: [0.1455, -0.2060, -0.2114], b: [-0.0375, -0.0406, 0.3279] },
        ],
    ],
    [
        [
            Adjustment { a: [-0.2924, 0.0345, -0.0296], b: [-0.1393, 0.0177, 0.3133] },
            Adjustment { a: [-0.1550, 0.0160, -0.0179], b: [-0.0669, 0.0150, 0.1557] },
            Adjustment { a: [0.1194, -0.0144, 0.0137], b: [0.0810, 0.0030, -0.1671] },
            Adjustment { a: [0.2611, -0.0265, 0.0288], b: [0.1568, -0.0050, -0.3291] },
        ],
        [
            Adjustment { a: [-0.3696, 0.0381, 0.0365], b: [0.0095, 0.0117, -0.3949] },
            Adjustment { a: [-0.1920, 0.0186, 0.0163], b: [0.0087, 0.0112, -0.1985] },
            Adjustment { a: [0.1531, -0.0175, -0.0208], b: [0.0067, 0.0082, 0.1843] },
            Adjustment { a: [0.3272, -0.0335, -0.0377], b: [0.0124, 0.0064, 0.3774] },
        ],
    ],
    [
        [
            Adjustment { a: [0.3445, -0.0652, 0.0068], b: [-0.6632, 0.3938, -0.0512] },
            Adjustment { a: [0.1616, -0.0324, 0.0038], b: [-0.3225, 0.1996, -0.0282] },
            Adjustment { a: [-0.2003, 0.0306, -0.0067], b: [0.3409, -0.1808, 0.0048] },
            Adjustment { a: [-0.3802, 0.0665, -0.0078], b: [0.6706, -0.3789, 0.0161] },
        ],
        [
            Adjustment { a: [-0.4030, 0.0028, 0.0029], b: [0.3526, 0.3545, -0.0531] },
            Adjustment { a: [-0.2086, 0.0006, 0.0000], b: [0.1795, 0.1817, -0.0305] },
            Adjustment { a: [0.1706, 0.0007, -0.0031], b: [-0.1633, -0.1654, 0.0116] },
            Adjustment { a: [0.3629, -0.0002, -0.0044], b: [-0.3399, -0.3308, 0.0325] },
        ],
    ],
];
const HUE: [[[f32; 3]; 2]; 3] = [
    [[0.039, 0.182, -0.131], [-0.121, 0.003, 0.416]],
    [[0.235, -0.051, -0.026], [0.241, 0.024, -0.354]],
    [[-0.405, 0.093, 0.019], [0.435, -0.319, 0.021]],
];
const SAT: [[[f32; 3]; 2]; 3] = [
    [[0.268, -0.154, -0.088], [-0.133, -0.118, 0.499]],
    [[0.378, -0.044, -0.003], [-0.047, -0.074, 0.469]],
    [[0.460, 0.004, -0.024], [-0.050, -0.207, 0.163]],
];
const IDENTITY: [[f32; 3]; 3] = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
fn inverse(a: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let cofactor = std::array::from_fn::<_, 3, _>(|i| {
        std::array::from_fn::<_, 3, _>(|j| {
            let r1 = (i + 1) % 3;
            let r2 = (i + 2) % 3;
            let c1 = (j + 1) % 3;
            let c2 = (j + 2) % 3;
            a[r1][c1] * a[r2][c2] - a[r1][c2] * a[r2][c1]
        })
    });
    let determinant: f32 = (0..3).map(|c| a[0][c] * cofactor[0][c]).sum();
    // The supported slider range remains far from singularity.
    debug_assert!(determinant.abs() > 1e-6);
    std::array::from_fn(|i| std::array::from_fn(|j| cofactor[j][i] / determinant))
}
pub(crate) struct Calibration {
    pub(crate) matrix: [[f32; 3]; 3],
    pub(crate) shadow: f32,
}
impl Calibration {
    pub(crate) fn new(primaries: [[f32; 2]; 3], shadow: f32, model: CalibrationModel) -> Self {
        if model == CalibrationModel::Measured {
            return Self::measured(primaries, shadow);
        }
        let mut matrix = IDENTITY;
        for (i, controls) in primaries.into_iter().enumerate() {
            for (kind, value) in controls.into_iter().enumerate() {
                let basis = if kind == 0 { HUE[i] } else { SAT[i] };
                let mut adjustment = std::array::from_fn::<_, 3, _>(|row| {
                    let a = basis[0][row];
                    let b = basis[1][row];
                    let factor = if kind == 1 && value < 0. {
                        -value
                    } else {
                        value
                    };
                    [a * factor, -(a + b) * factor, b * factor]
                });
                if kind == 1 && value < 0. {
                    let positive = std::array::from_fn(|r| {
                        std::array::from_fn(|c| IDENTITY[r][c] + adjustment[r][c])
                    });
                    let reversed = inverse(positive);
                    adjustment = std::array::from_fn(|r| {
                        std::array::from_fn(|c| reversed[r][c] - IDENTITY[r][c])
                    });
                }
                for r in 0..3 {
                    for c in 0..3 {
                        matrix[r][c] += adjustment[r][c];
                    }
                }
            }
        }
        Self { matrix, shadow }
    }
    /// Measured changes add, as the original model's do.
    fn measured(primaries: [[f32; 2]; 3], shadow: f32) -> Self {
        let mut matrix = IDENTITY;
        for (primary, controls) in primaries.into_iter().enumerate() {
            for (kind, value) in controls.into_iter().enumerate() {
                if value == 0. {
                    continue;
                }
                let Adjustment { a, b } = Adjustment::measured(primary, kind, value);
                for r in 0..3 {
                    matrix[r][0] += a[r];
                    matrix[r][1] -= a[r] + b[r];
                    matrix[r][2] += b[r];
                }
            }
        }
        Self { matrix, shadow }
    }
    pub(crate) fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let q = self
            .matrix
            .map(|row| row.iter().zip(p).map(|(a, b)| a * b).sum::<f32>());
        if self.shadow == 0. {
            return q;
        }
        let y = (0.2126 * q[0] + 0.7152 * q[1] + 0.0722 * q[2]).max(0.);
        let amount = self.shadow.abs() * y * (-6. * y).exp();
        // Normalized green/magenta shifts are asymmetric: either green or its
        // complementary channels are attenuated, rather than lifting all shadows.
        let direction = if self.shadow > 0. {
            [0.116, -0.189, -0.002]
        } else {
            [-0.331, 0.029, -0.152]
        };
        std::array::from_fn(|c| q[c] + amount * direction[c])
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neutral_and_zero_calibration_are_preserved() {
        let p = [-0.1, 0.2, 1.2];
        assert_eq!(
            Calibration::new([[0.; 2]; 3], 0., CalibrationModel::Original).apply(p),
            p
        );
        for control in [-1., -0.5, 0.5, 1.] {
            for axis in 0..3 {
                for kind in 0..2 {
                    let mut controls = [[0.; 2]; 3];
                    controls[axis][kind] = control;
                    let c = Calibration::new(controls, 0., CalibrationModel::Original);
                    for v in [0., 0.18, 1., 2.] {
                        assert!(c.apply([v; 3]).iter().all(|q| (q - v).abs() < 2e-6));
                    }
                }
            }
        }
    }
    #[test]
    fn measured_sliders_keep_neutrals_and_hit_their_measured_changes() {
        for primary in 0..3 {
            for kind in 0..2 {
                for (i, value) in [-1., -0.5, 0.5, 1.].into_iter().enumerate() {
                    let mut controls = [[0.; 2]; 3];
                    controls[primary][kind] = value;
                    let c = Calibration::new(controls, 0., CalibrationModel::Measured);
                    for v in [0., 0.18, 1.] {
                        assert!(c.apply([v; 3]).iter().all(|q| (q - v).abs() < 2e-6));
                    }
                    let Adjustment { a, b } = MEASURED[primary][kind][i];
                    for r in 0..3 {
                        let row = c.matrix[r];
                        assert!((row[0] - IDENTITY[r][0] - a[r]).abs() < 1e-6);
                        assert!((row[2] - IDENTITY[r][2] - b[r]).abs() < 1e-6);
                    }
                }
                // Halfway between measured positions, halfway between their changes.
                let mut controls = [[0.; 2]; 3];
                controls[primary][kind] = 0.75;
                let c = Calibration::new(controls, 0., CalibrationModel::Measured);
                let [_, _, half, full] = MEASURED[primary][kind];
                assert!((c.matrix[0][0] - 1. - (half.a[0] + full.a[0]) / 2.).abs() < 1e-6);
            }
        }
        // Camera Raw's Blue Hue −100 turns pure blue toward cyan, not magenta.
        let mut controls = [[0.; 2]; 3];
        controls[2][0] = -1.;
        let blue = Calibration::new(controls, 0., CalibrationModel::Measured).apply([0., 0., 1.]);
        assert!(blue[1] > blue[0], "{blue:?}");
    }
    #[test]
    fn shadow_tint_direction_and_highlight_falloff() {
        let plus = Calibration::new([[0.; 2]; 3], 0.5, CalibrationModel::Original).apply([0.1; 3]);
        let minus =
            Calibration::new([[0.; 2]; 3], -0.5, CalibrationModel::Original).apply([0.1; 3]);
        assert!(plus[0] > plus[1] && plus[2] > plus[1]);
        assert!(minus[1] > minus[0] && minus[1] > minus[2]);
        assert_eq!(
            Calibration::new([[0.; 2]; 3], 1., CalibrationModel::Original).apply([0.; 3]),
            [0.; 3]
        );
        let high = Calibration::new([[0.; 2]; 3], 1., CalibrationModel::Original).apply([2.; 3]);
        assert!(high.iter().all(|v| (v - 2.).abs() < 1e-4));
    }
    #[test]
    fn combined_slider_extremes_stay_finite() {
        for bits in 0..128 {
            let v = |i| if bits & (1 << i) == 0 { -1. } else { 1. };
            let controls = std::array::from_fn(|i| [v(i * 2), v(i * 2 + 1)]);
            for model in [CalibrationModel::Original, CalibrationModel::Measured] {
                let calibration = Calibration::new(controls, v(6), model);
                for p in [
                    [0.; 3],
                    [0.18; 3],
                    [1., 0., 0.],
                    [0., 1., 0.],
                    [0., 0., 1.],
                    [-0.1, 2., 8.],
                ] {
                    assert!(calibration.apply(p).iter().all(|v| v.is_finite()));
                }
            }
        }
    }
}
