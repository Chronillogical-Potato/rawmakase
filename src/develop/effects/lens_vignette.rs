//! Lightroom's manual lens Vignetting (Lens Corrections > Manual), fitted to Camera Raw
//! 18.7 renders of flat synthetic DNGs.
//!
//! Camera Raw multiplies scene-linear light, before the tone curve, by a radial gain
//! over the whole photo: the crop does not move or resize it, and the gain is the same
//! at every brightness. The radius is the distance from the photo's centre over its
//! half diagonal, so a square and a 3:2 photo share one profile. Positive amounts
//! lighten the corners and negative ones darken them, by
//!
//! `ln gain = amount × c·r^p / (1 + k·r^p)`
//!
//! where Midpoint mainly raises the power `p`, keeping more of the middle untouched;
//! `c`, `p` and `k` move linearly with it. The fit's log error is 0.018 RMS over 32
//! renders (Amount ±25 to ±100, Midpoint 0 to 100, three brightnesses).
use crate::lens::Radial;
use serde::{Deserialize, Serialize};

/// Which operator renders a recipe's manual lens Vignetting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LensVignetteModel {
    /// RAWmakase's first version, applied to the finished pixels after the crop: what
    /// recipes saved before the measured model keep, so they render as they did.
    #[default]
    Original,
    /// The gain measured in Camera Raw, applied to the camera image.
    Measured,
}
impl LensVignetteModel {
    pub(crate) fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// The measured gain for one Amount and Midpoint.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ManualVignette {
    amount: f32,
    scale: f32,
    power: f32,
    knee: f32,
}
impl ManualVignette {
    /// Lightroom's Amount (−1 to 1) and Midpoint (0 to 1); `None` at Amount 0.
    pub(crate) fn new(amount: f32, midpoint: f32) -> Option<Self> {
        if amount == 0. {
            return None;
        }
        let m = midpoint.clamp(0., 1.) * 100.;
        Some(Self {
            amount: amount.clamp(-1., 1.),
            scale: 2.206 + 0.001321 * m,
            power: 2.110 + 0.07700 * m,
            knee: 0.6184 + 0.001916 * m,
        })
    }
    /// The gain at radius `r` (1 is the half diagonal).
    pub(crate) fn gain(&self, r: f32) -> f32 {
        let t = r.max(0.).powf(self.power);
        (self.amount * self.scale * t / (1. + self.knee * t)).exp()
    }
}

/// Knots of a table holding the manual gain, with the lens profile's: denser towards
/// the corners, where high Midpoints bend the gain sharply, and reaching a little past
/// them for samples that distortion correction moves outwards.
const TABLE_KNOTS: usize = 64;
const TABLE_REACH: f32 = 1.1;

/// `lens` raised to `amount`, times `manual`, as one table for sampling (and the GPU).
/// The table's radius is the decoded image's; `frame_scale` is the decoded image's half
/// diagonal over the photo frame's (the camera's default crop), which the manual gain
/// is measured on. The lens table keeps its own centre, the decoded image's, so a
/// default crop off the sensor's centre is approximated there.
pub(crate) fn combined_table(
    lens: Option<&Radial>,
    amount: f32,
    manual: &ManualVignette,
    frame_scale: f32,
) -> Radial {
    let knots: Vec<f32> = (0..TABLE_KNOTS)
        .map(|i| TABLE_REACH * (i as f32 / (TABLE_KNOTS - 1) as f32).sqrt())
        .collect();
    let values = knots
        .iter()
        .map(|r| lens.map_or(1., |l| l.eval(*r).powf(amount)) * manual.gain(*r * frame_scale))
        .collect();
    Radial { knots, values }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Camera Raw 18.7's scene-linear gain at the corner (radius 0.975) and halfway
    /// (0.525), from flat renders of a 3:2 DNG.
    #[test]
    fn gain_follows_camera_raw_in_both_directions() {
        for (amount, midpoint, half, corner) in [
            (-0.5, 0.5, 0.982, 0.554),
            (0.5, 0.5, 1.026, 1.837),
            (-1., 0.5, 0.947, 0.291),
            (-0.5, 0.2, 0.919, 0.532),
            (-0.5, 1., 0.999, 0.589),
            (0.5, 0., 1.320, 1.934),
        ] {
            let v = ManualVignette::new(amount, midpoint).unwrap();
            assert!(
                (v.gain(0.525) / half - 1.).abs() < 0.04,
                "{amount} {midpoint} half"
            );
            assert!(
                (v.gain(0.975) / corner - 1.).abs() < 0.05,
                "{amount} {midpoint} corner"
            );
            assert_eq!(v.gain(0.), 1.);
        }
        assert!(ManualVignette::new(0., 0.5).is_none());
    }

    #[test]
    fn combined_table_matches_the_gain() {
        let v = ManualVignette::new(-0.7, 0.9).unwrap();
        let lens = Radial {
            knots: vec![0., 1.],
            values: vec![1., 1.5],
        };
        let table = combined_table(Some(&lens), 0.5, &v, 1.);
        for r in [0., 0.3, 0.7, 0.9, 0.97, 1.] {
            let exact = lens.eval(r).powf(0.5) * v.gain(r);
            assert!((table.eval(r) / exact - 1.).abs() < 0.003, "{r}");
        }
        // A default crop: the frame's corner gets the full corner gain.
        let framed = combined_table(None, 1., &v, 1.25);
        assert!((framed.eval(0.8) / v.gain(1.) - 1.).abs() < 0.003);
    }
}
