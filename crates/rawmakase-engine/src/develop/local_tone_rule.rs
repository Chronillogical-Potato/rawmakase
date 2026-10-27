//! The global rule of positive Shadows and negative Highlights: Camera Raw's gain for a
//! base level is a function of where that level sits in the photo's histogram of base
//! levels (how far below the top, above the bottom and from the mean and quantiles, the
//! mass in the stops around it, the range) and of how textured the photo is at the
//! top. Measured on flat-patch mosaics with controlled histograms (including deep
//! ranges, bimodal histograms and clipped tops) and fitted together with
//! linear-profile training photos, targets counted to 13 stops under white
//! (2026-10-28; docs/scene-tone-stage.md). A flat maximum is a degenerate case for Camera Raw, so
//! the mosaics carry a faint texture. The slider scales the +100 / −100 gain linearly
//! (measured exactly at ±50).
//!
//! The rule is a small network (`local_tone_rule_data.rs`; two hidden layers of
//! [`HIDDEN`] units) evaluated once per photo at
//! 48 base levels, which gives a [`Curve`] the per-pixel code and the GPU read like any
//! other. With `RAWMAKASE_RULE_DUMP=<path>` set, each curve built appends its statistics
//! and table to `<path>` as a JSON line and writes the map it read to `<path>.map`
//! (diagnostics; `scene-tone-rule.py taps` trains on those maps).
use super::local_tone::{Curve, blur};
use super::local_tone_rule_data::{B1, B2, B3, MU, SD, W1, W2, W3};

/// The network's width, from the generated weights.
const HIDDEN: usize = W1.len();
/// Radius (map pixels) of the mean whose residual measures texture.
const TEXTURE_RADIUS: usize = 2;
/// How far below the top (stops) texture is measured for `tex_top`.
const TOP_BAND: f32 = 2.;
/// Texture beyond this (log2 RMS) counts no further: the mosaics measured a step at a flat
/// top and a slow growth after it; photos are always past the step, so the inputs are
/// clamped rather than extrapolated (`pipeline.py`).
const TEX_CLAMP: f32 = 0.6;
const TEX_TOP_CLAMP: f32 = 0.05;

