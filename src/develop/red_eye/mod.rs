//! Lightroom's Red Eye Correction: an ellipse over a pupil whose colour is replaced by
//! a dark neutral, with Pupil Size and Darken sliders. Corrections are stored as
//! parameters beside the recipe, like spot removal, and render on the linear camera
//! image before Heal and Clone, so every later edit and export sees them.
//!
//! Positions are in image space (see [`crate::develop::ImageFrame`]); sizes are
//! fractions of the photo's long edge. The ellipse is Lightroom's: semi-axes along the
//! image's x and y and their correlation (`crs:RedEyeInfo`'s `width`, `height` and
//! `alpha`), not a rotation angle.
mod detect;
mod render;

use anyhow::{Result, ensure};
use serde::{Deserialize, Deserializer, Serialize};

pub use detect::{DetectError, Pupil, find_pupil};
pub(crate) use render::Placed;

/// Lightroom's Type menu. Pet Eye is not supported yet.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum EyeKind {
    #[default]
    Red,
}

/// One corrected eye. Corrections apply in list order.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct RedEyeOp {
    #[serde(default)]
    pub kind: EyeKind,
    /// The pupil's centre, in image space.
    pub center: [f32; 2],
    /// Semi-axes along image x and y, as fractions of the long edge.
    pub radius: [f32; 2],
    /// Correlation of x and y over the ellipse (−1 to 1, exclusive): tilts it.
    #[serde(default)]
    pub correlation: f32,
    /// Lightroom's Pupil Size, 0–1: how far beyond the pupil the correction reaches.
    pub pupil_size: f32,
    /// Lightroom's Darken, 0–1: 0 lightens the pupil, 1 makes it nearly black.
    pub darken: f32,
}

/// Lightroom's slider defaults.
pub const DEFAULT_PUPIL_SIZE: f32 = 0.5;
pub const DEFAULT_DARKEN: f32 = 0.5;
/// Most corrections per photo.
pub const MAX_OPS: usize = 200;
/// The largest correlation kept; Camera Raw refuses ±1.
pub const MAX_CORRELATION: f32 = 0.95;

impl RedEyeOp {
    pub fn validate(&self) -> Result<()> {
        let unit = |v: f32| v.is_finite() && (0. ..=1.).contains(&v);
        let position = |v: f32| v.is_finite() && v.abs() <= 2.;
        let radius = |r: f32| r.is_finite() && (1e-4..=0.25).contains(&r);
        ensure!(
            self.center.iter().all(|v| position(*v))
                && self.radius.iter().all(|r| radius(*r))
                && self.correlation.is_finite()
                && self.correlation.abs() <= MAX_CORRELATION
                && unit(self.pupil_size)
                && unit(self.darken),
            "Invalid red eye correction"
        );
        Ok(())
    }
    /// Points on the ellipse's outline in image space, for a photo whose width is
    /// `aspect` times its height.
    pub fn outline(&self, aspect: f32, points: usize) -> Vec<[f32; 2]> {
        let (sx, sy) = crate::develop::retouch::radii(1., aspect);
        let [a, b] = self.radius;
        let c = self.correlation;
        (0..points)
            .map(|i| {
                let t = i as f32 / points as f32 * std::f32::consts::TAU;
                // The Cholesky factor of [[a², cab], [cab, b²]] maps the unit circle
                // onto the ellipse.
                let (x, y) = (
                    a * t.cos(),
                    b * (c * t.cos() + (1. - c * c).sqrt() * t.sin()),
                );
                [self.center[0] + x * sx, self.center[1] + y * sy]
            })
            .collect()
    }
    /// Whether image-space position `p` lies inside the ellipse.
    pub fn contains(&self, p: [f32; 2], aspect: f32) -> bool {
        let (sx, sy) = crate::develop::retouch::radii(1., aspect);
        let d = [(p[0] - self.center[0]) / sx, (p[1] - self.center[1]) / sy];
        render::mahalanobis2(self.radius, self.correlation, d) <= 1.
    }
    /// Moves the ellipse by `delta` (image space).
    pub fn translate(&mut self, delta: [f32; 2]) {
        self.center = [self.center[0] + delta[0], self.center[1] + delta[1]];
    }
}

pub fn validate(ops: &[RedEyeOp]) -> Result<()> {
    ensure!(ops.len() <= MAX_OPS, "Too many red eye corrections");
    ops.iter().try_for_each(RedEyeOp::validate)
}

/// Reads saved corrections, skipping any this release cannot read (a type added by a
/// later release), so they never stop the photo's other local edits from loading.
pub(crate) fn lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<RedEyeOp>, D::Error> {
    let values = Vec::<serde_json::Value>::deserialize(d)?;
    Ok(values
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect())
}

#[cfg(test)]
mod tests;
