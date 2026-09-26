//! Natural cubic point curves and compatible legacy linear/monotone curves.
use anyhow::{Result, ensure};
use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ToneCurve {
    pub points: Vec<[f32; 2]>,
    pub smooth: bool,
    pub natural: bool,
}
impl Default for ToneCurve {
    fn default() -> Self {
        Self {
            points: vec![[0., 0.], [1., 1.]],
            smooth: true,
            natural: true,
        }
    }
}
impl<'de> Deserialize<'de> for ToneCurve {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Data {
            points: Vec<[f32; 2]>,
            smooth: bool,
            #[serde(default)]
            natural: bool,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Stored {
            Legacy([f32; 5]),
            Current(Data),
        }
        Ok(match Stored::deserialize(d)? {
            Stored::Legacy(values) => Self {
                points: values
                    .into_iter()
                    .enumerate()
                    .map(|(i, y)| [i as f32 / 4., y])
                    .collect(),
                smooth: false,
                natural: false,
            },
            Stored::Current(data) => Self {
                points: data.points,
                smooth: data.smooth,
                natural: data.natural,
            },
        })
    }
}
impl ToneCurve {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (2..=32).contains(&self.points.len()),
            "Curve needs 2–32 points"
        );
        ensure!(
            self.points
                .iter()
                .flatten()
                .all(|v| v.is_finite() && (0. ..=1.).contains(v)),
            "Invalid curve point"
        );
        ensure!(
            self.natural || (self.points[0][0] == 0. && self.points.last().unwrap()[0] == 1.),
            "Curve endpoints must span 0–1"
        );
        ensure!(
            self.points.windows(2).all(|p| p[1][0] - p[0][0] >= 0.00049),
            "Curve points must have distinct increasing inputs"
        );
        Ok(())
    }
    pub fn insert(&mut self, p: [f32; 2]) -> usize {
        let x = p[0].clamp(0., 1.);
        let y = p[1].clamp(0., 1.);
        if let Some(i) = self.points.iter().position(|q| (q[0] - x).abs() < 0.001) {
            return i;
        }
        let i = self.points.partition_point(|q| q[0] < x);
        if self.points.len() == 32 {
            return i.min(31);
        }
        self.points.insert(i, [x, y]);
        self.smooth = true;
        i
    }
    pub fn move_point(&mut self, i: usize, p: [f32; 2]) {
        if i >= self.points.len() {
            return;
        }
        let last = self.points.len() - 1;
        let x = if i == 0 {
            if self.natural {
                p[0].clamp(0., self.points[1][0] - 0.0005)
            } else {
                0.
            }
        } else if i == last {
            if self.natural {
                p[0].clamp(self.points[last - 1][0] + 0.0005, 1.)
            } else {
                1.
            }
        } else {
            p[0].clamp(
                self.points[i - 1][0] + 0.0005,
                self.points[i + 1][0] - 0.0005,
            )
        };
        self.points[i] = [x, p[1].clamp(0., 1.)];
        self.smooth = true;
    }
    pub fn remove(&mut self, i: usize) -> bool {
        if i == 0 || i + 1 >= self.points.len() {
            return false;
        }
        self.points.remove(i);
        self.smooth = true;
        true
    }
    fn slope(&self, i: usize) -> f32 {
        let a = self.points[i];
        let b = self.points[i + 1];
        (b[1] - a[1]) / (b[0] - a[0])
    }
    fn tangent(&self, i: usize) -> f32 {
        if i == 0 {
            return self.slope(0);
        }
        if i + 1 == self.points.len() {
            return self.slope(i - 1);
        }
        let a = self.slope(i - 1);
        let b = self.slope(i);
        if a * b <= 0. {
            return 0.;
        }
        let h0 = self.points[i][0] - self.points[i - 1][0];
        let h1 = self.points[i + 1][0] - self.points[i][0];
        let w0 = 2. * h1 + h0;
        let w1 = h1 + 2. * h0;
        (w0 + w1) / (w0 / a + w1 / b)
    }
    pub fn evaluate(&self, x: f32) -> f32 {
        if self.natural && self.smooth {
            return NaturalSpline::new(&self.points).evaluate(x);
        }
        let x = x.clamp(0., 1.);
        let i = self
            .points
            .partition_point(|p| p[0] <= x)
            .saturating_sub(1)
            .min(self.points.len() - 2);
        let a = self.points[i];
        let b = self.points[i + 1];
        let h = b[0] - a[0];
        let t = (x - a[0]) / h;
        if !self.smooth {
            return a[1] * (1. - t) + b[1] * t;
        }
        let t2 = t * t;
        let t3 = t2 * t;
        ((2. * t3 - 3. * t2 + 1.) * a[1]
            + (t3 - 2. * t2 + t) * h * self.tangent(i)
            + (-2. * t3 + 3. * t2) * b[1]
            + (t3 - t2) * h * self.tangent(i + 1))
        .clamp(a[1].min(b[1]), a[1].max(b[1]))
    }
}
/// Natural cubic spline: zero second derivatives at the two control endpoints.
/// Unlike the legacy monotone interpolator this permits smooth overshoot,
/// clipped only to the output range. Endpoint clipping follows the point domain.
struct NaturalSpline {
    points: Vec<[f64; 2]>,
    second: Vec<f64>,
}
impl NaturalSpline {
    fn new(points: &[[f32; 2]]) -> Self {
        let points: Vec<_> = points.iter().map(|p| [p[0] as f64, p[1] as f64]).collect();
        let n = points.len();
        let mut second = vec![0.; n];
        let mut upper = vec![0.; n];
        for i in 1..n - 1 {
            let left = points[i][0] - points[i - 1][0];
            let right = points[i + 1][0] - points[i][0];
            let diagonal = 2. * (left + right) - left * upper[i - 1];
            let rhs = 6.
                * ((points[i + 1][1] - points[i][1]) / right
                    - (points[i][1] - points[i - 1][1]) / left);
            upper[i] = right / diagonal;
            second[i] = (rhs - left * second[i - 1]) / diagonal;
        }
        for i in (1..n - 1).rev() {
            second[i] -= upper[i] * second[i + 1];
        }
        Self { points, second }
    }
    fn evaluate(&self, x: f32) -> f32 {
        let x = x as f64;
        let last = self.points.len() - 1;
        if x <= self.points[0][0] {
            return self.points[0][1] as f32;
        }
        if x >= self.points[last][0] {
            return self.points[last][1] as f32;
        }
        let i = self
            .points
            .partition_point(|p| p[0] <= x)
            .saturating_sub(1)
            .min(last - 1);
        let h = self.points[i + 1][0] - self.points[i][0];
        let a = (self.points[i + 1][0] - x) / h;
        let b = (x - self.points[i][0]) / h;
        (a * self.points[i][1]
            + b * self.points[i + 1][1]
            + ((a * a * a - a) * self.second[i] + (b * b * b - b) * self.second[i + 1]) * h * h
                / 6.)
            .clamp(0., 1.) as f32
    }
}

