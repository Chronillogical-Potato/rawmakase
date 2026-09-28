//! One-click Auto: white balance and the Basic tone sliders, chosen for a photo.
//!
//! White balance is estimated from the camera pixels. The tone sliders are fitted by
//! rendering a small copy of the photo through the real pipeline and measuring the
//! result, so the estimates follow the sliders as they render rather than a model of
//! them.
use super::{
    Recipe,
    pipeline::{preview, render},
};
use crate::{color_math::srgb_decode, raw::CameraImage};
use anyhow::{Result, ensure};

/// Long edge of the copy the tone estimates are measured on.
const ANALYSIS_EDGE: u32 = 256;
/// Display (sRGB-encoded) luminance Auto places the photo's median at: 18% gray.
const TARGET_MEDIAN: f32 = 0.46;
/// Where Auto places the brightest channel of the brightest 0.2% of pixels.
const TARGET_WHITE: f32 = 0.97;
/// Where Auto places the darkest 0.2% of pixels' luminance.
const TARGET_BLACK: f32 = 0.015;
/// Slider limits for Whites and Blacks. Strong positive Whites and lifted Blacks look
/// flat, and positive Whites is not yet image-adaptive (see docs/tone-controls.md).
const WHITES: (f32, f32) = (-0.5, 0.35);
const BLACKS: (f32, f32) = (-0.5, 0.2);
/// Positive Exposure gives up to this many EV of the median target to avoid clipping.
const MAX_CLIP_CONCESSION: f32 = 1.;
/// Fraction of pixels allowed to clip after Auto raises exposure.
const MAX_CLIPPED: f32 = 0.01;

/// White balance and tone together, as the Basic panel's Auto button applies them.
/// Every other setting of `base` is kept.
pub fn auto_adjust(im: &CameraImage, base: &Recipe) -> Result<Recipe> {
    let small = preview(im, ANALYSIS_EDGE);
    let mut r = base.clone();
    fit_white_balance(&small, &mut r)?;
    fit_tone(&small, &mut r)?;
    r.validate()?;
    Ok(r)
}

/// `base` with white balance chosen so the photo's near-neutral areas render neutral.
pub fn auto_white_balance(im: &CameraImage, base: &Recipe) -> Result<Recipe> {
    let mut r = base.clone();
    fit_white_balance(&preview(im, ANALYSIS_EDGE), &mut r)?;
    r.validate()?;
    Ok(r)
}

/// `base` with Exposure, Contrast, Highlights, Shadows, Whites and Blacks fitted to the
/// photo as `base` otherwise renders it.
pub fn auto_tone(im: &CameraImage, base: &Recipe) -> Result<Recipe> {
    let small = preview(im, ANALYSIS_EDGE);
    let mut r = base.clone();
    fit_tone(&small, &mut r)?;
    r.validate()?;
    Ok(r)
}

fn fit_white_balance(im: &CameraImage, r: &mut Recipe) -> Result<()> {
    r.wb = neutral_gains(im)?;
    r.sync_white_balance_controls(&im.metadata);
    Ok(())
}

/// Per-channel white balance gains, relative to As Shot and normalised to green.
///
/// Starting from As Shot, each pass averages only the pixels that look nearly neutral
/// under the previous estimate, with a tighter tolerance each time, so a large
/// coloured surface (grass, sky, a wall) pulls less than in plain gray world. Gray
/// world is the fallback when too little of the photo is near neutral. Pixels near
/// clipping or in the noise floor are ignored.
fn neutral_gains(im: &CameraImage) -> Result<[f32; 3]> {
    let usable: Vec<[f32; 3]> = im
        .pixels
        .iter()
        .filter(|p| p.iter().all(|v| *v > 0.002 && *v < 0.9))
        .copied()
        .collect();
    ensure!(
        usable.len() >= 16,
        "No usable pixels for automatic white balance"
    );
    let mut gains = None;
    for tolerance in [0.5, 0.25, 0.12] {
        match neutral_average(&usable, gains.unwrap_or([1.; 3]), tolerance) {
            Some(g) => gains = Some(g),
            // Too few neutral candidates: the previous, broader estimate is safer.
            None => break,
        }
    }
    Ok(gains
        .or_else(|| neutral_average(&usable, [1.; 3], f32::INFINITY))
        .unwrap_or([1.; 3]))
}

