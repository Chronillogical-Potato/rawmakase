//! Highlight recovery: clipped channels rebuilt from the ones that are not.
use super::*;

/// Colour references for wide, partially clipped highlights. References must be
/// smoothly varying, unclipped pixels in the same connected bright area. Dark
/// gaps separate objects; an unrelated bright patch cannot supply missing colour.
struct RecoveryColors {
    size: [usize; 2],
    scale: [f32; 2],
    channels: [Vec<f32>; 3],
}
impl RecoveryColors {
    fn build(im: &CameraImage, cancel: &AtomicBool) -> Result<Self> {
        let scale = (512. / im.width.max(im.height) as f32).min(1.);
        let width = (im.width as f32 * scale).ceil() as usize;
        let height = (im.height as f32 * scale).ceil() as usize;
        let mut grid = vec![[0.; 4]; width * height];
        let mut bright = vec![0_u32; grid.len()];
        let mut total = vec![0_u32; grid.len()];
        let peak = |p: [f32; 3]| {
            (0..3)
                .map(|c| p[c] / im.metadata.wb[c].max(0.001))
                .fold(0., f32::max)
        };
        for y in 0..im.height as usize {
            check_cancel(cancel)?;
            for x in 0..im.width as usize {
                let p = im.pixels[y * im.width as usize + x];
                let i = (y * height / im.height as usize) * width + x * width / im.width as usize;
                total[i] += 1;
                let value = peak(p);
                if value < 0.75 {
                    continue;
                }
                bright[i] += 1;
                if value >= 0.97 {
                    continue;
                }
                // Do not infer an overexposed object's colour from a demosaiced
                // edge against a different object (or a chart's dark patch gap).
                let smooth = [(-3, 0), (3, 0), (0, -3), (0, 3)]
                    .into_iter()
                    .all(|(dx, dy)| {
                        let xx = (x as i32 + dx).clamp(0, im.width as i32 - 1) as usize;
                        let yy = (y as i32 + dy).clamp(0, im.height as i32 - 1) as usize;
                        let q = im.pixels[yy * im.width as usize + xx];
                        peak(q) < 0.97
                            && (0..3).all(|c| (q[c] - p[c]).abs() <= p[c].max(0.01) * 0.15)
                    });
                if !smooth {
                    continue;
                }
                let sum = p.iter().sum::<f32>();
                let weight = value.powi(4);
                for c in 0..3 {
                    grid[i][c] += p[c] / sum * weight;
                }
                grid[i][3] += weight;
            }
        }
        let mut visited = vec![false; grid.len()];
        let mut channels = std::array::from_fn(|_| vec![1. / 3.; grid.len()]);
        let mut component = Vec::new();
        for start in 0..grid.len() {
            check_cancel(cancel)?;
            if visited[start] || bright[start] * 2 < total[start] {
                continue;
            }
            component.clear();
            component.push(start);
            visited[start] = true;
            let mut head = 0;
            let mut sum = [0_f64; 4];
            while head < component.len() {
                let i = component[head];
                head += 1;
                for c in 0..4 {
                    sum[c] += f64::from(grid[i][c]);
                }
                let (x, y) = (i % width, i / width);
                for next in [
                    (x > 0).then(|| i - 1),
                    (x + 1 < width).then_some(i + 1),
                    (y > 0).then(|| i - width),
                    (y + 1 < height).then_some(i + width),
                ]
                .into_iter()
                .flatten()
                {
                    if !visited[next] && bright[next] * 2 >= total[next] {
                        visited[next] = true;
                        component.push(next);
                    }
                }
            }
            if sum[3] > 0. {
                for &i in &component {
                    for c in 0..3 {
                        channels[c][i] = (sum[c] / sum[3]) as f32;
                    }
                }
            }
        }
        Ok(Self {
            size: [width, height],
            scale: [
                width as f32 / im.width as f32,
                height as f32 / im.height as f32,
            ],
            channels,
        })
    }
    fn at(&self, x: i32, y: i32) -> [f32; 3] {
        std::array::from_fn(|c| {
            develop::local_tone::grid_sample(
                &self.channels[c],
                self.size,
                self.scale,
                [x as f32, y as f32],
            )
        })
    }
}

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
    // Most images never need the wider search. Share its bounded colour map only
    // when the first partially clipped pixel has no local colour reference.
    let colors = std::sync::OnceLock::new();
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
        if weight_sum == 0.
            && !valid.is_empty()
            && let Ok(colors) = colors.get_or_init(|| RecoveryColors::build(im, cancel))
        {
            let color = colors.at(x, y);
            let denom = valid.iter().map(|c| color[*c]).sum::<f32>();
            if denom > 1e-5 {
                let gain = valid.iter().map(|c| p[*c]).sum::<f32>() / denom;
                estimate = color.map(|c| (c * gain).min(neutral * 4.));
                weight_sum = 1.;
            }
        }
        let blend = sensor.map(|v| ((v - 0.97) / 0.03).clamp(0., 1.));
        for c in 0..3 {
            let target = if weight_sum > 0. {
                estimate[c] / weight_sum
            } else {
                neutral
            };
            // With one channel clipped, its value is a floor: the light was at least
            // that bright, and the other two hold its colour. With two clipped,
            // Camera Raw renders the core whiter than the recorded pair (a sunset sun
            // with red and blue clipped is yellow-white, not magenta), so the estimate
            // stays as it is. The floor fades out as another channel's own blend
            // comes in, so a gradient into a second clipped channel has no step.
            let alone: f32 = (0..3).filter(|d| *d != c).map(|d| 1. - blend[d]).product();
            let target = target + (target.max(p[c]) - target) * alone;
            p[c] = p[c] * (1. - blend[c]) + target * blend[c];
        }
    });
    check_cancel(cancel)?;
    Ok(out)
}
