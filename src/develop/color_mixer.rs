//! Engine 4 color mixer (HSL), Saturation and Vibrance, as measured Camera Raw responses.
//!
//! Camera Raw's color mixer behaves like a hue/saturation/value lookup in linear
//! ProPhoto RGB. For each slider at its extremes (±100; ±50 for Saturation and
//! Vibrance), `color_mixer.bin` holds the change it makes to the default rendering:
//! hue shift (turns), log2 saturation and log2 value factors, on a grid of 36 hues ×
//! 6 saturations (spaced by √s) × 6 values (spaced by v^0.45). The grid was measured
//! on nine photos (Fujifilm X100F, Sony A7 II and A7CR); held-out photos reproduce each
//! slider to 0.0002–0.0066 MAE. Slider positions scale the hue shift, the value factor's
//! log and positive saturation's log linearly; negative saturation scales the factor
//! itself linearly, which matches Camera Raw at −25 and −50. Several sliders add their
//! changes. See docs/color-mixer.md.
use super::Recipe;
use crate::color_math::mul;

const HUES: usize = 36;
const SATS: usize = 6;
const VALS: usize = 6;
const CELLS: usize = HUES * SATS * VALS;
const TABLE: usize = 3 * CELLS;
/// 8 bands × (hue, saturation, luminance) × (−, +), then Saturation −/+, Vibrance −/+.
const TABLES: usize = 52;
const SCALE: f32 = 1. / 8000.;
static DATA: &[u8] = include_bytes!("color_mixer.bin");

fn value(table: usize, channel: usize, cell: usize) -> f32 {
    let i = 2 * (table * TABLE + channel * CELLS + cell);
    i16::from_le_bytes([DATA[i], DATA[i + 1]]) as f32 * SCALE
}

