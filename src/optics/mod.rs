//! The lens model the renderer evaluates: radial functions of the normalized image
//! radius for vignetting, distortion and lateral chromatic aberration. Readers in
//! [`lens`](crate::lens) and [`dng`](crate::dng) produce it. [`lcp`] holds Adobe
//! lens profiles as data. Only `lcp` depends on anything in the crate: `xml`, to
//! parse the profiles.
pub mod lcp;

use serde::{Deserialize, Serialize};

/// A radial function sampled at increasing radii. Radius 0 is the image centre and
/// 1 is half of the image diagonal. Values are linearly interpolated and held constant
/// beyond the outermost knots.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Radial {
    pub knots: Vec<f32>,
    pub values: Vec<f32>,
}
impl Radial {
    pub(crate) fn new(knots: Vec<f32>, values: Vec<f32>) -> Option<Self> {
        let r = Self { knots, values };
        r.valid().then_some(r)
    }
    fn valid(&self) -> bool {
        self.knots.len() >= 2
            && self.knots.len() == self.values.len()
            && self.knots.len() <= 64
            && self.knots.iter().chain(&self.values).all(|v| v.is_finite())
            && self.knots.windows(2).all(|k| k[0] < k[1])
            && self.knots[0] >= 0.
            && self.values.iter().all(|v| (0.2..=5.).contains(v))
    }
    pub fn eval(&self, r: f32) -> f32 {
        let k = &self.knots;
        let i = k.partition_point(|x| *x <= r);
        if i == 0 {
            return self.values[0];
        }
        if i == k.len() {
            return self.values[k.len() - 1];
        }
        let t = (r - k[i - 1]) / (k[i] - k[i - 1]);
        self.values[i - 1] + (self.values[i] - self.values[i - 1]) * t
    }
}

/// Built-in lens correction for one photo.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LensCorrection {
    /// Where the data came from, for display, e.g. "Fujifilm built-in".
    pub source: String,
    /// Lightroom applies this correction by default. True where verified against
    /// Adobe's own rendering; other corrections are available but start disabled.
    #[serde(default)]
    pub default_on: bool,
    /// Gain that restores illumination at each radius (1 = none).
    pub vignetting: Option<Radial>,
    /// Source radius divided by output radius for the green channel (1 = none).
    pub distortion: Option<Radial>,
    /// Additional source radius scale for red and blue relative to green (1 = none).
    pub chromatic: Option<[Radial; 2]>,
}
/// No correction, for applying measured chromatic aberration alone.
pub(crate) static NO_CORRECTION: LensCorrection = LensCorrection {
    source: String::new(),
    default_on: false,
    vignetting: None,
    distortion: None,
    chromatic: None,
};
impl LensCorrection {
    pub fn is_empty(&self) -> bool {
        self.vignetting.is_none() && self.distortion.is_none() && self.chromatic.is_none()
    }
    pub fn validate(&self) -> bool {
        self.vignetting.iter().all(Radial::valid)
            && self.distortion.iter().all(Radial::valid)
            && self.chromatic.iter().flatten().all(Radial::valid)
    }
    pub fn vignetting_gain(&self, r: f32) -> f32 {
        self.vignetting.as_ref().map_or(1., |v| v.eval(r))
    }
    /// Source radius scale for red, green and blue at output radius `r`.
    pub fn radial_scale(&self, r: f32) -> [f32; 3] {
        self.radial_scale_with(r, 1.)
    }
    /// As `radial_scale`, with Lightroom's Distortion amount (1 = 100%).
    pub fn radial_scale_with(&self, r: f32, amount: f32) -> [f32; 3] {
        self.radial_scale_ca(r, amount, None)
    }
    /// As `radial_scale_with`, with `chromatic` measured from the decoded image in place
    /// of the lens data's own. A measurement is a function of the decoded radius, so it
    /// is evaluated where green samples, after distortion.
    pub fn radial_scale_ca(
        &self,
        r: f32,
        amount: f32,
        chromatic: Option<&[Radial; 2]>,
    ) -> [f32; 3] {
        let g = self
            .distortion
            .as_ref()
            .map_or(1., |d| 1. + (d.eval(r) - 1.) * amount);
        match (chromatic, &self.chromatic) {
            (Some([red, blue]), _) => [g * red.eval(r * g), g, g * blue.eval(r * g)],
            (None, Some([red, blue])) => [g * red.eval(r), g, g * blue.eval(r)],
            (None, None) => [g; 3],
        }
    }
    /// Output radii are scaled by this factor so every corrected corner samples inside
    /// the sensor, as Lightroom crops the undefined border after distortion correction.
    pub fn fill_scale(&self) -> f32 {
        if self.distortion.is_none() && self.chromatic.is_none() {
            return 1.;
        }
        self.fill_scale_with(1.)
    }
    pub fn fill_scale_with(&self, amount: f32) -> f32 {
        if self.distortion.is_none() && self.chromatic.is_none() {
            return 1.;
        }
        let widest = (0..=64)
            .flat_map(|i| self.radial_scale_with(i as f32 / 64., amount))
            .fold(1f32, f32::max);
        1. / widest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn radial_interpolates_and_clamps() {
        let r = Radial::new(vec![0., 0.5, 1.], vec![1., 1.2, 1.6]).unwrap();
        assert_eq!(r.eval(-1.), 1.);
        assert!((r.eval(0.25) - 1.1).abs() < 1e-6);
        assert!((r.eval(0.75) - 1.4).abs() < 1e-6);
        assert_eq!(r.eval(2.), 1.6);
        assert!(Radial::new(vec![0., 0.], vec![1., 1.]).is_none());
        assert!(Radial::new(vec![0., 1.], vec![1., f32::NAN]).is_none());
    }
    #[test]
    fn fill_scale_keeps_corners_inside() {
        let c = LensCorrection {
            distortion: Radial::new(vec![0., 1.], vec![1., 1.02]),
            ..Default::default()
        };
        let s = c.fill_scale();
        assert!(s < 1. && s > 0.97);
        assert!(c.radial_scale(s)[1] * s <= 1.0001);
        assert_eq!(LensCorrection::default().fill_scale(), 1.);
    }
}
