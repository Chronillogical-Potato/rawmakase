//! Additional photographic controls used by imported XMP recipes.
use crate::{
    curve::ToneCurve,
    develop::{Recipe, Rendered},
};
use anyhow::{Result, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Effects {
    pub channels: [ToneCurve; 3],
    pub parametric: [f32; 4],
    pub splits: [f32; 3],
    pub calibration: [[f32; 2]; 3],
    pub shadow_tint: f32,
    pub monochrome: bool,
    pub gray_mix: [f32; 8],
    pub balance: f32,
    pub blending: f32,
    pub global_grade: [f32; 3],
    pub clarity: f32,
    pub texture: f32,
    pub dehaze: f32,
    pub grain: f32,
    pub grain_size: f32,
    pub grain_roughness: f32,
    pub grain_seed: u32,
    pub vignette: f32,
    pub vignette_midpoint: f32,
    pub vignette_roundness: f32,
    pub vignette_feather: f32,
    pub vignette_highlights: f32,
    pub vignette_style: u8,
    pub lens_vignette: f32,
    pub lens_vignette_midpoint: f32,
    pub defringe: [f32; 2],
    pub defringe_ranges: [[f32; 2]; 2],
    pub luma_detail: f32,
    pub luma_contrast: f32,
    pub chroma_detail: f32,
    pub chroma_smoothness: f32,
}
impl Default for Effects {
    fn default() -> Self {
        Self {
            channels: std::array::from_fn(|_| ToneCurve::default()),
            parametric: [0.; 4],
            splits: [0.25, 0.5, 0.75],
            calibration: [[0.; 2]; 3],
            shadow_tint: 0.,
            monochrome: false,
            gray_mix: [0.; 8],
            balance: 0.,
            blending: 0.5,
            global_grade: [0.; 3],
            clarity: 0.,
            texture: 0.,
            dehaze: 0.,
            grain: 0.,
            grain_size: 0.25,
            grain_roughness: 0.5,
            grain_seed: 42,
            vignette: 0.,
            vignette_midpoint: 0.5,
            vignette_roundness: 0.,
            vignette_feather: 0.5,
            vignette_highlights: 0.,
            vignette_style: 0,
            lens_vignette: 0.,
            lens_vignette_midpoint: 0.5,
            defringe: [0.; 2],
            defringe_ranges: [[0.3, 0.7], [0.4, 0.6]],
            luma_detail: 0.5,
            luma_contrast: 0.,
            chroma_detail: 0.5,
            chroma_smoothness: 0.5,
        }
    }
}
impl Effects {
    pub fn validate(&self) -> Result<()> {
        for c in &self.channels {
            c.validate()?;
        }
        ensure!(
            self.parametric
                .iter()
                .chain(self.calibration.iter().flatten())
                .chain(self.gray_mix.iter())
                .chain([
                    &self.shadow_tint,
                    &self.balance,
                    &self.clarity,
                    &self.texture,
                    &self.dehaze,
                    &self.vignette,
                    &self.vignette_roundness,
                    &self.lens_vignette
                ])
                .all(|v| v.is_finite() && v.abs() <= 1.),
            "Invalid preset effect"
        );
        ensure!(
            self.splits[0] > 0.
                && self.splits[2] < 1.
                && self.splits.windows(2).all(|p| p[0] < p[1]),
            "Invalid parametric curve splits"
        );
        ensure!(
            [
                self.blending,
                self.grain,
                self.grain_size,
                self.grain_roughness,
                self.vignette_midpoint,
                self.vignette_feather,
                self.vignette_highlights,
                self.lens_vignette_midpoint,
                self.luma_detail,
                self.luma_contrast,
                self.chroma_detail,
                self.chroma_smoothness
            ]
            .iter()
            .chain(self.defringe.iter())
            .chain(self.defringe_ranges.iter().flatten())
            .all(|v| v.is_finite() && (0. ..=1.).contains(v)),
            "Invalid effect range"
        );
        ensure!(self.vignette_style <= 2, "Invalid vignette style");
        ensure!(
            self.global_grade
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 1.),
            "Invalid global grade"
        );
        Ok(())
    }
    pub fn calibrate(&self, mut p: [f32; 3]) -> [f32; 3] {
        for c in 0..3 {
            let [h, s] = self.calibration[c];
            if h == 0. && s == 0. {
                continue;
            }
            let source = p[c];
            let a = (c + 1) % 3;
            let b = (c + 2) % 3;
            p[a] += source * h * 0.12;
            p[b] -= source * h * 0.12;
            let gray = (p[0] + p[1] + p[2]) / 3.;
            p[c] += (p[c] - gray) * s * 0.35;
        }
        let y = (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]).max(0.);
        let tint = self.shadow_tint * (-y * 8.).exp() * y * 0.3;
        p[1] += tint;
        p[0] -= tint * 0.5;
        p[2] -= tint * 0.5;
        p
    }
    pub fn parametric(&self, x: f32) -> f32 {
        if self.parametric == [0.; 4] {
            return x;
        }
        let anchors = [0., self.splits[0], self.splits[1], self.splits[2], 1.];
        let mut delta = 0.;
        for i in 0..4 {
            let lo = anchors[i];
            let hi = anchors[i + 1];
            let mid = (lo + hi) * 0.5;
            let radius = (hi - lo) * 1.5;
            let w = (1. - ((x - mid) / radius).abs()).clamp(0., 1.);
            delta += self.parametric[i] * w * w * (3. - 2. * w) * 0.18;
        }
        (x + delta * 4. * x * (1. - x)).clamp(0., 1.)
    }
    /// Lightroom's Fringe Color Selector: points the Purple or Green hue range at the
    /// fringe colour `rgb` (encoded sRGB, as shown) and turns that Amount on if it is
    /// off. Returns which (0 purple, 1 green), or `None` when the colour is neither.
    pub fn pick_fringe(&mut self, rgb: [f32; 3]) -> Option<usize> {
        let lab = crate::develop::pipeline::srgb_to_lab(rgb.map(crate::color_math::srgb_decode));
        if lab[1].hypot(lab[2]) < 0.02 {
            return None;
        }
        let hue = lab[2].atan2(lab[1]).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        // The nearer window that holds the hue well inside its slider: the outer ends
        // reach reds, yellows and blues, which are not fringes.
        let (i, at) = DEFRINGE_CENTERS
            .into_iter()
            .map(|center| ((hue - center + 0.5).rem_euclid(1.) - 0.5) / DEFRINGE_WINDOW + 0.5)
            .enumerate()
            .filter(|(_, at)| (0.15..=0.85).contains(at))
            .min_by(|a, b| (a.1 - 0.5).abs().total_cmp(&(b.1 - 0.5).abs()))?;
        // A range 20 wide on Lightroom's 0–100 hue sliders around the picked colour.
        let lo = (at - 0.1).clamp(0., 0.8);
        self.defringe_ranges[i] = [lo, lo + 0.2];
        if self.defringe[i] == 0. {
            // Lightroom's Amount 5 of 20.
            self.defringe[i] = 0.25;
        }
        Some(i)
    }
    /// Lightroom's Defringe: chroma of hues inside the Purple and Green ranges is
    /// reduced, the more the higher Amount and the chroma (see
    /// docs/lens-corrections.md).
    pub fn defringe_color(&self, mut lab: [f32; 3], h: f32) -> [f32; 3] {
        for (i, center) in DEFRINGE_CENTERS.into_iter().enumerate() {
            if self.defringe[i] == 0. {
                continue;
            }
            let chroma = lab[1].hypot(lab[2]);
            let k = 1.
                - defringe_weight(h, center, self.defringe_ranges[i])
                    * defringe_strength(self.defringe[i], chroma);
            lab[1] *= k;
            lab[2] *= k;
        }
        lab
    }
}
/// Centres of the Purple and Green hue windows (Oklab hue, 0–1) and the hue span of
/// each Hue slider's 0–100, fitted to Camera Raw 18.6 renders.
const DEFRINGE_CENTERS: [f32; 2] = [0.875, 0.46];
const DEFRINGE_WINDOW: f32 = 0.5;
/// Half width of the soft edge of a hue range.
const DEFRINGE_SOFT: f32 = 0.025;
/// Share of chroma kept at full strength for a grey-ish colour, falling as chroma
/// rises (Oklab chroma scale), and the Amount (0–20) over which strength builds up.
const DEFRINGE_KEEP: f32 = 0.45;
const DEFRINGE_CHROMA: f32 = 0.09;
const DEFRINGE_RATE: f32 = 2.5;
/// How much of hue `h` the range `[lo, hi]` (0–1 of the Hue slider) of the window
/// centred at `center` selects.
fn defringe_weight(h: f32, center: f32, [lo, hi]: [f32; 2]) -> f32 {
    let d = (h - center + 0.5).rem_euclid(1.) - 0.5;
    let step = |x: f32| {
        let t = ((x + DEFRINGE_SOFT) / (2. * DEFRINGE_SOFT)).clamp(0., 1.);
        t * t * (3. - 2. * t)
    };
    step(d - (lo - 0.5) * DEFRINGE_WINDOW) * step((hi - 0.5) * DEFRINGE_WINDOW - d)
}
/// Share of chroma removed at full weight for Amount `amount` (0–1): Camera Raw
/// removes more of a stronger fringe colour.
fn defringe_strength(amount: f32, chroma: f32) -> f32 {
    (1. - DEFRINGE_KEEP * (-chroma / DEFRINGE_CHROMA).exp())
        * (1. - (-amount * 20. / DEFRINGE_RATE).exp())
}
fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut v = (x as u32).wrapping_mul(0x9e3779b9) ^ (y as u32).wrapping_mul(0x85ebca6b) ^ seed;
    v ^= v >> 16;
    v = v.wrapping_mul(0x7feb352d);
    v ^= v >> 15;
    v = v.wrapping_mul(0x846ca68b);
    v ^= v >> 16;
    (v as f64 / u32::MAX as f64 * 2. - 1.) as f32
}
fn grain(x: f32, y: f32, size: f32, seed: u32) -> f32 {
    let x = x / size;
    let y = y / size;
    let ix = x.floor() as i32;
    let iy = y.floor() as i32;
    let smooth = |v: f32| v * v * (3. - 2. * v);
    let a = smooth(x - ix as f32);
    let b = smooth(y - iy as f32);
    let n = hash(ix, iy, seed) * (1. - a) + hash(ix + 1, iy, seed) * a;
    let m = hash(ix, iy + 1, seed) * (1. - a) + hash(ix + 1, iy + 1, seed) * a;
    n * (1. - b) + m * b
}
pub fn spatial_finish(im: &mut Rendered, r: &Recipe, origin: [u32; 2], full: [u32; 2]) {
    spatial_finish_scaled(im, r, origin, full, 1.);
}
/// Vignettes and grain for an output with `scale` pixels per full-resolution pixel.
/// Grain keeps its full-resolution pattern and, like the full render resized, loses
/// amplitude where a preview pixel averages several grains.
pub(crate) fn spatial_finish_scaled(
    im: &mut Rendered,
    r: &Recipe,
    origin: [u32; 2],
    full: [u32; 2],
    scale: f32,
) {
    let e = &r.effects;
    if e.grain == 0. && e.vignette == 0. && e.lens_vignette == 0. {
        return;
    }
    im.pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
        let x = origin[0] + i as u32 % im.width;
        let y = origin[1] + i as u32 / im.width;
        let nx = ((x as f32 + 0.5) / full[0] as f32 - 0.5) * 2.;
        let ny = ((y as f32 + 0.5) / full[1] as f32 - 0.5) * 2.;
        let power = 2f32.powf(-e.vignette_roundness * 1.5 + 1.);
        let distance = (nx.abs().powf(power) + ny.abs().powf(power)).powf(1. / power);
        let start = e.vignette_midpoint * 0.9;
        let feather = (e.vignette_feather * 0.9 + 0.05).max(0.05);
        let t = ((distance - start) / feather).clamp(0., 1.);
        let mask = t * t * (3. - 2. * t);
        let l = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
        let protect = 1. - e.vignette_highlights * l.powi(4);
        if e.vignette_style == 2 {
            let target = if e.vignette < 0. { 0. } else { 1. };
            for v in p.iter_mut() {
                *v += (target - *v) * e.vignette.abs() * mask * protect;
            }
        } else {
            let gain = 2f32.powf(e.vignette * mask * protect * 2.);
            for v in p.iter_mut() {
                *v *= gain;
            }
        }
        let lens = ((nx * nx + ny * ny - e.lens_vignette_midpoint).max(0.)
            / (2. - e.lens_vignette_midpoint))
            .clamp(0., 1.);
        let gain = 2f32.powf(-e.lens_vignette * lens * 2.);
        let size = 0.75 + e.grain_size * 5.;
        let (gx, gy) = if scale == 1. {
            (x as f32, y as f32)
        } else {
            (
                (x as f32 + 0.5) / scale - 0.5,
                (y as f32 + 0.5) / scale - 0.5,
            )
        };
        let coarse = grain(gx, gy, size, e.grain_seed) * (size * scale).min(1.) / size.min(1.);
        let fine =
            hash(gx.round() as i32, gy.round() as i32, e.grain_seed ^ 0x21f09) * scale.min(1.);
        let noise = (coarse * (1. - e.grain_roughness) + fine * e.grain_roughness)
            * e.grain
            * 0.13
            * (4. * l * (1. - l)).max(0.2);
        for v in p {
            *v = (*v * gain + noise).clamp(0., 1.);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fringe_selector_sets_the_band_of_the_picked_color() {
        let mut e = Effects::default();
        assert_eq!(e.pick_fringe([0.6, 0.3, 0.8]), Some(0));
        assert_eq!(e.defringe[0], 0.25);
        let [lo, hi] = e.defringe_ranges[0];
        assert!((hi - lo - 0.2).abs() < 1e-6 && (0. ..=1.).contains(&lo) && hi <= 1.);
        // The picked hue is inside the new range and is removed.
        let lab = crate::develop::pipeline::srgb_to_lab(
            [0.6f32, 0.3, 0.8].map(crate::color_math::srgb_decode),
        );
        let hue = lab[2].atan2(lab[1]).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        e.defringe[0] = 1.;
        let out = e.defringe_color(lab, hue);
        assert!(out[1].hypot(out[2]) < lab[1].hypot(lab[2]) * 0.5);
        // An Amount already set is kept.
        e.defringe[1] = 0.6;
        assert_eq!(e.pick_fringe([0.3, 0.7, 0.3]), Some(1));
        assert_eq!(e.defringe[1], 0.6);
        // Neither purple nor green, or no colour at all.
        let before = e.clone();
        assert_eq!(e.pick_fringe([0.8, 0.2, 0.2]), None);
        assert_eq!(e.pick_fringe([0.5, 0.5, 0.5]), None);
        assert_eq!(e, before);
    }
}