/// The combined change of all active sliders, one grid.
pub(crate) struct ColorMixer {
    pub(crate) delta: Vec<[f32; 3]>,
}
impl ColorMixer {
    pub(crate) fn new(r: &Recipe) -> Option<Self> {
        debug_assert_eq!(DATA.len(), TABLES * TABLE * 2);
        let mut active: Vec<(usize, f32)> = Vec::new();
        for (band, controls) in r.hsl.iter().enumerate() {
            for (kind, s) in controls.iter().enumerate() {
                if *s != 0. {
                    let sign = usize::from(*s > 0.);
                    active.push(((band * 3 + kind) * 2 + sign, s.abs().min(1.)));
                }
            }
        }
        // Measured at ±50: positions beyond extrapolate linearly.
        for (i, s) in [r.saturation, r.vibrance].into_iter().enumerate() {
            if s != 0. {
                active.push((48 + i * 2 + usize::from(s > 0.), (s.abs() * 2.).min(2.)));
            }
        }
        if active.is_empty() {
            return None;
        }
        let delta = (0..CELLS)
            .map(|cell| {
                std::array::from_fn(|c| {
                    active
                        .iter()
                        .map(|(t, w)| {
                            let v = value(*t, c, cell);
                            // Negative saturation sliders scale the saturation factor
                            // linearly, as Camera Raw does: scaling the log factor of a
                            // strong measured desaturation overshoots at −25 and −50.
                            // Even tables are the negative extremes.
                            if c == 1 && t % 2 == 0 {
                                (1. + w * (v.exp2() - 1.)).max(1e-3).log2()
                            } else {
                                v * w
                            }
                        })
                        .sum::<f32>()
                })
            })
            .collect();
        Some(Self { delta })
    }
    /// Trilinear lookup between grid centres; hue wraps.
    fn lookup(&self, h: f32, s: f32, v: f32) -> [f32; 3] {
        let fh = h.rem_euclid(1.) * HUES as f32 - 0.5;
        let fs = (s.clamp(0., 1.).sqrt() * SATS as f32 - 0.5).clamp(0., (SATS - 1) as f32);
        let fv = (v.clamp(0., 1.).powf(0.45) * VALS as f32 - 0.5).clamp(0., (VALS - 1) as f32);
        let h0 = fh.floor();
        let (th, ts, tv) = (fh - h0, fs.fract(), fv.fract());
        let hi = |d: isize| (h0 as isize + d).rem_euclid(HUES as isize) as usize;
        let (s0, v0) = (fs as usize, fv as usize);
        let (s1, v1) = ((s0 + 1).min(SATS - 1), (v0 + 1).min(VALS - 1));
        let mut out = [0.; 3];
        for (dh, wh) in [(0, 1. - th), (1, th)] {
            for (si, ws) in [(s0, 1. - ts), (s1, ts)] {
                for (vi, wv) in [(v0, 1. - tv), (v1, tv)] {
                    let d = self.delta[(hi(dh) * SATS + si) * VALS + vi];
                    let w = wh * ws * wv;
                    for c in 0..3 {
                        out[c] += d[c] * w;
                    }
                }
            }
        }
        out
    }
    /// `rgb` is linear display RGB (sRGB primaries).
    pub(crate) fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let p = mul(crate::camera_profiles::RGB_TO_PRO, rgb).map(|v| v.max(0.));
        let max = p.into_iter().fold(0f32, f32::max);
        let min = p.into_iter().fold(f32::INFINITY, f32::min);
        if max <= 1e-6 {
            return rgb;
        }
        let d = max - min;
        let h = if d < 1e-9 {
            0.
        } else if max == p[0] {
            ((p[1] - p[2]) / d).rem_euclid(6.) / 6.
        } else if max == p[1] {
            ((p[2] - p[0]) / d + 2.) / 6.
        } else {
            ((p[0] - p[1]) / d + 4.) / 6.
        };
        let s = d / max;
        let [dh, ds, dv] = self.lookup(h, s, max);
        let h = (h + dh).rem_euclid(1.) * 6.;
        let s = (s * ds.exp2()).clamp(0., 1.);
        let v = max * dv.exp2();
        let c = v * s;
        let x = c * (1. - (h % 2. - 1.).abs());
        let q = match h as usize {
            0 => [c, x, 0.],
            1 => [x, c, 0.],
            2 => [0., c, x],
            3 => [0., x, c],
            4 => [x, 0., c],
            _ => [c, 0., x],
        };
        mul(
            crate::camera_profiles::PRO_TO_RGB,
            q.map(|v| v + (max * dv.exp2()) - c),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn table_blob_has_expected_size_and_neutrals_stay_neutral() {
        assert_eq!(DATA.len(), TABLES * TABLE * 2);
        assert!(ColorMixer::new(&Recipe::default()).is_none());
        let mut r = Recipe::default();
        r.hsl[1] = [0.5, 1., -1.];
        r.saturation = 0.3;
        let m = ColorMixer::new(&r).unwrap();
        let gray = m.apply([0.2; 3]);
        assert!((gray[0] - gray[1]).abs() < 1e-4 && (gray[1] - gray[2]).abs() < 1e-4);
        assert!(m.apply([0.3, 0.2, 0.1]).iter().all(|v| v.is_finite()));
    }
    #[test]
    fn orange_luminance_darkens_skin_tones() {
        let mut r = Recipe::default();
        let skin = [0.5, 0.3, 0.2];
        let lum = |p: [f32; 3]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
        r.hsl[1][2] = -1.;
        let darker = ColorMixer::new(&r).unwrap().apply(skin);
        r.hsl[1][2] = 1.;
        let brighter = ColorMixer::new(&r).unwrap().apply(skin);
        assert!(lum(darker) < lum(skin) && lum(brighter) > lum(skin));
        // Saturation −100 of every band approaches gray.
        let r = Recipe {
            saturation: -1.,
            ..Default::default()
        };
        let p = ColorMixer::new(&r).unwrap().apply(skin);
        let spread = |p: [f32; 3]| {
            p.iter().fold(0f32, |a, b| a.max(*b)) - p.iter().fold(1f32, |a, b| a.min(*b))
        };
        assert!(spread(p) < spread(skin) * 0.3);
    }
    #[test]
    fn negative_saturation_scales_the_factor_linearly() {
        let at = |s: f32| {
            let mut r = Recipe::default();
            r.hsl[3][1] = s;
            ColorMixer::new(&r).unwrap().delta
        };
        let (full, half) = (at(-1.), at(-0.5));
        for (f, h) in full.iter().zip(&half) {
            let expected = (1. + 0.5 * (f[1].exp2() - 1.)).max(1e-3).log2();
            assert!((h[1] - expected).abs() < 1e-5);
            // Hue and value keep scaling their measured change linearly.
            assert!((h[0] - f[0] * 0.5).abs() < 1e-6 && (h[2] - f[2] * 0.5).abs() < 1e-6);
        }
        // Positive saturation keeps scaling the log factor.
        let (full, half) = (at(1.), at(0.5));
        assert!(
            full.iter()
                .zip(&half)
                .all(|(f, h)| (h[1] - f[1] * 0.5).abs() < 1e-6)
        );
    }
}
