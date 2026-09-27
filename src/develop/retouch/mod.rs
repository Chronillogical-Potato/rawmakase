//! Lightroom's Remove panel in Heal and Clone modes: spots and brushed areas that copy
//! pixels from another part of the photo. Operations are stored as parameters and
//! rendered on the linear camera image before everything else, so they follow every
//! later edit and export at full quality.
//!
//! Positions are in image space (see [`crate::develop::ImageFrame`]): normalised to the
//! oriented photo before lens correction, Transform, crop and straightening. Sizes are
//! fractions of the photo's long edge.
mod heal;
mod layer;
mod search;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(crate) use layer::{RetouchCache, apply};
pub use search::find_source;

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum RetouchMode {
    /// Copy the source texture and match its tone and colour to the surroundings.
    #[default]
    Heal,
    /// Copy the source pixels as they are.
    Clone,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum RetouchShape {
    /// A circle, from a click.
    Spot { center: [f32; 2], radius: f32 },
    /// A brushed path of dabs with a common radius, from a drag.
    Brush {
        points: Arc<[[f32; 2]]>,
        radius: f32,
    },
}
/// One Heal or Clone operation. Operations apply in list order; later ones see the
/// result of earlier ones, as in Lightroom.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RetouchOp {
    pub mode: RetouchMode,
    pub shape: RetouchShape,
    /// Soft edge, 0–1 of the radius.
    pub feather: f32,
    /// 0–1.
    pub opacity: f32,
    /// Source position minus destination position, in image space.
    pub offset: [f32; 2],
}
/// Longest brush path kept, in dabs.
pub const MAX_POINTS: usize = 4096;
/// Most operations per photo.
pub const MAX_OPS: usize = 1000;
impl RetouchOp {
    pub fn validate(&self) -> Result<()> {
        let unit = |v: f32| v.is_finite() && (0. ..=1.).contains(&v);
        let position = |p: &[f32; 2]| p.iter().all(|v| v.is_finite() && v.abs() <= 2.);
        ensure!(
            unit(self.feather) && unit(self.opacity) && position(&self.offset),
            "Invalid spot removal settings"
        );
        let radius = |r: f32| r.is_finite() && (1e-4..=0.5).contains(&r);
        match &self.shape {
            RetouchShape::Spot { center, radius: r } => {
                ensure!(position(center) && radius(*r), "Invalid spot")
            }
            RetouchShape::Brush { points, radius: r } => ensure!(
                !points.is_empty()
                    && points.len() <= MAX_POINTS
                    && points.iter().all(position)
                    && radius(*r),
                "Invalid spot removal brush"
            ),
        }
        Ok(())
    }
    /// The destination's radius, as a fraction of the long edge.
    pub fn radius(&self) -> f32 {
        match &self.shape {
            RetouchShape::Spot { radius, .. } | RetouchShape::Brush { radius, .. } => *radius,
        }
    }
    pub fn set_radius(&mut self, r: f32) {
        match &mut self.shape {
            RetouchShape::Spot { radius, .. } | RetouchShape::Brush { radius, .. } => *radius = r,
        }
    }
    /// Where the pin sits: the spot's centre, or the brush path's last dab.
    pub fn pin(&self) -> [f32; 2] {
        match &self.shape {
            RetouchShape::Spot { center, .. } => *center,
            RetouchShape::Brush { points, .. } => points[points.len() - 1],
        }
    }
    /// Moves the destination by `delta` (image space), keeping the source in place.
    pub fn translate(&mut self, delta: [f32; 2]) {
        let shift = |p: [f32; 2]| [p[0] + delta[0], p[1] + delta[1]];
        match &mut self.shape {
            RetouchShape::Spot { center, .. } => *center = shift(*center),
            RetouchShape::Brush { points, .. } => {
                *points = points.iter().map(|p| shift(*p)).collect();
            }
        }
        self.offset = [self.offset[0] - delta[0], self.offset[1] - delta[1]];
    }
    /// Image-space bounds of the destination, `[x0, y0, x1, y1]`, for a photo whose
    /// width is `aspect` times its height.
    pub fn bounds(&self, aspect: f32) -> [f32; 4] {
        let (rx, ry) = radii(self.radius(), aspect);
        let mut b = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        let mut add = |p: [f32; 2]| {
            b = [
                b[0].min(p[0] - rx),
                b[1].min(p[1] - ry),
                b[2].max(p[0] + rx),
                b[3].max(p[1] + ry),
            ];
        };
        match &self.shape {
            RetouchShape::Spot { center, .. } => add(*center),
            RetouchShape::Brush { points, .. } => points.iter().for_each(|p| add(*p)),
        }
        b
    }
}
/// A long-edge fraction as normalised x and y radii.
pub(crate) fn radii(r: f32, aspect: f32) -> (f32, f32) {
    if aspect >= 1. {
        (r, r * aspect)
    } else {
        (r / aspect, r)
    }
}
pub fn validate(ops: &[RetouchOp]) -> Result<()> {
    ensure!(ops.len() <= MAX_OPS, "Too many spot removals");
    ops.iter().try_for_each(RetouchOp::validate)
}

#[cfg(test)]
mod tests;

/// Lightroom's Visualize Spots: a black-and-white view of fine luminance detail in a
/// rendered preview, where dust and small blemishes stand out. `threshold` (0–1) is
/// the panel's slider: higher shows fainter detail. Returns 8-bit RGB.
pub fn visualize_spots(image: &crate::develop::Rendered, threshold: f32) -> Vec<u8> {
    let (w, h) = (image.width as usize, image.height as usize);
    let lum: Vec<f32> = image
        .pixels
        .iter()
        .map(|p| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2])
        .collect();
    // Detail = luminance minus its 5×5 mean, from running sums.
    let r = 2usize;
    let mut rows = vec![0.; w * h];
    for y in 0..h {
        for x in 0..w {
            let (a, b) = (x.saturating_sub(r), (x + r + 1).min(w));
            rows[y * w + x] = lum[y * w + a..y * w + b].iter().sum::<f32>() / (b - a) as f32;
        }
    }
    let gain = 4. * 2f32.powf(threshold.clamp(0., 1.) * 5.);
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        let (a, b) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let mean = (a..b).map(|yy| rows[yy * w + x]).sum::<f32>() / (b - a) as f32;
            let v = ((lum[y * w + x] - mean).abs() * gain).min(1.);
            let v = (255. * v.sqrt()) as u8;
            out.extend([v, v, v]);
        }
    }
    out
}
