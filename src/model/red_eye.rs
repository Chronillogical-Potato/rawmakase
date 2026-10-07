//! Red Eye and Pet Eye corrections as a recipe stores them (Lightroom's Red Eye
//! Correction): an ellipse over a pupil with Pupil Size and Darken, or for Pet Eye
//! an optional catchlight. Positions are in image space; sizes are fractions of the
//! photo's long edge. The ellipse is Lightroom's: semi-axes along the image's x and
//! y and their correlation (`crs:RedEyeInfo`'s `width`, `height` and `alpha`), not a
//! rotation angle. Rendering and pupil detection are `develop::red_eye`'s.
use anyhow::{Result, ensure};
use serde::{Deserialize, Deserializer, Serialize};

/// Lightroom's Type menu.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum EyeKind {
    /// A red pupil, made a dark neutral.
    #[default]
    Red,
    /// An animal's glowing pupil, made black, with Lightroom's Add Catchlight: the
    /// catchlight's offset from the centre in units of the semi-axes along image x and
    /// y (within the unit circle), or `None`.
    Pet { catchlight: Option<[f32; 2]> },
}
/// Whether catchlight offset `c` (in units of the semi-axes) lies within an ellipse
/// with `correlation`.
pub fn catchlight_inside(c: [f32; 2], correlation: f32) -> bool {
    mahalanobis2([1., 1.], correlation, c) <= 1. + 1e-4
}
/// `c` (in units of the semi-axes), pulled in to an ellipse with `correlation`.
fn within(c: [f32; 2], correlation: f32) -> [f32; 2] {
    let length = mahalanobis2([1., 1.], correlation, c).sqrt();
    if length > 1. {
        c.map(|v| v / length)
    } else {
        c
    }
}
/// Lightroom's default catchlight (`highlightX = 0.591, highlightY = 0.424`), as an
/// offset in units of the semi-axes.
pub const DEFAULT_CATCHLIGHT: [f32; 2] = [0.182, -0.152];
impl EyeKind {
    /// Lightroom's name for it, as in "Add Red Eye Correction".
    pub fn name(self) -> &'static str {
        match self {
            EyeKind::Red => "Red Eye",
            EyeKind::Pet { .. } => "Pet Eye",
        }
    }
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
                && unit(self.darken)
                && match self.kind {
                    EyeKind::Pet {
                        catchlight: Some(c),
                    } => {
                        c.iter().all(|v| v.is_finite()) && catchlight_inside(c, self.correlation)
                    }
                    _ => true,
                },
            "Invalid red eye correction"
        );
        Ok(())
    }
    /// Points on the ellipse's outline in image space, for a photo whose width is
    /// `aspect` times its height.
    pub fn outline(&self, aspect: f32, points: usize) -> Vec<[f32; 2]> {
        let (sx, sy) = super::retouch::radii(1., aspect);
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
        let (sx, sy) = super::retouch::radii(1., aspect);
        let d = [(p[0] - self.center[0]) / sx, (p[1] - self.center[1]) / sy];
        mahalanobis2(self.radius, self.correlation, d) <= 1.
    }
    /// The catchlight's image-space position, for a photo whose width is `aspect` times
    /// its height.
    pub fn catchlight_at(&self, aspect: f32) -> Option<[f32; 2]> {
        let EyeKind::Pet {
            catchlight: Some(c),
        } = self.kind
        else {
            return None;
        };
        let (sx, sy) = super::retouch::radii(1., aspect);
        let half = half(self);
        Some([
            self.center[0] + c[0] * self.radius[0] * half * sx,
            self.center[1] + c[1] * self.radius[1] * half * sy,
        ])
    }
    /// Places the catchlight at image-space position `p`, kept within the pupil.
    pub fn set_catchlight(&mut self, p: [f32; 2], aspect: f32) {
        let (sx, sy) = super::retouch::radii(1., aspect);
        let half = half(self);
        let mut c = [
            (p[0] - self.center[0]) / (self.radius[0] * half * sx),
            (p[1] - self.center[1]) / (self.radius[1] * half * sy),
        ];
        c = within(c, self.correlation);
        if let EyeKind::Pet { catchlight } = &mut self.kind {
            *catchlight = Some(c);
        }
    }
    /// Pulls the catchlight in to the pupil's edge if it lies beyond it, as Lightroom's
    /// default can on a strongly tilted pupil.
    pub fn fit_catchlight(&mut self) {
        let correlation = self.correlation;
        if let EyeKind::Pet {
            catchlight: Some(c),
        } = &mut self.kind
        {
            *c = within(*c, correlation);
        }
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

/// Where the correction is half applied, in units of the ellipse, at Pupil Size 0 and
/// its growth to Pupil Size 1 (measured: 0.59 and 1.56).
const HALF_AT: f32 = 0.585;
const HALF_GROWTH: f32 = 0.975;
/// Pet Eye's half-way distance relative to Red Eye's (measured: 102 against 107 pixels).
const PET_HALF: f32 = 0.953;
/// The half-way distance of `op`'s falloff, in units of its ellipse.
pub fn half(op: &RedEyeOp) -> f32 {
    let half = HALF_AT + HALF_GROWTH * op.pupil_size;
    match op.kind {
        EyeKind::Red => half,
        EyeKind::Pet { .. } => half * PET_HALF,
    }
}
/// `d`'s squared distance from the centre of the ellipse with semi-axes `radius` and
/// `correlation`, in units of the ellipse.
pub fn mahalanobis2(radius: [f32; 2], correlation: f32, d: [f32; 2]) -> f32 {
    let (x, y) = (d[0] / radius[0], d[1] / radius[1]);
    (x * x - 2. * correlation * x * y + y * y) / (1. - correlation * correlation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corrections_keep_their_stored_form() {
        let pet = RedEyeOp {
            kind: EyeKind::Pet {
                catchlight: Some(DEFAULT_CATCHLIGHT),
            },
            center: [0.5, 0.25],
            radius: [0.02, 0.03],
            correlation: 0.,
            pupil_size: DEFAULT_PUPIL_SIZE,
            darken: DEFAULT_DARKEN,
        };
        let json = serde_json::to_value(&pet).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "kind": {"Pet": {"catchlight": [0.182_f32, -0.152_f32]}},
                "center": [0.5, 0.25],
                "radius": [0.02_f32, 0.03_f32],
                "correlation": 0.0,
                "pupil_size": 0.5,
                "darken": 0.5,
            })
        );
        // Kind and correlation were added later; older corrections are Red Eye, untilted.
        let old: RedEyeOp = serde_json::from_str(
            r#"{"center":[0.5,0.5],"radius":[0.02,0.02],"pupil_size":0.5,"darken":0.5}"#,
        )
        .unwrap();
        assert_eq!((old.kind, old.correlation), (EyeKind::Red, 0.));
        assert!(validate(&[pet, old]).is_ok());
    }

    #[test]
    fn validation_keeps_its_limits() {
        let mut op = RedEyeOp {
            kind: EyeKind::Red,
            center: [0.5, 0.5],
            radius: [MAX_RADIUS, 0.01],
            correlation: MAX_CORRELATION,
            pupil_size: 1.,
            darken: 0.,
        };
        assert!(op.validate().is_ok());
        op.correlation = 0.96;
        assert!(op.validate().is_err());
        op.correlation = 0.;
        op.radius[0] = MAX_RADIUS * 1.01;
        assert!(op.validate().is_err());
        op.radius[0] = 0.1;
        assert!(validate(&vec![op.clone(); MAX_OPS]).is_ok());
        assert!(validate(&vec![op; MAX_OPS + 1]).is_err());
    }
}