/// Histogram statistics of a photo's base levels (its measurement copy).
pub(super) struct RuleStats {
    sorted: Vec<f32>,
    top: f32,
    bot: f32,
    mean: f32,
    q10: f32,
    q25: f32,
    q50: f32,
    q75: f32,
    q90: f32,
    tex: f32,
    tex_top: f32,
}
impl RuleStats {
    /// From the map's log2 luminance and base level (a · log + b), `w` × `h`.
    pub(super) fn new(logs: &[f32], base: &[f32], w: usize, h: usize) -> Self {
        if let Ok(path) = std::env::var("RAWMAKASE_RULE_DUMP") {
            // Diagnostics: the map's log luminance and base level as raw f32 (w, h, logs, base).
            let mut bytes = Vec::with_capacity(8 + 8 * logs.len());
            bytes.extend((w as f32).to_le_bytes());
            bytes.extend((h as f32).to_le_bytes());
            for v in logs.iter().chain(base) {
                bytes.extend(v.to_le_bytes());
            }
            let _ = std::fs::write(format!("{path}.map"), bytes);
        }
        let mut sorted = base.to_vec();
        sorted.sort_by(f32::total_cmp);
        let n = sorted.len();
        let q = |p: f32| sorted[((p * (n - 1) as f32) as usize).min(n - 1)];
        let mean = (sorted.iter().map(|v| *v as f64).sum::<f64>() / n as f64) as f32;
        // The maximum, not a percentile: a small bright object sets the top, and the
        // mass features then see it as a small mass (the textured-square probes).
        let top = sorted[n - 1];
        let smooth = blur(logs, w, h, TEXTURE_RADIUS);
        let (mut sum, mut sum_top, mut count_top) = (0f64, 0f64, 0usize);
        for ((l, s), b) in logs.iter().zip(&smooth).zip(base) {
            let d = f64::from(l - s) * f64::from(l - s);
            sum += d;
            if *b > top - TOP_BAND {
                sum_top += d;
                count_top += 1;
            }
        }
        let tex = (sum / n as f64).sqrt() as f32;
        let tex_top = if count_top > 0 {
            (sum_top / count_top as f64).sqrt() as f32
        } else {
            tex
        };
        Self {
            top,
            bot: q(0.001),
            mean,
            q10: q(0.1),
            q25: q(0.25),
            q50: q(0.5),
            q75: q(0.75),
            q90: q(0.9),
            tex,
            tex_top,
            sorted,
        }
    }
    /// Fraction of base levels at or below `x`.
    fn cdf(&self, x: f32) -> f32 {
        self.sorted.partition_point(|v| *v <= x) as f32 / self.sorted.len() as f32
    }
    /// The network's inputs for base level `x` (`pipeline.py`'s `FEAT`, in order).
    fn features(&self, x: f32) -> [f32; 20] {
        let m = |lo: f32, hi: f32| self.cdf(x + hi) - self.cdf(x + lo);
        [
            self.top - x,
            x - self.bot,
            self.mean - x,
            self.q50 - x,
            self.q75 - x,
            self.q90 - x,
            self.q25 - x,
            self.q10 - x,
            self.cdf(x),
            self.top - self.bot,
            m(0., 1.),
            m(1., 2.),
            m(2., 3.),
            m(3., 5.),
            m(-1., 0.),
            m(-2., -1.),
            m(-3., -2.),
            m(-5., -3.),
            self.tex.min(TEX_CLAMP),
            self.tex_top.min(TEX_TOP_CLAMP) / TEX_TOP_CLAMP,
        ]
    }
    /// Log2 gain at base level `x`: Shadows +100, Highlights −100.
    pub(super) fn gain(&self, x: f32) -> [f32; 2] {
        let f = self.features(x);
        let mut h1 = [0f32; HIDDEN];
        for (j, out) in h1.iter_mut().enumerate() {
            let mut s = B1[j];
            for (i, v) in f.iter().enumerate() {
                s += W1[j][i] * (v - MU[i]) / SD[i];
            }
            *out = silu(s);
        }
        let mut h2 = [0f32; HIDDEN];
        for (j, out) in h2.iter_mut().enumerate() {
            let mut s = B2[j];
            for (i, v) in h1.iter().enumerate() {
                s += W2[j][i] * v;
            }
            *out = silu(s);
        }
        std::array::from_fn(|k| {
            let mut s = B3[k];
            for (i, v) in h2.iter().enumerate() {
                s += W3[k][i] * v;
            }
            s
        })
    }
    /// The curve of one control at full strength (Shadows +100 when `shadows`, else
    /// Highlights −100) over the photo's base levels, as [`Curve`] reads it: absolute
    /// levels (key 0) from a stop below the bottom to a stop above the top, held
    /// outside. Shadows lifts nothing above the top and Highlights pulls nothing below
    /// the bottom, so holding the ends extends the rule as the mosaics measured it.
    pub(super) fn curve(&self, shadows: bool) -> Curve {
        let (lo, hi) = (self.bot - 1., self.top + 1.);
        let step = (hi - lo) / 48.;
        let mut table: [f32; 48] = std::array::from_fn(|i| {
            let g = self.gain(lo + (i as f32 + 0.5) * step);
            if shadows { g[0].max(0.) } else { g[1].min(0.) }
        });
        monotone(&mut table, step);
        if let Ok(path) = std::env::var("RAWMAKASE_RULE_DUMP") {
            // Diagnostics: the statistics and curve, one JSON line per curve.
            let line = format!(
                "{{\"shadows\":{shadows},\"top\":{},\"bot\":{},\"mean\":{},\"q\":[{},{},{},{},{}],\"tex\":{},\"tex_top\":{},\"n\":{},\"lo\":{lo},\"hi\":{hi},\"table\":{:?}}}\n",
                self.top,
                self.bot,
                self.mean,
                self.q10,
                self.q25,
                self.q50,
                self.q75,
                self.q90,
                self.tex,
                self.tex_top,
                self.sorted.len(),
                table
            );
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
        }
        Curve {
            key: 0.,
            lo,
            hi,
            table,
            tail: 0.,
        }
    }
}
/// Projects the output log luminance (level + gain) of a 48-column table with columns
/// `step` apart onto nondecreasing values, by pooled adjacent violators: the
/// least-squares correction, as `Family::monotone` does for the fitted tables. A
/// brighter base never renders darker than a dimmer one, so a clipped plateau keeps
/// no bright rim (`clipped_highlights_do_not_solarize`); flat compression is allowed.
fn monotone(table: &mut [f32; 48], step: f32) {
    let mut sums = [0f32; 48];
    let mut counts = [0usize; 48];
    let mut blocks = 0;
    for (i, g) in table.iter().enumerate() {
        sums[blocks] = g + i as f32 * step;
        counts[blocks] = 1;
        blocks += 1;
        while blocks > 1
            && sums[blocks - 2] / counts[blocks - 2] as f32
                > sums[blocks - 1] / counts[blocks - 1] as f32
        {
            sums[blocks - 2] += sums[blocks - 1];
            counts[blocks - 2] += counts[blocks - 1];
            blocks -= 1;
        }
    }
    let mut i = 0;
    for block in 0..blocks {
        for _ in 0..counts[block] {
            if counts[block] > 1 {
                table[i] = sums[block] / counts[block] as f32 - i as f32 * step;
            }
            i += 1;
        }
    }
}
fn silu(x: f32) -> f32 {
    x / (1. + (-x).exp())
}
/// `unit` scaled to slider fraction `s` (the rule is linear in the slider).
pub(super) fn at(unit: &Curve, s: f32) -> Curve {
    Curve {
        key: unit.key,
        lo: unit.lo,
        hi: unit.hi,
        table: unit.table.map(|v| v * s.abs()),
        tail: 0.,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotone_never_lets_a_brighter_level_render_darker() {
        let step = 0.25;
        // A pull that steepens past -1 per stop over the top columns.
        let mut t: [f32; 48] = std::array::from_fn(|i| -(i as f32) * 0.4);
        monotone(&mut t, step);
        for i in 1..48 {
            assert!(
                t[i] + i as f32 * step >= t[i - 1] + (i - 1) as f32 * step - 1e-5,
                "{i}"
            );
        }
        // An already nondecreasing output is untouched.
        let mut u: [f32; 48] = std::array::from_fn(|i| -(i as f32) * 0.1);
        let before = u;
        monotone(&mut u, step);
        assert_eq!(u, before);
    }

    /// A 16-level staircase over 6 stops with faint texture, as the mosaics: the rule
    /// lifts the darkest levels by about a stop at Shadows +100, leaves the top stops
    /// alone, and pulls the top at Highlights −100 while leaving the bottom.
    #[test]
    fn staircase_follows_the_measured_shape() {
        let (w, h) = (64, 64);
        let mut logs = vec![0f32; w * h];
        for (i, v) in logs.iter_mut().enumerate() {
            let (x, y) = (i % w, i / w);
            let patch = (y / 16) * 4 + x / 16;
            let noise = ((i * 7919) % 13) as f32 / 13. - 0.5;
            *v = -1.3 - 6. * patch as f32 / 15. + 0.05 * noise;
        }
        let stats = RuleStats::new(&logs, &logs, w, h);
        let s = stats.curve(true);
        let hc = stats.curve(false);
        let at = |c: &Curve, x: f32| {
            let f = ((x - c.lo) / (c.hi - c.lo) * 48. - 0.5).clamp(0., 47.);
            c.table[(f as usize).min(47)]
        };
        assert!(at(&s, -1.3) < 0.1, "top not lifted: {}", at(&s, -1.3));
        assert!(at(&s, -7.) > 0.6, "bottom lifted: {}", at(&s, -7.));
        assert!(at(&hc, -1.5) < -0.5, "top pulled: {}", at(&hc, -1.5));
        assert!(at(&hc, -7.) > -0.15, "bottom left: {}", at(&hc, -7.));
    }
}
