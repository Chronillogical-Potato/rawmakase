//! Engine 4 Shadows and Highlights: an edge-aware local tone operator fitted to Camera
//! Raw. The base level is a guided filter of log2 luminance of the toned image, with a
//! radius of 3.2% of the long edge and ε = 1.5 (log2 units squared). The measured
//! tables in `local_tone_data.rs` give the log2 gain for each base level relative to an
//! image key. The base level and keys come from a reduced copy of the photo, so the
//! result does not depend on the rendered region or preview size.
use super::local_tone_data::{Family, HIGHLIGHTS, SHADOWS, SLIDER_VALUES};
use rayon::prelude::*;

/// Long edge of the reduced image the base level is computed on.
const MAP_EDGE: u32 = 512;
const RADIUS: f32 = 0.032;
const EPSILON: f32 = 1.5;

pub(crate) struct LocalToneMap {
    pub(crate) width: usize,
    pub(crate) height: usize,
    /// Guided-filter coefficients: base = a · log2(Y) + b.
    pub(crate) a: Vec<f32>,
    pub(crate) b: Vec<f32>,
    /// Source image size, to convert sample coordinates.
    pub(crate) scale: [f32; 2],
    pub(crate) shadows: Option<Curve>,
    pub(crate) highlights: Option<Curve>,
}
pub(crate) struct Curve {
    pub(crate) key: f32,
    pub(crate) lo: f32,
    pub(crate) hi: f32,
    pub(crate) table: [f32; 48],
}
impl Curve {
    fn new(family: &Family, s: f32, key: f32) -> Option<Self> {
        if s == 0. {
            return None;
        }
        let s = s.clamp(-1., 1.);
        let mut table = [0.; 48];
        // Interpolate between measured positions; 0 is no change.
        let mut points: Vec<(f32, Option<&[f32; 48]>)> = SLIDER_VALUES
            .iter()
            .zip(&family.tables)
            .map(|(v, t)| (*v, Some(t)))
            .collect();
        points.insert(3, (0., None));
        let j = points
            .windows(2)
            .position(|w| s <= w[1].0)
            .unwrap_or(points.len() - 2);
        let ((s0, t0), (s1, t1)) = (points[j], points[j + 1]);
        let w = (s - s0) / (s1 - s0);
        for (i, v) in table.iter_mut().enumerate() {
            let y0 = t0.map_or(0., |t| t[i]);
            let y1 = t1.map_or(0., |t| t[i]);
            *v = y0 + (y1 - y0) * w;
        }
        Some(Self {
            key,
            lo: family.lo,
            hi: family.hi,
            table,
        })
    }
    fn eval(&self, base: f32) -> f32 {
        let f = ((base - self.key - self.lo) / (self.hi - self.lo) * 48. - 0.5).clamp(0., 47.);
        let i = (f as usize).min(46);
        self.table[i] + (self.table[i + 1] - self.table[i]) * (f - i as f32)
    }
}
pub(crate) fn luminance(rgb: [f32; 3]) -> f32 {
    (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]).max(6e-4)
}
impl LocalToneMap {
    /// `tone` maps a camera sample to linear display RGB after the profile tone curve.
    pub(crate) fn build(
        im: super::pipeline::Source,
        shadows: f32,
        highlights: f32,
        tone: impl Fn([f32; 3]) -> [f32; 3] + Sync,
    ) -> Option<Self> {
        if shadows == 0. && highlights == 0. {
            return None;
        }
        let small = super::pipeline::preview_source(im, MAP_EDGE);
        let (w, h) = (small.width as usize, small.height as usize);
        let lum: Vec<f32> = small
            .pixels
            .par_iter()
            .map(|p| luminance(tone(*p)))
            .collect();
        let logs: Vec<f32> = lum.iter().map(|y| y.log2()).collect();
        let percentile = |q: f32| {
            let mut v = lum.clone();
            let k = ((v.len() - 1) as f32 * q) as usize;
            v.select_nth_unstable_by(k, f32::total_cmp);
            v[k].log2()
        };
        let r = ((RADIUS * w.max(h) as f32).round() as usize).max(1);
        // He et al. guided filter with the image as its own guide.
        let mean = |x: &[f32]| blur(x, w, h, r);
        let m = mean(&logs);
        let sq: Vec<f32> = logs.iter().map(|v| v * v).collect();
        let m2 = mean(&sq);
        let a: Vec<f32> = m
            .iter()
            .zip(&m2)
            .map(|(m, m2)| {
                let var = (m2 - m * m).max(0.);
                var / (var + EPSILON)
            })
            .collect();
        let b: Vec<f32> = m.iter().zip(&a).map(|(m, a)| m - a * m).collect();
        Some(Self {
            width: w,
            height: h,
            a: mean(&a),
            b: mean(&b),
            scale: [w as f32 / im.width as f32, h as f32 / im.height as f32],
            shadows: Curve::new(&SHADOWS, shadows, percentile(SHADOWS.percentile)),
            highlights: Curve::new(&HIGHLIGHTS, highlights, percentile(HIGHLIGHTS.percentile)),
        })
    }
    /// Luminance gain for a toned pixel at camera-image sample position `x`, `y`.
    pub(crate) fn gain(&self, x: f32, y: f32, rgb: [f32; 3]) -> f32 {
        let fx = ((x + 0.5) * self.scale[0] - 0.5).clamp(0., (self.width - 1) as f32);
        let fy = ((y + 0.5) * self.scale[1] - 0.5).clamp(0., (self.height - 1) as f32);
        let (ix, iy) = (fx as usize, fy as usize);
        let (jx, jy) = ((ix + 1).min(self.width - 1), (iy + 1).min(self.height - 1));
        let (tx, ty) = (fx - ix as f32, fy - iy as f32);
        let bilinear = |v: &[f32]| {
            let top = v[iy * self.width + ix] * (1. - tx) + v[iy * self.width + jx] * tx;
            let bottom = v[jy * self.width + ix] * (1. - tx) + v[jy * self.width + jx] * tx;
            top * (1. - ty) + bottom * ty
        };
        let base = bilinear(&self.a) * luminance(rgb).log2() + bilinear(&self.b);
        let ev = self.shadows.as_ref().map_or(0., |c| c.eval(base))
            + self.highlights.as_ref().map_or(0., |c| c.eval(base));
        ev.exp2()
    }
}
/// Mean over a (2r+1)² window, clamped at the borders, via running sums.
fn blur(x: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let pass = |src: &[f32], len: usize, count: usize, at: &dyn Fn(usize, usize) -> usize| {
        let mut out = vec![0.; src.len()];
        for line in 0..count {
            let get = |i: isize| src[at(line, i.clamp(0, len as isize - 1) as usize)] as f64;
            let mut sum: f64 = (-(r as isize)..=r as isize).map(get).sum();
            for i in 0..len {
                out[at(line, i)] = (sum / (2 * r + 1) as f64) as f32;
                sum += get(i as isize + r as isize + 1) - get(i as isize - r as isize);
            }
        }
        out
    };
    let rows = pass(x, w, h, &|line, i| line * w + i);
    pass(&rows, h, w, &|line, i| i * w + line)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blur_preserves_constants_and_means() {
        let x = vec![2.; 30];
        assert!(blur(&x, 6, 5, 2).iter().all(|v| (v - 2.).abs() < 1e-6));
        let mut x = vec![0.; 25];
        x[12] = 25.;
        let b = blur(&x, 5, 5, 1);
        assert!((b[12] - 25. / 9.).abs() < 1e-5);
    }
    #[test]
    fn curves_interpolate_and_vanish_at_zero() {
        assert!(Curve::new(&SHADOWS, 0., 0.).is_none());
        let c = Curve::new(&SHADOWS, 0.6, -1.).unwrap();
        // Positive Shadows lifts dark bases and leaves the key level almost unchanged.
        assert!(c.eval(-7.) > 0.5);
        assert!(c.eval(-1.).abs() < 0.1);
        let half = Curve::new(&SHADOWS, 0.15, -1.).unwrap();
        let full = Curve::new(&SHADOWS, 0.3, -1.).unwrap();
        assert!((half.eval(-6.) - full.eval(-6.) / 2.).abs() < 1e-5);
        let h = Curve::new(&HIGHLIGHTS, -0.6, -3.).unwrap();
        assert!(h.eval(0.) < -0.2);
    }
}
