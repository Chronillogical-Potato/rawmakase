//! Heal and Clone operations as a recipe stores them (Lightroom's Remove panel):
//! where each copies from and to, its shape, feather and opacity. Positions are
//! in image space, sizes fractions of the photo's long edge; rendering them is
//! `develop::retouch`'s.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
}
pub fn validate(ops: &[RetouchOp]) -> Result<()> {
    ensure!(ops.len() <= MAX_OPS, "Too many spot removals");
    ops.iter().try_for_each(RetouchOp::validate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operations_keep_their_stored_form() {
        let spot = RetouchOp {
            mode: RetouchMode::Clone,
            shape: RetouchShape::Spot {
                center: [0.25, 0.5],
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
        };
        let json = serde_json::to_string(&spot).unwrap();
        assert_eq!(
            json,
            r#"{"mode":"Clone","shape":{"Spot":{"center":[0.25,0.5],"radius":0.02}},"feather":0.5,"opacity":1.0,"offset":[0.1,0.0]}"#
        );
        assert_eq!(serde_json::from_str::<RetouchOp>(&json).unwrap(), spot);
        let brush = r#"{"mode":"Heal","shape":{"Brush":{"points":[[0.1,0.1],[0.2,0.1]],"radius":0.01}},"feather":0.0,"opacity":0.5,"offset":[0.0,0.2]}"#;
        let op: RetouchOp = serde_json::from_str(brush).unwrap();
        assert_eq!(op.pin(), [0.2, 0.1]);
        assert!(validate(&[spot, op]).is_ok());
        let unknown = format!("{},\"extra\":1}}", &brush[..brush.len() - 1]);
        assert!(serde_json::from_str::<serde_json::Value>(&unknown).is_ok());
        assert!(serde_json::from_str::<RetouchOp>(&unknown).is_err());
    }

    #[test]
    fn validation_keeps_its_limits() {
        let at = |radius| RetouchOp {
            mode: RetouchMode::Heal,
            shape: RetouchShape::Spot {
                center: [0.5, 0.5],
                radius,
            },
            feather: 0.,
            opacity: 1.,
            offset: [0.; 2],
        };
        assert!(at(0.5).validate().is_ok());
        assert!(at(0.6).validate().is_err());
        assert!(validate(&vec![at(0.1); MAX_OPS]).is_ok());
        assert!(validate(&vec![at(0.1); MAX_OPS + 1]).is_err());
        let mut long = at(0.1);
        long.shape = RetouchShape::Brush {
            points: vec![[0.5; 2]; MAX_POINTS + 1].into(),
            radius: 0.1,
        };
        assert!(long.validate().is_err());
    }
}
