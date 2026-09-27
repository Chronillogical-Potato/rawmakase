//! Global Basic-panel tone sliders (Dehaze, Contrast, Whites, Blacks) for engine 4, as measured
//! Lightroom responses. Each slider is a curve over the rendered image; values between
//! the measured slider positions are interpolated linearly, with 0 as the identity.
use super::basic_tone_data::{BLACKS, CONTRAST, DEHAZE, DEHAZE_VALUES, SLIDER_VALUES, WHITES};

const SIZE: usize = 1024;

/// Composed Dehaze → Contrast → Whites → Blacks curve, sampled at `SIZE + 1` points over 0–1.
#[derive(Clone)]
pub(crate) struct BasicTone {
    pub(crate) lut: Vec<f32>,
}
impl BasicTone {
    pub(crate) fn new(contrast: f32, whites: f32, blacks: f32, dehaze: f32) -> Option<Self> {
        if contrast == 0. && whites == 0. && blacks == 0. && dehaze == 0. {
            return None;
        }
        let lut = (0..=SIZE)
            .map(|i| {
                let x = i as f32 / SIZE as f32;
                let x = slider(&DEHAZE_VALUES, &DEHAZE, dehaze, x);
                let x = slider(&SLIDER_VALUES, &CONTRAST, contrast, x);
                let x = slider(&SLIDER_VALUES, &WHITES, whites, x);
                slider(&SLIDER_VALUES, &BLACKS, blacks, x)
            })
            // Measured tables carry small non-monotone noise; tone must never invert.
            .scan(0f32, |max, y| {
                *max = max.max(y);
                Some(*max)
            })
            .collect();
        Some(Self { lut })
    }
    pub(crate) fn eval(&self, x: f32) -> f32 {
        let f = x.clamp(0., 1.) * SIZE as f32;
        let i = (f as usize).min(SIZE - 1);
        self.lut[i] + (self.lut[i + 1] - self.lut[i]) * (f - i as f32)
    }
    /// DNG RGBTone: the curve maps the largest and smallest channel, and the middle
    /// channel keeps its relative position, preserving hue.
    pub(crate) fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let p = p.map(|v| v.clamp(0., 1.));
        let lo = p.into_iter().fold(f32::INFINITY, f32::min);
        let hi = p.into_iter().fold(0f32, f32::max);
        let (a, b) = (self.eval(lo), self.eval(hi));
        if hi - lo > 1e-8 {
            p.map(|v| a + (b - a) * (v - lo) / (hi - lo))
        } else {
            [a; 3]
        }
    }
}

/// Dehaze → Contrast → Whites → Blacks at `x`, without `BasicTone`'s table and
/// monotone clean-up: the local adjustments evaluate it per pixel.
pub(crate) fn compose(contrast: f32, whites: f32, blacks: f32, dehaze: f32, x: f32) -> f32 {
    let x = slider(&DEHAZE_VALUES, &DEHAZE, dehaze, x);
    let x = slider(&SLIDER_VALUES, &CONTRAST, contrast, x);
    let x = slider(&SLIDER_VALUES, &WHITES, whites, x);
    slider(&SLIDER_VALUES, &BLACKS, blacks, x)
}
/// The measured tables in the order `develop.wgsl`'s local curves read them: Dehaze,
/// Contrast, Whites, Blacks, each 6 × 64 values, then the slider positions (Dehaze's,
/// then the others').
pub(crate) fn gpu_tables() -> Vec<f32> {
    let mut out: Vec<f32> = [&DEHAZE, &CONTRAST, &WHITES, &BLACKS]
        .into_iter()
        .flat_map(|t| t.iter().flatten().copied())
        .collect();
    out.extend(DEHAZE_VALUES);
    out.extend(SLIDER_VALUES);
    out
}
/// One measured table: curve for slider `s` at input `x`.
fn slider(values: &[f32; 6], table: &[[f32; 64]; 6], s: f32, x: f32) -> f32 {
    if s == 0. {
        return x;
    }
    let s = s.clamp(-1., 1.);
    // Bracketing measured positions, with the identity at 0.
    let mut points: Vec<(f32, Option<&[f32; 64]>)> = values
        .iter()
        .zip(table)
        .map(|(v, t)| (*v, Some(t)))
        .collect();
    points.insert(3, (0., None));
    let j = points
        .windows(2)
        .position(|w| s <= w[1].0)
        .unwrap_or(points.len() - 2);
    let (s0, t0) = points[j];
    let (s1, t1) = points[j + 1];
    let w = (s - s0) / (s1 - s0);
    let y0 = t0.map_or(x, |t| curve(t, x));
    let y1 = t1.map_or(x, |t| curve(t, x));
    y0 + (y1 - y0) * w
}

/// Linear interpolation between bin centres; linear extrapolation to 0 and 1.
fn curve(t: &[f32; 64], x: f32) -> f32 {
    let f = (x * 64. - 0.5).clamp(-0.5, 63.5);
    let i = (f.floor() as isize).clamp(0, 62) as usize;
    let y = t[i] + (t[i + 1] - t[i]) * (f - i as f32);
    y.clamp(0., 1.)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neutral_sliders_are_identity_and_curves_are_monotone() {
        assert!(BasicTone::new(0., 0., 0., 0.).is_none());
        for s in [-1., -0.6, -0.1, 0.1, 0.4, 1.] {
            for (c, w, b, d) in [
                (s, 0., 0., 0.),
                (0., s, 0., 0.),
                (0., 0., s, 0.),
                (0., 0., 0., s),
            ] {
                let t = BasicTone::new(c, w, b, d).unwrap();
                assert!(
                    t.lut.windows(2).all(|p| p[1] >= p[0] - 1e-4),
                    "{c} {w} {b} {d}"
                );
                assert!(t.lut.iter().all(|v| (0. ..=1.).contains(v)));
            }
        }
        // A small slider value changes the curve only slightly.
        let t = BasicTone::new(0.01, 0., 0., 0.).unwrap();
        assert!((0..=10).all(|i| (t.eval(i as f32 / 10.) - i as f32 / 10.).abs() < 0.01));
        // Positive contrast darkens shadows and brightens highlights.
        let t = BasicTone::new(0.5, 0., 0., 0.).unwrap();
        assert!(t.eval(0.2) < 0.2 && t.eval(0.8) > 0.8);
        let gray = t.apply([0.3; 3]);
        assert!((gray[0] - gray[1]).abs() < 1e-6 && (gray[1] - gray[2]).abs() < 1e-6);
    }
}
