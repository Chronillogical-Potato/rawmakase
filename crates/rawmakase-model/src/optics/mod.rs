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
pub static NO_CORRECTION: LensCorrection = LensCorrection {
    source: String::new(),
    default_on: false,
    vignetting: None,
    distortion: None,
    chromatic: None,
};
impl LensCorrection {
    /// The `source` of the correction read from a Sony file's own tables (a DNG's
    /// opcodes are another source, with Lightroom's framing).
    pub const SONY_BUILTIN: &'static str = "Sony built-in";
    pub fn is_empty(&self) -> bool {
        self.vignetting.is_none() && self.distortion.is_none() && self.chromatic.is_none()
    }
    pub fn validate(&self) -> bool {
        self.vignetting.iter().all(Radial::valid)
            && self.distortion.iter().all(Radial::valid)
            && self.chromatic.iter().flatten().all(Radial::valid)
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn vignetting_gain(&self, r: f32) -> f32 {
        self.vignetting.as_ref().map_or(1., |v| v.eval(r))
    }
    /// As `radial_scale_with` at 100%.
    #[cfg(test)]
    pub fn radial_scale(&self, r: f32) -> [f32; 3] {
        self.radial_scale_with(r, 1.)
    }
    /// Source radius scale for red, green and blue at output radius `r`, with
    /// Lightroom's Distortion amount (1 = 100%).
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
    /// As `fill_scale_with` at 100%.
    #[cfg(test)]
    pub fn fill_scale(&self) -> f32 {
        if self.distortion.is_none() && self.chromatic.is_none() {
            return 1.;
        }
        self.fill_scale_with(1.)
    }
    /// Output radii are scaled by this factor so every corrected corner samples inside
    /// the sensor, as Lightroom crops the undefined border after distortion correction,
    /// with Lightroom's Distortion amount (1 = 100%).
    pub fn fill_scale_with(&self, amount: f32) -> f32 {
        if self.distortion.is_none() && self.chromatic.is_none() {
            return 1.;
        }
        let widest = (0..=64)
            .flat_map(|i| self.radial_scale_with(i as f32 / 64., amount))
            .fold(1f32, f32::max);
        1. / widest
    }
    /// As `fill_scale_with`, for a photo that is only the rectangle `crop` (`[left, top,
    /// right, bottom]`, in pixels) of the decoded image of `size`: the scale keeps every
    /// corrected sample of the crop's edge inside the decoded image, which can reach
    /// beyond the crop (a camera's aspect-ratio crop of a wider sensor). Never below
    /// the scale for the whole image.
    pub fn fill_scale_within(&self, amount: f32, size: [f32; 2], crop: [f32; 4]) -> f32 {
        let whole = self.fill_scale_with(amount);
        if whole >= 1. {
            return whole;
        }
        let [w, h] = size;
        let half = w.hypot(h) * 0.5;
        let [left, top, right, bottom] = crop.map(|v| v.max(0.));
        let (x0, x1) = (left - w * 0.5, right.min(w) - w * 0.5);
        let (y0, y1) = (top - h * 0.5, bottom.min(h) - h * 0.5);
        let inside = |dx: f32, dy: f32, fill: f32| {
            let s = self
                .radial_scale_with(dx.hypot(dy) * fill / half, amount)
                .into_iter()
                .fold(0f32, f32::max);
            dx.abs() * fill * s <= w * 0.5 && dy.abs() * fill * s <= h * 0.5
        };
        // About one sample per pixel along the edge, so no extremum between samples
        // leaves the image by more than a fraction of a pixel.
        let steps = (w.max(h).ceil() as usize).clamp(64, 8192);
        let mut fill = 1f32;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let (x, y) = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
            for (dx, dy) in [(x, y0), (x, y1), (x0, y), (x1, y)] {
                if inside(dx, dy, fill) {
                    continue;
                }
                let (mut lo, mut hi) = (whole, fill);
                for _ in 0..24 {
                    let mid = (lo + hi) * 0.5;
                    if inside(dx, dy, mid) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                fill = lo;
            }
        }
        fill
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
    #[test]
    fn fill_within_a_crop_of_a_wider_sensor_is_looser_and_still_inside() {
        let c = LensCorrection {
            distortion: Radial::new(vec![0., 1.], vec![1., 1.08]),
            ..Default::default()
        };
        let size = [6000., 4000.];
        // A 16:9 crop of the 3:2 image.
        let crop = [0., 437.5, 6000., 3812.5];
        let whole = c.fill_scale_with(1.);
        let within = c.fill_scale_within(1., size, crop);
        assert!(within > whole + 0.005 && within <= 1., "{whole} {within}");
        let half = 6000f32.hypot(4000.) * 0.5;
        for (x, y) in [
            (3000f32, 1562.5f32),
            (3000., 0.),
            (0., 1562.5),
            (1500., 1562.5),
        ] {
            let s = c.radial_scale_with(x.hypot(y) * within / half, 1.)[1];
            assert!(
                x * within * s <= 3000.01 && y * within * s <= 2000.01,
                "{x} {y}"
            );
        }
        // The whole image is never tighter than its own scale; no distortion needs none.
        let all = c.fill_scale_within(1., size, [0., 0., 6000., 4000.]);
        assert!(all >= whole && all < whole + 0.01, "{whole} {all}");
        assert_eq!(
            LensCorrection::default().fill_scale_within(1., size, crop),
            1.
        );
    }
    #[test]
    fn fill_within_leaves_no_edge_pixel_outside_between_samples() {
        // A table with bumps, the kind Sony's signed values give.
        let c = LensCorrection {
            distortion: Radial::new(
                (0..11).map(|i| i as f32 / 10.).collect(),
                vec![
                    1.08, 1.06, 1.04, 1.02, 1.02, 1.02, 1.02, 1.02, 1.02, 1.03, 0.99,
                ],
            ),
            ..Default::default()
        };
        let (w, h) = (6000f32, 4000f32);
        let fill = c.fill_scale_within(1., [w, h], [0., 0., w, h]);
        let half = w.hypot(h) * 0.5;
        let mut worst = 0f32;
        for i in 0..6000 {
            for (px, py) in [(i as f32 + 0.5, 0.5), (i as f32 + 0.5, h - 0.5)] {
                let (dx, dy) = (px - w * 0.5, py - h * 0.5);
                let s = c.radial_scale_with(dx.hypot(dy) * fill / half, 1.)[1];
                worst = worst.max((dx * fill * s).abs() - w * 0.5);
                worst = worst.max((dy * fill * s).abs() - h * 0.5);
            }
        }
        assert!(worst < 0.5, "{worst}");
    }
}
