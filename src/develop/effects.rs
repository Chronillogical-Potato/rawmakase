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
    pub fn defringe_color(&self, mut lab: [f32; 3], h: f32) -> [f32; 3] {
        // Hue bands are expressed relative to the purple/green windows of the control.
        for (i, center) in [0.85f32, 0.4].into_iter().enumerate() {
            let range = self.defringe_ranges[i];
            let lo = center + (range[0] - 0.5) * 0.3;
            let hi = center + (range[1] - 0.5) * 0.3;
            let mid = (lo + hi) * 0.5;
            let width = ((hi - lo) * 0.5).max(0.005);
            let d = (h - mid + 0.5).rem_euclid(1.) - 0.5;
            let w = (1. - (d.abs() / width)).clamp(0., 1.);
            let k = 1. - self.defringe[i] * w;
            lab[1] *= k;
            lab[2] *= k;
        }
        lab
    }
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
        let coarse = grain(x as f32, y as f32, 0.75 + e.grain_size * 5., e.grain_seed);
        let fine = hash(x as i32, y as i32, e.grain_seed ^ 0x21f09);
        let noise = (coarse * (1. - e.grain_roughness) + fine * e.grain_roughness)
            * e.grain
            * 0.13
            * (4. * l * (1. - l)).max(0.2);
        for v in p {
            *v = (*v * gain + noise).clamp(0., 1.);
        }
    });
}