/// Gains that make the average of the pixels within `tolerance` of neutral under
/// `gains` neutral, if enough of the photo qualifies.
fn neutral_average(pixels: &[[f32; 3]], gains: [f32; 3], tolerance: f32) -> Option<[f32; 3]> {
    let mut sum = [0f64; 3];
    let mut count = 0usize;
    for p in pixels {
        if chroma(std::array::from_fn(|c| p[c] * gains[c])) <= tolerance {
            for c in 0..3 {
                sum[c] += p[c] as f64;
            }
            count += 1;
        }
    }
    if count < (pixels.len() / 50).max(16) {
        return None;
    }
    // Corrections beyond two stops per channel are more likely a coloured subject than
    // a coloured light.
    Some(std::array::from_fn(|c| {
        (sum[1] / sum[c].max(1e-12)).clamp(0.25, 4.) as f32
    }))
}

/// Distance of a colour from neutral in normalised chromaticity: 0 for gray, 0.2 for
/// about ±10% red and blue, 2 for a pure primary.
fn chroma(p: [f32; 3]) -> f32 {
    let sum = p[0] + p[1] + p[2];
    if sum <= 0. {
        return f32::INFINITY;
    }
    3. * ((p[0] / sum - 1. / 3.).abs() + (p[2] / sum - 1. / 3.).abs())
}

/// Sorted luminance and brightest-channel values of a render, in display encoding.
struct Measure {
    luma: Vec<f32>,
    peak: Vec<f32>,
}
impl Measure {
    fn of(im: &CameraImage, r: &Recipe) -> Result<Self> {
        let out = render(im, r, 0)?;
        ensure!(!out.pixels.is_empty(), "Nothing to measure for Auto");
        let mut luma = Vec::with_capacity(out.pixels.len());
        let mut peak = Vec::with_capacity(out.pixels.len());
        for p in &out.pixels {
            let p = p.map(|v| if v.is_finite() { v.clamp(0., 1.) } else { 0. });
            luma.push(0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]);
            peak.push(p[0].max(p[1]).max(p[2]));
        }
        luma.sort_by(f32::total_cmp);
        peak.sort_by(f32::total_cmp);
        Ok(Self { luma, peak })
    }
    fn luma(&self, q: f32) -> f32 {
        percentile(&self.luma, q)
    }
    fn peak(&self, q: f32) -> f32 {
        percentile(&self.peak, q)
    }
    /// Fraction of pixels with a channel at the top of the output range.
    fn clipped(&self) -> f32 {
        let from = self.peak.partition_point(|v| *v < 0.995);
        (self.peak.len() - from) as f32 / self.peak.len() as f32
    }
}
fn percentile(sorted: &[f32], q: f32) -> f32 {
    sorted[((sorted.len() - 1) as f32 * q.clamp(0., 1.)).round() as usize]
}

fn fit_tone(im: &CameraImage, r: &mut Recipe) -> Result<()> {
    r.exposure = 0.;
    r.contrast = 0.;
    r.highlights = 0.;
    r.shadows = 0.;
    r.whites = 0.;
    r.blacks = 0.;
    // Spot removal barely moves the statistics and is the slowest stage to render.
    let mut t = r.clone();
    t.retouch.clear();

    t.exposure = fit_exposure(im, &mut t)?;
    let m = Measure::of(im, &t)?;

    // Recover bright highlights and open deep shadows, in proportion to how much of
    // the photo sits there.
    t.highlights = -((m.luma(0.97) - 0.8) / 0.18).clamp(0., 1.) * 0.6;
    t.shadows = ((0.15 - m.luma(0.05)) / 0.15).clamp(0., 1.) * 0.5;
    // Flat photos get contrast, very contrasty ones lose a little.
    let spread = m.luma(0.75) - m.luma(0.25);
    t.contrast = ((0.36 - spread) * 1.2).clamp(-0.2, 0.25);

    // Whites and Blacks set the end points, each measured as it renders. Highlights
    // clipped in the camera stay clipped at any Whites, so when even the lowest setting
    // cannot reach the target, Whites stays at 0 and Highlights does the recovery.
    t.whites = solve(WHITES.0, WHITES.1, 0.2, |w| {
        t.whites = w;
        Ok(Measure::of(im, &t)?.peak(0.998) - TARGET_WHITE)
    })?;
    if t.whites <= WHITES.0 {
        t.whites = 0.;
    }
    t.blacks = solve(BLACKS.0, BLACKS.1, 0.05, |b| {
        t.blacks = b;
        Ok(Measure::of(im, &t)?.luma(0.002) - TARGET_BLACK)
    })?;

    r.exposure = round(t.exposure, 100.);
    r.contrast = round(t.contrast, 100.);
    r.highlights = round(t.highlights, 100.);
    r.shadows = round(t.shadows, 100.);
    r.whites = round(t.whites, 100.);
    r.blacks = round(t.blacks, 100.);
    Ok(())
}

