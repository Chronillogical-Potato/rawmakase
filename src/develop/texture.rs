//! Presence > Texture. [`TextureModel::Measured`] follows Camera Raw 18.7, fitted to
//! renders of synthetic charts: sine gratings of 0.004 to 0.25 cycles per pixel at
//! ±0.1 to ±2 EV, large flats and edges, at Texture −100 to +100.
//!
//! Camera Raw's Texture is a local contrast of log luminance over a broad band of
//! scales, a few to about thirty pixels, in pixels of the full-resolution photo
//! (an image twice the size renders the same per pixel). It leaves large flat areas
//! alone, boosts faint detail most (×1.78 at ±0.1 EV and +100) and strong contrast
//! hardly at all (×1.05 at ±2 EV), so edges get soft halos fading over about thirty
//! pixels. Here the detail is a Laplacian pyramid of the log of each colour channel
//! (Camera Raw's Texture also raises colour contrast at colour edges, up to +19
//! chroma beside a saturated red, as if each channel had its own): each level is
//! compressed where it is strong and weighted, and the sum scaled by a strength that
//! Texture sets. Fitted to the gratings within 0.04 RMS (×gain) and to the edges'
//! halos within 2.4% of the edge's step.
use crate::raw::CameraImage;
use anyhow::{Result, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

/// Which operator renders Texture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureModel {
    /// RAWmakase's first Texture, a 3-pixel box detail: what recipes saved before the
    /// measured one keep, so they render as they did.
    #[default]
    Original,
    /// Fitted to Camera Raw 18.7.
    Measured,
}
impl TextureModel {
    pub(crate) fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// The Texture this recipe renders with the measured operator, or 0 when it takes the
/// original one (older recipes, earlier engines).
pub(crate) fn measured(r: &super::Recipe) -> f32 {
    if r.engine >= 4 && r.texture_model == TextureModel::Measured {
        r.effects.texture
    } else {
        0.
    }
}

/// Pyramid levels, in full-resolution pixels: level `l` holds detail about `2^l`
/// pixels across.
const LEVELS: usize = 6;
/// Each level's weight and the log2 contrast above which it is compressed.
const WEIGHTS: [f32; LEVELS] = [1.402, 0.665, 1.571, 0.583, 0., 0.072];
const THRESHOLDS: [f32; LEVELS] = [0.506, 0.098, 0.167, 0.192, 0.135, 0.176];
/// Strength by Texture.
const AMOUNTS: [f32; 6] = [-1., -0.5, 0., 0.25, 0.5, 1.];
const STRENGTH: [f32; 6] = [-0.752, -0.499, 0., 0.334, 0.546, 0.852];

fn strength(amount: f32) -> f32 {
    let a = amount.clamp(-1., 1.);
    let i = AMOUNTS
        .partition_point(|v| *v <= a)
        .clamp(1, AMOUNTS.len() - 1);
    let t = (a - AMOUNTS[i - 1]) / (AMOUNTS[i] - AMOUNTS[i - 1]);
    STRENGTH[i - 1] + (STRENGTH[i] - STRENGTH[i - 1]) * t
}

/// `im`, an image `scale` times the full-resolution photo's size, with the measured
/// Texture `amount` applied to each channel; stops with an error when `cancel` is set.
pub(crate) fn apply(
    im: &CameraImage,
    amount: f32,
    scale: f32,
    cancel: &AtomicBool,
) -> Result<CameraImage> {
    // A reduced image's level 0 is a coarser full-resolution level.
    let offset = (-scale.max(1e-3).log2()).round().max(0.) as usize;
    let s = strength(amount);
    let mut out = im.clone();
    out.recovered = Default::default();
    for c in 0..3 {
        let logs = Plane {
            w: im.width as usize,
            h: im.height as usize,
            data: im
                .pixels
                .par_iter()
                .map(|p| p[c].max(1e-6).log2())
                .collect(),
        };
        let added = detail(&logs, offset, cancel)?;
        out.pixels
            .par_iter_mut()
            .zip(added.data.par_iter())
            .for_each(|(p, d)| p[c] *= (s * d).exp2());
    }
    Ok(out)
}
/// The compressed, weighted detail of `plane` as pyramid level `level` and coarser,
/// expanded to its size.
fn detail(plane: &Plane, level: usize, cancel: &AtomicBool) -> Result<Plane> {
    let zero = || Plane {
        w: plane.w,
        h: plane.h,
        data: vec![0.; plane.w * plane.h],
    };
    if level >= LEVELS || plane.w < 4 || plane.h < 4 {
        return Ok(zero());
    }
    ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
    let coarse = plane.down();
    let up = coarse.up(plane.w, plane.h);
    let below = detail(&coarse, level + 1, cancel)?.up(plane.w, plane.h);
    let (w, t) = (WEIGHTS[level], THRESHOLDS[level]);
    let data = plane
        .data
        .par_iter()
        .zip(up.data.par_iter().zip(below.data.par_iter()))
        .map(|(v, (u, b))| {
            let band = v - u;
            let x = band / t;
            b + w * band / (1. + x * x)
        })
        .collect();
    Ok(Plane {
        w: plane.w,
        h: plane.h,
        data,
    })
}

/// One channel at one pyramid level.
struct Plane {
    w: usize,
    h: usize,
    data: Vec<f32>,
}
/// The 5-tap binomial filter, and its expanding weights for even and odd samples.
const TAPS: [f32; 5] = [1. / 16., 4. / 16., 6. / 16., 4. / 16., 1. / 16.];
const EVEN: [f32; 3] = [TAPS[0] * 2., TAPS[2] * 2., TAPS[4] * 2.];
const ODD: f32 = TAPS[1] * 2.;
fn mirror(v: isize, n: usize) -> usize {
    let n = n as isize;
    let v = v.abs();
    (if v >= n { 2 * n - 2 - v } else { v }).clamp(0, n - 1) as usize
}
impl Plane {
    /// Blurred and halved.
    fn down(&self) -> Self {
        let (w, h) = (self.w.div_ceil(2), self.h.div_ceil(2));
        let mut wide = vec![0.; w * self.h];
        wide.par_chunks_mut(w)
            .zip(self.data.par_chunks(self.w))
            .for_each(|(out, row)| {
                for (x, o) in out.iter_mut().enumerate() {
                    *o = (0..5)
                        .map(|k| row[mirror(2 * x as isize + k - 2, self.w)] * TAPS[k as usize])
                        .sum();
                }
            });
        let mut data = vec![0.; w * h];
        data.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
            let rows: [&[f32]; 5] = std::array::from_fn(|k| {
                let r = mirror(2 * y as isize + k as isize - 2, self.h);
                &wide[r * w..(r + 1) * w]
            });
            for (x, o) in out.iter_mut().enumerate() {
                *o = (0..5).map(|k| rows[k][x] * TAPS[k]).sum();
            }
        });
        Plane { w, h, data }
    }
    /// Doubled to `w` × `h` and blurred, as a Laplacian pyramid expands a level.
    fn up(&self, w: usize, h: usize) -> Self {
        let expand = |i: usize, at: &dyn Fn(isize) -> f32| -> f32 {
            let c = (i / 2) as isize;
            if i.is_multiple_of(2) {
                at(c - 1) * EVEN[0] + at(c) * EVEN[1] + at(c + 1) * EVEN[2]
            } else {
                (at(c) + at(c + 1)) * ODD
            }
        };
        let mut wide = vec![0.; w * self.h];
        wide.par_chunks_mut(w)
            .zip(self.data.par_chunks(self.w))
            .for_each(|(out, row)| {
                for (x, o) in out.iter_mut().enumerate() {
                    *o = expand(x, &|c| row[mirror(c, self.w)]);
                }
            });
        let mut data = vec![0.; w * h];
        data.par_chunks_mut(w).enumerate().for_each(|(y, out)| {
            let row = |c: isize| {
                let r = mirror(c, self.h);
                &wide[r * w..(r + 1) * w]
            };
            for (x, o) in out.iter_mut().enumerate() {
                *o = expand(y, &|c| row(c)[x]);
            }
        });
        Plane { w, h, data }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gain Texture `amount` gives a vertical sine grating of `f` cycles per pixel
    /// and `amplitude` EV, on a full-resolution image.
    fn gain_at(f: f32, amplitude: f32, amount: f32) -> f32 {
        let (w, h) = (1024u32, 128u32);
        let im = CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels: (0..w * h)
                .map(|i| {
                    let x = (i % w) as f32;
                    [0.18 * (amplitude * (std::f32::consts::TAU * f * x).sin()).exp2(); 3]
                })
                .collect(),
            metadata: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        };
        let out = apply(&im, amount, 1., &AtomicBool::new(false)).unwrap();
        let amp = |im: &CameraImage| {
            let (mut s, mut c) = (0., 0.);
            for x in 128..896 {
                let v = im.pixels[(64 * w + x) as usize][1].log2();
                let t = std::f32::consts::TAU * f * x as f32;
                (s, c) = (s + v * t.sin(), c + v * t.cos());
            }
            (s * s + c * c).sqrt()
        };
        amp(&out) / amp(&im)
    }

    /// Camera Raw 18.7 at Texture +100 on ±0.5 EV gratings, and on fainter and
    /// stronger ones at 0.03 cycles per pixel.
    #[test]
    fn measured_texture_follows_camera_raw_over_scale_and_contrast() {
        for (f, amplitude, camera_raw) in [
            (0.008, 0.5, 1.14),
            (0.03, 0.5, 1.43),
            (0.12, 0.5, 1.52),
            (0.03, 0.1, 1.78),
            (0.03, 2., 1.05),
        ] {
            let ours = gain_at(f, amplitude, 1.);
            assert!(
                (ours - camera_raw).abs() < 0.15,
                "{f} {amplitude}: {ours} against {camera_raw}"
            );
        }
        // Negative Texture smooths the same band; none leaves the image alone.
        assert!(gain_at(0.03, 0.5, -1.) < 0.75);
        assert!((gain_at(0.03, 0.5, 0.) - 1.).abs() < 1e-4);
    }
}