pub struct CurveLut([f32; 4097]);
impl CurveLut {
    pub fn new(curve: &ToneCurve) -> Self {
        if curve.natural && curve.smooth {
            let spline = NaturalSpline::new(&curve.points);
            Self(std::array::from_fn(|i| spline.evaluate(i as f32 / 4096.)))
        } else {
            Self(std::array::from_fn(|i| curve.evaluate(i as f32 / 4096.)))
        }
    }
    pub(crate) fn values(&self) -> &[f32; 4097] {
        &self.0
    }
    pub fn evaluate(&self, x: f32) -> f32 {
        let p = x.clamp(0., 1.) * 4096.;
        let i = (p as usize).min(4095);
        let t = p - i as f32;
        self.0[i] * (1. - t) + self.0[i + 1] * t
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn natural_curve_matches_independent_cubic_reference() {
        let curve = ToneCurve {
            points: vec![
                [0., 0.],
                [0.25, 0.125],
                [0.5, 0.625],
                [0.75, 0.875],
                [1., 1.],
            ],
            smooth: true,
            natural: true,
        };
        // Golden samples generated independently with SciPy's natural cubic solver.
        let reference = [
            (0.125, 0.018973214),
            (0.375, 0.36495536),
            (0.625, 0.78683036),
            (0.875, 0.94084823),
        ];
        let lut = CurveLut::new(&curve);
        for (x, y) in reference {
            assert!(
                (lut.evaluate(x) - y).abs() < 1e-6,
                "{} vs {}",
                lut.evaluate(x),
                y
            );
        }
    }
    #[test]
    fn saved_curves_keep_legacy_interpolation_and_new_endpoints_clip() {
        let old: ToneCurve =
            serde_json::from_str(r#"{"points":[[0,0],[0.4,0.7],[1,1]],"smooth":true}"#).unwrap();
        assert!(!old.natural);
        let new = ToneCurve {
            points: vec![[0.2, 0.1], [0.6, 0.9]],
            smooth: true,
            natural: true,
        };
        new.validate().unwrap();
        assert_eq!(new.evaluate(0.), 0.1);
        assert_eq!(new.evaluate(1.), 0.9);
        assert!((new.evaluate(0.4) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn insert_move_remove_and_endpoints() -> Result<()> {
        let mut c = ToneCurve::default();
        let i = c.insert([0.4, 0.3]);
        assert_eq!(i, 1);
        c.move_point(i, [0.65, 0.7]);
        assert_eq!(c.points[i], [0.65, 0.7]);
        c.move_point(0, [0.2, 0.1]);
        assert_eq!(c.points[0], [0.2, 0.1]);
        assert!(!c.remove(0));
        assert!(c.remove(1));
        assert_eq!(c.points.len(), 2);
        c.validate()
    }
    #[test]
    fn legacy_curve_keeps_exact_linear_shape() -> Result<()> {
        let c: ToneCurve = serde_json::from_str("[0.1,0.2,0.55,0.8,1.0]")?;
        assert!(!c.smooth);
        assert!((c.evaluate(0.375) - 0.375).abs() < 1e-6);
        let loaded: ToneCurve = serde_json::from_str(&serde_json::to_string(&c)?)?;
        assert_eq!(c, loaded);
        Ok(())
    }
    #[test]
    fn smooth_curve_hits_points_is_monotonic_and_has_no_overshoot() {
        let c = ToneCurve {
            points: vec![[0., 0.05], [0.12, 0.06], [0.4, 0.5], [0.8, 0.9], [1., 0.95]],
            smooth: true,
            natural: false,
        };
        let mut prev = 0.;
        for i in 0..=10000 {
            let y = c.evaluate(i as f32 / 10000.);
            assert!(y + 1e-6 >= prev);
            assert!((0.05..=0.95).contains(&y));
            prev = y;
        }
        for [x, y] in &c.points {
            assert!((c.evaluate(*x) - y).abs() < 1e-6);
        }
        for [x, _] in &c.points[1..c.points.len() - 1] {
            let left = (c.evaluate(*x) - c.evaluate(x - 0.0001)) / 0.0001;
            let right = (c.evaluate(x + 0.0001) - c.evaluate(*x)) / 0.0001;
            assert!((left - right).abs() < 0.01);
        }
    }
    #[test]
    fn lut_tracks_spline() {
        let c = ToneCurve {
            points: vec![[0., 0.], [0.3, 0.1], [0.6, 0.8], [1., 1.]],
            smooth: true,
            natural: false,
        };
        let lut = CurveLut::new(&c);
        for i in 0..1000 {
            let x = i as f32 / 1000.;
            assert!((c.evaluate(x) - lut.evaluate(x)).abs() < 1e-5);
        }
    }
    #[test]
    fn nonmonotonic_creative_curve_stays_in_range() -> Result<()> {
        let c = ToneCurve {
            points: vec![[0., 0.], [0.25, 0.8], [0.7, 0.1], [1., 1.]],
            smooth: true,
            natural: false,
        };
        c.validate()?;
        for i in 0..1000 {
            assert!((0. ..=1.).contains(&c.evaluate(i as f32 / 1000.)));
        }
        Ok(())
    }
}
