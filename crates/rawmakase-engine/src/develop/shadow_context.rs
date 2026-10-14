//! Calibrated scene context for positive Shadows.
//!
//! A mean luminance alone cannot distinguish a dark scene lit through small gaps
//! from a scene with a large bright area. Read the distribution at several spatial
//! scales, relative to the mean, and interpolate the measured regional-key offsets.
//! Only positive Shadows uses this key; exposure, negative Shadows and Highlights
//! retain their own calibration. The map and the key are shared by CPU, GPU and masks.
//! See docs/scene-tone-stage.md and scripts/corpus/shadow-context.py.
use super::clarity::gauss;
use super::shadow_context_data::{CENTRES, OFFSETS};

fn percentile(sorted: &[f32], q: f32) -> f32 {
    let position = (sorted.len() - 1) as f32 * q;
    let lo = position.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (position - lo as f32)
}

pub(super) fn offset(logs: &[f32], key: f32, w: usize, h: usize) -> f32 {
    let mut sorted = logs.to_vec();
    sorted.sort_unstable_by(f32::total_cmp);
    let mut feature = Vec::with_capacity(34);
    for q in [0.01, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95, 0.99, 0.995, 0.999] {
        feature.push(percentile(&sorted, q) - key);
    }
    let linear: Vec<f32> = logs.iter().map(|v| v.exp2()).collect();
    for scale in [0.005, 0.02, 0.08, 0.25] {
        let sigma = scale * w.max(h) as f32;
        let mut light: Vec<f32> = gauss(&linear, w, h, sigma)
            .into_iter()
            .map(|v| v.max(super::local_tone::FLOOR).log2())
            .collect();
        light.sort_unstable_by(f32::total_cmp);
        for q in [0.1, 0.5, 0.9, 0.99] {
            feature.push(0.5 * (percentile(&light, q) - key));
        }
        let smooth = gauss(logs, w, h, sigma);
        let (sum, square, absolute) = logs.iter().zip(smooth).fold(
            (0_f64, 0_f64, 0_f64),
            |(sum, square, absolute), (v, b)| {
                let d = f64::from(*v - b);
                (sum + d, square + d * d, absolute + d.abs())
            },
        );
        let n = logs.len() as f64;
        feature.push(0.5 * (absolute / n) as f32);
        feature.push(0.5 * (square / n - (sum / n).powi(2)).max(0.).sqrt() as f32);
    }
    // The third neighbour sets a continuous bandwidth. Inverse-distance weighting
    // keeps measured scenes close to their calibration without selecting a single
    // nearest scene (which would jump as Exposure or white balance changes).
    let distance: Vec<f32> = CENTRES
        .iter()
        .map(|centre| {
            feature
                .iter()
                .zip(centre)
                .map(|(a, b)| (a - b).powi(2))
                .sum()
        })
        .collect();
    let mut sorted_distance = distance.clone();
    sorted_distance.select_nth_unstable_by(2, f32::total_cmp);
    let denominator = sorted_distance[2].max(0.01);
    // Subtract the nearest distance to avoid underflow far outside the sample set.
    let nearest = distance.iter().copied().fold(f32::INFINITY, f32::min);
    let (sum, weight) =
        distance
            .iter()
            .zip(OFFSETS)
            .fold((0., 0.), |(sum, weight), (d, offset)| {
                let k = (-2. * (d - nearest) / denominator).exp() / (d + 0.01);
                (sum + k * offset, weight + k)
            });
    (sum / weight).clamp(-2., 2.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changing_exposure_does_not_activate_the_shadow_correction() {
        let (w, h) = (96, 64);
        let logs: Vec<f32> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f32, (i / w) as f32);
                -6. + (x * 0.12).sin() * (y * 0.09).cos()
                    + if (x - 30.).hypot(y - 25.) < 3. {
                        6.
                    } else {
                        0.
                    }
            })
            .collect();
        let key =
            (logs.iter().map(|v| v.exp2() as f64).sum::<f64>() / logs.len() as f64).log2() as f32;
        let expected = offset(&logs, key, w, h);
        for exposure in [-3., -2., 0., 1., 2.] {
            let shifted: Vec<f32> = logs.iter().map(|v| v + exposure).collect();
            let actual = offset(&shifted, key + exposure, w, h);
            assert!(
                (actual - expected).abs() < 0.001,
                "{exposure}: {actual} vs {expected}"
            );
        }
    }

    #[test]
    fn uniform_and_extreme_scenes_have_finite_bounded_corrections() {
        for level in [-14., -8., -2., 0., 8., 20.] {
            let logs = vec![level; 64 * 64];
            let actual = offset(&logs, level, 64, 64);
            assert!(actual.is_finite() && (-2. ..=2.).contains(&actual));
        }
        assert!(CENTRES.iter().flatten().all(|v| v.is_finite()));
        assert!(
            OFFSETS
                .iter()
                .all(|v| v.is_finite() && (-2. ..=2.).contains(v))
        );
    }
}
