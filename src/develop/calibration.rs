//! Independent, reference-calibrated approximation of primary adjustments.
//! Matrices act on color differences, preserving neutral gray exactly. Coefficients
//! were estimated from isolated Adobe Standard/X100F controls; see the validation
//! report for the tested cameras/settings. These are not Adobe's private algorithms.
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
    matrix: [[f32; 3]; 3],
    shadow: f32,
}
impl Calibration {
    pub(crate) fn new(primaries: [[f32; 2]; 3], shadow: f32) -> Self {
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
        assert_eq!(Calibration::new([[0.; 2]; 3], 0.).apply(p), p);
        for control in [-1., -0.5, 0.5, 1.] {
            for axis in 0..3 {
                for kind in 0..2 {
                    let mut controls = [[0.; 2]; 3];
                    controls[axis][kind] = control;
                    let c = Calibration::new(controls, 0.);
                    for v in [0., 0.18, 1., 2.] {
                        assert!(c.apply([v; 3]).iter().all(|q| (q - v).abs() < 2e-6));
                    }
                }
            }
        }
    }
    #[test]
    fn shadow_tint_direction_and_highlight_falloff() {
        let plus = Calibration::new([[0.; 2]; 3], 0.5).apply([0.1; 3]);
        let minus = Calibration::new([[0.; 2]; 3], -0.5).apply([0.1; 3]);
        assert!(plus[0] > plus[1] && plus[2] > plus[1]);
        assert!(minus[1] > minus[0] && minus[1] > minus[2]);
        assert_eq!(Calibration::new([[0.; 2]; 3], 1.).apply([0.; 3]), [0.; 3]);
        let high = Calibration::new([[0.; 2]; 3], 1.).apply([2.; 3]);
        assert!(high.iter().all(|v| (v - 2.).abs() < 1e-4));
    }
    #[test]
    fn combined_slider_extremes_stay_finite() {
        for bits in 0..128 {
            let v = |i| if bits & (1 << i) == 0 { -1. } else { 1. };
            let controls = std::array::from_fn(|i| [v(i * 2), v(i * 2 + 1)]);
            let calibration = Calibration::new(controls, v(6));
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
