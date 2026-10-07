//! Highlight recovery: clipped channels rebuilt from the ones that are not.
use super::*;

/// Neighborhood-ratio reconstruction in camera space, before color conversion.
/// Fully clipped neighborhoods have no recoverable color and use a neutral fallback.
pub fn recover_highlights(im: &CameraImage) -> CameraImage {
    recover_highlights_cancellable(im, &AtomicBool::new(false)).unwrap()
}
pub(super) fn recover_highlights_cancellable(
    im: &CameraImage,
    cancel: &AtomicBool,
) -> Result<CameraImage> {
    check_cancel(cancel)?;
    let mut out = im.clone();
    out.pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let sensor =
            std::array::from_fn::<_, 3, _>(|c| im.pixels[i][c] / im.metadata.wb[c].max(0.001));
        let peak = sensor.into_iter().fold(0f32, f32::max);
        if peak < 0.97 {
            return;
        }
        let valid: Vec<usize> = (0..3).filter(|c| sensor[*c] < 0.97).collect();
        let x = i as i32 % im.width as i32;
        let y = i as i32 / im.width as i32;
        let mut estimate = [0.; 3];
        let mut weight_sum = 0.;
        if !valid.is_empty() {
            for dy in -3i32..=3 {
                for dx in -3i32..=3 {
                    let xx = (x + dx).clamp(0, im.width as i32 - 1);
                    let yy = (y + dy).clamp(0, im.height as i32 - 1);
                    let q = im.pixels[(yy as u32 * im.width + xx as u32) as usize];
                    if (0..3).any(|c| q[c] / im.metadata.wb[c].max(0.001) >= 0.97) {
                        continue;
                    }
                    let denom: f32 = valid.iter().map(|c| q[*c]).sum();
                    if denom < 1e-5 {
                        continue;
                    }
                    let target: f32 = valid.iter().map(|c| p[*c]).sum();
                    let gain = (target / denom).clamp(0.25, 4.);
                    let weight = 1. / (1. + (dx * dx + dy * dy) as f32);
                    weight_sum += weight;
                    for c in 0..3 {
                        estimate[c] += q[c] * gain * weight;
                    }
                }
            }
        }
        let neutral = valid.iter().map(|c| p[*c]).sum::<f32>() / valid.len().max(1) as f32;
        let neutral = if valid.is_empty() {
            p.iter().copied().fold(0f32, f32::max)
        } else {
            neutral
        };
        for c in 0..3 {
            let blend = ((sensor[c] - 0.97) / 0.03).clamp(0., 1.);
            let target = if weight_sum > 0. {
                estimate[c] / weight_sum
            } else {
                neutral
            };
            p[c] = p[c] * (1. - blend) + target * blend;
        }
    });
    check_cancel(cancel)?;
    Ok(out)
}