/// Exposure that brings the median to [`TARGET_MEDIAN`], found by the secant method on
/// the median's log linear luminance. Positive exposure is then reduced, by at most
/// [`MAX_CLIP_CONCESSION`] EV, while more than [`MAX_CLIPPED`] of the photo clips.
fn fit_exposure(im: &CameraImage, t: &mut Recipe) -> Result<f32> {
    let log_luminance = |v: f32| srgb_decode(v).max(1e-5).log2();
    let target = log_luminance(TARGET_MEDIAN);
    let mut e = 0f32;
    let mut previous: Option<(f32, f32)> = None;
    for _ in 0..6 {
        t.exposure = e;
        let error = log_luminance(Measure::of(im, t)?.luma(0.5)) - target;
        if error.abs() < 0.05 {
            break;
        }
        // The tone curve changes the slope; the last two steps estimate it.
        let slope = match previous {
            Some((pe, perr)) if (e - pe).abs() > 1e-3 => ((error - perr) / (e - pe)).clamp(0.3, 3.),
            _ => 1.,
        };
        previous = Some((e, error));
        e = (e - (error / slope).clamp(-2., 2.)).clamp(-4., 4.);
    }
    if e <= 0. {
        return Ok(e);
    }
    t.exposure = e;
    if Measure::of(im, t)?.clipped() <= MAX_CLIPPED {
        return Ok(e);
    }
    let lowest = (e - MAX_CLIP_CONCESSION).max(0.);
    // Bisection for the highest exposure that keeps clipping in bounds.
    let (mut lo, mut hi) = (lowest, e);
    for _ in 0..4 {
        let mid = (lo + hi) / 2.;
        t.exposure = mid;
        if Measure::of(im, t)?.clipped() <= MAX_CLIPPED {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(lo)
}

/// Root of an increasing `f` in `[lo, hi]`, by the secant method from 0 with an initial
/// `slope` guess. Returns the limit when the target is out of reach.
fn solve(lo: f32, hi: f32, slope: f32, mut f: impl FnMut(f32) -> Result<f32>) -> Result<f32> {
    const TOLERANCE: f32 = 0.004;
    let (mut x0, mut f0) = (0., f(0.)?);
    if f0.abs() < TOLERANCE {
        return Ok(0.);
    }
    let mut x1 = (-f0 / slope).clamp(lo, hi);
    for _ in 0..4 {
        let f1 = f(x1)?;
        if f1.abs() < TOLERANCE || (x1 == hi && f1 < 0.) || (x1 == lo && f1 > 0.) {
            break;
        }
        let measured = if (x1 - x0).abs() > 1e-4 {
            (f1 - f0) / (x1 - x0)
        } else {
            slope
        };
        (x0, f0) = (x1, f1);
        x1 = (x1 - f1 / measured.max(slope * 0.1)).clamp(lo, hi);
    }
    Ok(x1)
}

fn round(v: f32, steps: f32) -> f32 {
    (v * steps).round() / steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::Metadata;

    /// A 96 × 64 scene of smooth gradients, all channels scaled by `cast`.
    fn scene(cast: [f32; 3], level: f32) -> CameraImage {
        let (width, height) = (96u32, 64u32);
        CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: (0..width * height)
                .map(|i| {
                    let (x, y) = ((i % width) as f32 / width as f32, (i / width) as f32);
                    let v = level * (0.02 + 0.98 * x * x) * (1. + 0.1 * (y * 0.3).sin());
                    std::array::from_fn(|c| (v * cast[c]).min(1.))
                })
                .collect(),
            metadata: Metadata {
                width,
                height,
                wb: [1.; 3],
                daylight_wb: [1.; 3],
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        }
    }
    fn median(im: &CameraImage, r: &Recipe) -> f32 {
        Measure::of(im, r).unwrap().luma(0.5)
    }

    #[test]
    fn white_balance_neutralises_a_colour_cast() {
        let cast = [1.4, 1., 0.6];
        let gains = neutral_gains(&scene(cast, 0.5)).unwrap();
        for c in 0..3 {
            assert!(
                (gains[c] * cast[c] - 1.).abs() < 0.02,
                "{gains:?} does not undo {cast:?}"
            );
        }
    }

    #[test]
    fn white_balance_follows_neutral_areas_over_a_coloured_surface() {
        // Two thirds of the photo is saturated green; the rest is gray under a mild
        // warm light, which is what should be corrected.
        let mut im = scene([1.1, 1., 0.9], 0.5);
        let n = im.pixels.len();
        for p in &mut im.pixels[..n * 2 / 3] {
            *p = [0.05, 0.4, 0.08];
        }
        let gains = neutral_gains(&im).unwrap();
        assert!((gains[0] * 1.1 - 1.).abs() < 0.03, "{gains:?}");
        assert!((gains[2] * 0.9 - 1.).abs() < 0.03, "{gains:?}");
    }

    #[test]
    fn neutral_photos_keep_as_shot_white_balance() {
        let gains = neutral_gains(&scene([1.; 3], 0.5)).unwrap();
        assert!(gains.iter().all(|g| (g - 1.).abs() < 1e-3), "{gains:?}");
    }

    #[test]
    fn exposure_brings_dark_and_bright_photos_towards_middle_gray() {
        for (level, brighter) in [(0.04, true), (0.9, false)] {
            let im = scene([1.; 3], level);
            let base = Recipe::default();
            let auto = auto_tone(&im, &base).unwrap();
            assert_eq!(auto.exposure > 0., brighter, "exposure {}", auto.exposure);
            let before = (median(&im, &base) - TARGET_MEDIAN).abs();
            let after = (median(&im, &auto) - TARGET_MEDIAN).abs();
            assert!(after < before, "median error {before} -> {after}");
            assert!(after < 0.1, "median error {after}");
        }
    }

    #[test]
    fn auto_sets_only_white_balance_and_tone() {
        let im = scene([1.2, 1., 0.8], 0.1);
        let mut base = Recipe {
            saturation: 0.3,
            crop: [0.1, 0.1, 0.9, 0.9],
            exposure: -3.,
            contrast: 0.9,
            ..Default::default()
        };
        base.effects.clarity = -0.2;
        let auto = auto_adjust(&im, &base).unwrap();
        auto.validate().unwrap();
        assert_ne!(auto.wb, base.wb);
        assert_ne!(auto.exposure, base.exposure);
        // Everything else is as it was.
        let mut expected = auto.clone();
        expected.wb = base.wb;
        expected.temperature = base.temperature;
        expected.tint = base.tint;
        expected.exposure = base.exposure;
        expected.contrast = base.contrast;
        expected.highlights = base.highlights;
        expected.shadows = base.shadows;
        expected.whites = base.whites;
        expected.blacks = base.blacks;
        assert_eq!(expected, base);
        // Estimates are independent of the tone sliders they replace.
        let fresh = auto_adjust(&im, &Recipe::default()).unwrap();
        assert_eq!(fresh.exposure, auto.exposure);
        assert_eq!(fresh.whites, auto.whites);
    }

    #[test]
    fn empty_or_black_photos_are_errors() {
        let mut im = scene([1.; 3], 0.5);
        im.pixels.iter_mut().for_each(|p| *p = [0.; 3]);
        assert!(auto_white_balance(&im, &Recipe::default()).is_err());
    }

    #[test]
    fn solve_finds_roots_and_stops_at_the_limits() {
        let root = solve(-1., 1., 1., |x| Ok(0.3 * x * x * x + 0.5 * x - 0.2)).unwrap();
        assert!(
            (0.3 * root.powi(3) + 0.5 * root - 0.2).abs() < 0.004,
            "{root}"
        );
        assert_eq!(solve(-0.5, 0.5, 1., |x| Ok(x - 2.)).unwrap(), 0.5);
        assert_eq!(solve(-0.5, 0.5, 1., |x| Ok(x + 2.)).unwrap(), -0.5);
        assert_eq!(solve(-0.5, 0.5, 1., |x| Ok(x * 0.001)).unwrap(), 0.);
    }
}
