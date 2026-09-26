//! Engine 4 color grading, as measured Camera Raw responses. Each region's tint is a
//! per-channel gain of linear ProPhoto RGB that depends on the pixel's luminance;
//! Luminance sliders shift sRGB-encoded ProPhoto values by luminance. Tables are
//! measured at Saturation 50 for six hues and interpolated in hue; saturation scales
//! the log gain linearly. Only the default Blending and Balance were measured, so other
//! settings keep the previous operator (see `ColorGrade::new`).
use super::{
    Recipe,
    color_grade_data::{BINS, LUMINANCE, TINT},
};
use crate::color_math::{mul, srgb_decode, srgb_encode};

pub(crate) struct ColorGrade {
    /// Per luminance bin: log2 gains and encoded offsets, summed over regions.
    pub(crate) gain: [[f32; 3]; BINS],
    pub(crate) offset: [[f32; 3]; BINS],
}
impl ColorGrade {
    /// `None` when grading is inactive or uses Blending/Balance settings the tables do
    /// not cover.
    pub(crate) fn new(r: &Recipe) -> Option<Self> {
        let regions = [
            r.grading[0],
            r.grading[1],
            r.grading[2],
            r.effects.global_grade,
        ];
        if regions.iter().all(|g| g[1] == 0. && g[2] == 0.) {
            return None;
        }
        if (r.effects.blending - 0.5).abs() > 1e-4 || r.effects.balance.abs() > 1e-4 {
            return None;
        }
        let mut gain = [[0.; 3]; BINS];
        let mut offset = [[0.; 3]; BINS];
        for (region, [hue, sat, lum]) in regions.into_iter().enumerate() {
            if sat != 0. {
                let f = hue.rem_euclid(1.) * 6.;
                let (i, t) = (f as usize % 6, f.fract());
                let (a, b) = (&TINT[region][i], &TINT[region][(i + 1) % 6]);
                let scale = sat / 0.5;
                for bin in 0..BINS {
                    for c in 0..3 {
                        gain[bin][c] += (a[bin][c] * (1. - t) + b[bin][c] * t) * scale;
                    }
                }
            }
            if lum != 0. {
                let table = &LUMINANCE[region][usize::from(lum > 0.)];
                let scale = lum.abs() / 0.5;
                for bin in 0..BINS {
                    for c in 0..3 {
                        offset[bin][c] += table[bin][c] * scale;
                    }
                }
            }
        }
        Some(Self { gain, offset })
    }
    fn at(table: &[[f32; 3]; BINS], l: f32) -> [f32; 3] {
        let f = (l.clamp(0., 1.) * BINS as f32 - 0.5).clamp(0., (BINS - 1) as f32);
        let i = (f as usize).min(BINS - 2);
        let t = f - i as f32;
        std::array::from_fn(|c| table[i][c] * (1. - t) + table[i + 1][c] * t)
    }
    /// `rgb` is linear display RGB (sRGB primaries).
    pub(crate) fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let y = (0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]).max(0.);
        let l = srgb_encode(y.min(1.));
        let g = Self::at(&self.gain, l);
        let o = Self::at(&self.offset, l);
        let p = mul(crate::camera_profiles::RGB_TO_PRO, rgb);
        let p: [f32; 3] = std::array::from_fn(|c| {
            let v = p[c].max(0.) * g[c].exp2();
            srgb_decode((srgb_encode(v.clamp(0., 1.)) + o[c]).clamp(0., 1.))
        });
        mul(crate::camera_profiles::PRO_TO_RGB, p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inactive_or_unmeasured_settings_fall_back() {
        assert!(ColorGrade::new(&Recipe::default()).is_none());
        let mut r = Recipe::default();
        r.grading[0] = [240. / 360., 0.5, 0.];
        assert!(ColorGrade::new(&r).is_some());
        r.effects.balance = 0.3;
        assert!(ColorGrade::new(&r).is_none());
    }
    #[test]
    fn blue_shadow_tint_cools_shadows_more_than_highlights() {
        let mut r = Recipe::default();
        r.effects.blending = 0.5;
        r.grading[0] = [240. / 360., 0.5, 0.];
        let g = ColorGrade::new(&r).unwrap();
        let cool = |p: [f32; 3]| p[2] / p[0].max(1e-6);
        let dark = g.apply([0.02; 3]);
        let bright = g.apply([0.8; 3]);
        assert!(cool(dark) > 1.05, "{dark:?}");
        assert!(cool(dark) > cool(bright));
    }
}
