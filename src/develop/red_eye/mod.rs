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
/// The largest semi-axis, as a fraction of the long edge.
pub const MAX_RADIUS: f32 = 0.25;
/// The largest correlation kept; Camera Raw refuses ±1.
pub const MAX_CORRELATION: f32 = 0.95;

impl RedEyeOp {
    pub fn validate(&self) -> Result<()> {
        let unit = |v: f32| v.is_finite() && (0. ..=1.).contains(&v);
        let position = |v: f32| v.is_finite() && v.abs() <= 2.;
        let radius = |r: f32| r.is_finite() && (1e-4..=MAX_RADIUS).contains(&r);
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
    /// Moves the ellipse by `delta` (image space), keeping its centre on the photo.
    pub fn translate(&mut self, delta: [f32; 2]) {
        self.center = [
            (self.center[0] + delta[0]).clamp(0., 1.),
            (self.center[1] + delta[1]).clamp(0., 1.),
        ];
    }
}

pub fn validate(ops: &[RedEyeOp]) -> Result<()> {
    ensure!(ops.len() <= MAX_OPS, "Too many red eye corrections");
    ops.iter().try_for_each(RedEyeOp::validate)
}

/// A photo's red eye corrections, as a list of [`RedEyeOp`]s.
///
/// Corrections this release cannot read (a type a later release adds) are kept aside
/// as they were saved, with their place in the list, and written back there, so they
/// never stop the photo's other local edits from loading and are not lost or reordered
/// when it is edited here.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RedEyeList {
    ops: Vec<RedEyeOp>,
    /// Unreadable corrections and their positions in the saved list, in order.
    later: Vec<(usize, serde_json::Value)>,
}
impl RedEyeList {
    /// No corrections at all, readable or not.
    pub fn is_blank(&self) -> bool {
        self.ops.is_empty() && self.later.is_empty()
    }
}
impl From<Vec<RedEyeOp>> for RedEyeList {
    fn from(ops: Vec<RedEyeOp>) -> Self {
        Self {
            ops,
            later: Vec::new(),
        }
    }
}
impl std::ops::Deref for RedEyeList {
    type Target = Vec<RedEyeOp>;
    fn deref(&self) -> &Vec<RedEyeOp> {
        &self.ops
    }
}
impl std::ops::DerefMut for RedEyeList {
    fn deref_mut(&mut self) -> &mut Vec<RedEyeOp> {
        &mut self.ops
    }
}
impl Serialize for RedEyeList {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(Some(self.ops.len() + self.later.len()))?;
        let mut ops = self.ops.iter();
        let mut later = self.later.iter().peekable();
        for at in 0.. {
            if let Some((_, value)) = later.next_if(|(i, _)| *i <= at) {
                seq.serialize_element(value)?;
            } else if let Some(op) = ops.next() {
                seq.serialize_element(op)?;
            } else if let Some((_, value)) = later.next() {
                seq.serialize_element(value)?;
            } else {
                break;
            }
        }
        seq.end()
    }
}
impl<'de> Deserialize<'de> for RedEyeList {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let mut list = Self::default();
        for (i, value) in Vec::<serde_json::Value>::deserialize(d)?
            .into_iter()
            .enumerate()
        {
            match serde_json::from_value(value.clone()) {
                Ok(op) => list.ops.push(op),
                Err(_) => list.later.push((i, value)),
            }
        }
        Ok(list)
    }
}

#[cfg(test)]
mod tests;
