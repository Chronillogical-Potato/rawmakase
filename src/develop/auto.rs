//! One-click Auto: the Basic tone sliders, and white balance, chosen for a photo.
//!
//! White balance is estimated from the camera pixels. The tone sliders are fitted by
//! rendering a small copy of the photo through the real pipeline and measuring the
//! result, so the estimates follow the sliders as they render rather than a model of
//! them.
use super::{
    Recipe,
    pipeline::{preview, render},
    quality::recovered,
};
use crate::{
    color_math::srgb_decode,
    raw::{CameraImage, Metadata},
};
use anyhow::{Result, bail, ensure};
use std::sync::atomic::{AtomicBool, Ordering};

/// Long edge of the cropped render the tone estimates are measured on.
const ANALYSIS_EDGE: u32 = 256;
/// Largest long edge of the reduced photo that render is cropped from, so a tight
/// crop never copies a full-size photo (about 19 MB at 1536 × 1024).
const ANALYSIS_SOURCE_MAX: u32 = 1536;
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
/// Fraction of pixels Auto may newly clip when it raises exposure, beyond those already
/// clipped at Exposure 0.
const MAX_CLIPPED: f32 = 0.01;

/// `base` with white balance chosen so the photo's near-neutral areas render neutral.
pub fn auto_white_balance(im: &CameraImage, base: &Recipe) -> Result<Recipe> {
    auto_white_balance_cancellable(im, base, &AtomicBool::new(false))
}

/// [`auto_white_balance`] that stops with an error once `cancel` is set.
pub fn auto_white_balance_cancellable(
    im: &CameraImage,
    base: &Recipe,
    cancel: &AtomicBool,
) -> Result<Recipe> {
    check_cancel(cancel)?;
    // Camera pixels as decoded: highlight recovery invents colour where a channel
    // clipped, which must not count as a neutral.
    let small = preview(im, analysis_edge(im, base));
    check_cancel(cancel)?;
    let mut r = base.clone();
    fit_white_balance(&small, &mut r)?;
    r.validate()?;
    Ok(r)
}

/// `base` with Exposure, Contrast, Highlights, Shadows, Whites and Blacks fitted to the
/// photo before its adjustments (see [`auto_tone_basis`]), as the Basic panel's Auto
/// button applies them. White balance and every other setting of `base` are kept, as
/// in Lightroom.
pub fn auto_tone(im: &CameraImage, base: &Recipe) -> Result<Recipe> {
    auto_tone_cancellable(im, base, &AtomicBool::new(false))
}

/// [`auto_tone`] that stops with an error between renders once `cancel` is set.
pub fn auto_tone_cancellable(
    im: &CameraImage,
    base: &Recipe,
    cancel: &AtomicBool,
) -> Result<Recipe> {
    let mut t = auto_tone_basis(base);
    let small = tone_copy(im, &t, cancel)?;
    fit_tone(&small, &mut t, cancel)?;
    let mut r = base.clone();
    r.exposure = t.exposure;
    r.contrast = t.contrast;
    r.highlights = t.highlights;
    r.shadows = t.shadows;
    r.whites = t.whites;
    r.blacks = t.blacks;
    r.validate()?;
    Ok(r)
}

/// The photo Auto tone measures: `r` as its profile, white balance, calibration, lens
/// corrections and geometry render it, without the adjustments made on top (the tone
/// sliders, curves and Levels, presence, color mixer, B&W, grading, detail, effects,
/// spots and masks). Lightroom's Auto ignores them too: a point curve that lifts black
/// far above zero still leaves its Auto Blacks at ordinary values, which measuring
/// through the curve could not give. Auto therefore gives the same result whatever
/// those adjustments are.
pub fn auto_tone_basis(r: &Recipe) -> Recipe {
    let d = Recipe::default();
    let e = &r.effects;
    Recipe {
        exposure: 0.,
        contrast: 0.,
        highlights: 0.,
        shadows: 0.,
        whites: 0.,
        blacks: 0.,
        black_point: d.black_point,
        white_point: d.white_point,
        midtone: d.midtone,
        curve: d.curve,
        saturation: d.saturation,
        vibrance: d.vibrance,
        hsl: d.hsl,
        grading: d.grading,
        noise_luma: d.noise_luma,
        noise_chroma: d.noise_chroma,
        sharpening: d.sharpening,
        sharpening_radius: d.sharpening_radius,
        sharpening_detail: d.sharpening_detail,
        sharpening_masking: d.sharpening_masking,
        retouch: Vec::new(),
        masks: Vec::new(),
        effects: crate::develop::effects::Effects {
            calibration: e.calibration,
            shadow_tint: e.shadow_tint,
            lens_vignette: e.lens_vignette,
            lens_vignette_midpoint: e.lens_vignette_midpoint,
            defringe: e.defringe,
            defringe_ranges: e.defringe_ranges,
            ..d.effects
        },
        ..r.clone()
    }
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("Cancelled");
    }
    Ok(())
}

/// Long edge of a reduced copy of `im` whose crop under `r` has a long edge of about
/// [`ANALYSIS_EDGE`], so a tight crop is still measured on enough pixels, up to
/// [`ANALYSIS_SOURCE_MAX`] for the whole copy.
fn analysis_edge(im: &CameraImage, r: &Recipe) -> u32 {
    let [x0, y0, x1, y1] = r.crop;
    // The crop's sides may be in rotated (oriented) coordinates, so the shorter source
    // side gives a crop edge that is never overestimated.
    let crop_edge = ((x1 - x0).max(y1 - y0) * im.width.min(im.height) as f32).max(1.);
    let long_edge = im.width.max(im.height);
    let scale = (ANALYSIS_EDGE as f32 / crop_edge).min(1.);
    ((long_edge as f32 * scale).ceil() as u32).clamp(1, long_edge.min(ANALYSIS_SOURCE_MAX))
}

/// The reduced copy the tone sliders are fitted on (see [`analysis_edge`]).
///
/// Engines that recover highlights get a copy reduced from the recovered image, as the
/// app's preview and exports are, so a small clipped highlight is recovered before
/// averaging hides it. The copy is marked as already recovered, so rendering it does
/// not recover it again.
fn tone_copy(im: &CameraImage, r: &Recipe, cancel: &AtomicBool) -> Result<CameraImage> {
    let edge = analysis_edge(im, r);
    // Engines before 3 render without highlight recovery.
    if r.engine < 3 {
        return Ok(preview(im, edge));
    }
    let small = preview(&*recovered(im, cancel)?, edge);
    let _ = small.recovered.set(std::sync::Arc::new(small.clone()));
    Ok(small)
}

/// Lightroom's Auto white balance, as measured on 133 photos from three cameras (see
/// docs/tone-controls.md): gray world, then this many mired warmer…
const WARM_MIRED: f32 = 23.;
/// …and this much greener,
const TINT_SHIFT: f32 = -3.;
/// within these Temperature and Tint limits.
const AUTO_TEMPERATURE: (f32, f32) = (2850., 7500.);
const AUTO_TINT: (f32, f32) = (0., 30.);

fn fit_white_balance(im: &CameraImage, r: &mut Recipe) -> Result<()> {
    fit_white_balance_to(&crop_samples(im, r), &im.metadata, r)
}

fn fit_white_balance_to(samples: &[[f32; 3]], m: &Metadata, r: &mut Recipe) -> Result<()> {
    r.wb = gray_world(samples)?;
    r.sync_white_balance_controls(m);
    let (lo, hi) = AUTO_TEMPERATURE;
    let mired = (1e6 / r.temperature - WARM_MIRED).max(1e6 / hi);
    r.temperature = (1e6 / mired).clamp(lo, hi);
    r.tint = (r.tint + TINT_SHIFT).clamp(AUTO_TINT.0, AUTO_TINT.1);
    r.update_wb(m);
    r.auto_white_balance = Some([r.temperature, r.tint]);
    Ok(())
}

/// Camera pixels inside the crop of `r`, sampled on a grid of the output through the
/// recipe's geometry (orientation, straighten, Transform) and lens distortion
/// correction, as rendering samples them, so areas cropped away do not pull white
/// balance.
fn crop_samples(im: &CameraImage, r: &Recipe) -> Vec<[f32; 3]> {
    let g = super::Geometry::new(im, r, ANALYSIS_EDGE);
    let lens = super::image_space::LensMap::new(im, r);
    let (columns, rows) = (g.width.max(1), g.height.max(1));
    let mut samples = Vec::with_capacity((columns * rows) as usize);
    for row in 0..rows {
        for column in 0..columns {
            let u = (column as f32 + 0.5) / columns as f32;
            let v = (row as f32 + 0.5) / rows as f32;
            let [x, y] = g.source(u, v);
            let [x, y] = lens.as_ref().map_or([x, y], |l| l.forward(x, y));
            let (x, y) = (x.round(), y.round());
            if x >= 0. && y >= 0. && x < im.width as f32 && y < im.height as f32 {
                samples.push(im.pixels[y as usize * im.width as usize + x as usize]);
            }
        }
    }
    samples
}

/// Per-channel gains, relative to As Shot and normalised to green, that make the
/// average of the photo neutral (gray world). Pixels near clipping or in the noise
/// floor are ignored.
fn gray_world(pixels: &[[f32; 3]]) -> Result<[f32; 3]> {
    let mut sum = [0f64; 3];
    let mut count = 0usize;
    for p in pixels {
        if p.iter().all(|v| *v > 0.002 && *v < 0.9) {
            for c in 0..3 {
                sum[c] += p[c] as f64;
            }
            count += 1;
        }
    }
    ensure!(count >= 16, "No usable pixels for automatic white balance");
    Ok(std::array::from_fn(|c| (sum[1] / sum[c]) as f32))
}

/// Sorted luminance and brightest-channel values of a render, in display encoding.
struct Measure {
    luma: Vec<f32>,
    peak: Vec<f32>,
}
impl Measure {
    fn of(im: &CameraImage, r: &Recipe, cancel: &AtomicBool) -> Result<Self> {
        check_cancel(cancel)?;
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

fn fit_tone(im: &CameraImage, r: &mut Recipe, cancel: &AtomicBool) -> Result<()> {
    r.exposure = 0.;
    r.contrast = 0.;
    r.highlights = 0.;
    r.shadows = 0.;
    r.whites = 0.;
    r.blacks = 0.;
    // Spot removal barely moves the statistics and is the slowest stage to render.
    let mut t = r.clone();
    t.retouch.clear();

    t.exposure = fit_exposure(im, &mut t, cancel)?;
    let m = Measure::of(im, &t, cancel)?;

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
        Ok(Measure::of(im, &t, cancel)?.peak(0.998) - TARGET_WHITE)
    })?;
    if t.whites <= WHITES.0 {
        t.whites = WHITES.0;
        if Measure::of(im, &t, cancel)?.peak(0.998) > TARGET_WHITE + 0.004 {
            t.whites = 0.;
        }
    }
    t.blacks = solve(BLACKS.0, BLACKS.1, 0.05, |b| {
        t.blacks = b;
        Ok(Measure::of(im, &t, cancel)?.luma(0.002) - TARGET_BLACK)
    })?;
    // Shadows crushed to black in the camera stay black at any Blacks, so when even the
    // highest setting cannot reach the target, Blacks stays at 0 rather than lifting
    // the rest of the shadows.
    if t.blacks >= BLACKS.1 {
        t.blacks = BLACKS.1;
        if Measure::of(im, &t, cancel)?.luma(0.002) < TARGET_BLACK - 0.004 {
            t.blacks = 0.;
        }
    }

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
/// [`MAX_CLIP_CONCESSION`] EV, while it clips more than [`MAX_CLIPPED`] of the photo
/// beyond what already clips at Exposure 0 (highlights clipped in the camera).
fn fit_exposure(im: &CameraImage, t: &mut Recipe, cancel: &AtomicBool) -> Result<f32> {
    let log_luminance = |v: f32| srgb_decode(v).max(1e-5).log2();
    let target = log_luminance(TARGET_MEDIAN);
    let mut e = 0f32;
    let mut previous: Option<(f32, f32)> = None;
    let mut clipped_at_zero = 0.;
    for i in 0..6 {
        t.exposure = e;
        let m = Measure::of(im, t, cancel)?;
        if i == 0 {
            clipped_at_zero = m.clipped();
        }
        let error = log_luminance(m.luma(0.5)) - target;
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
    let allowed = clipped_at_zero + MAX_CLIPPED;
    t.exposure = e;
    if Measure::of(im, t, cancel)?.clipped() <= allowed {
        return Ok(e);
    }
    let lowest = (e - MAX_CLIP_CONCESSION).max(0.);
    // Bisection for the highest exposure that keeps clipping in bounds.
    let (mut lo, mut hi) = (lowest, e);
    for _ in 0..4 {
        let mid = (lo + hi) / 2.;
        t.exposure = mid;
        if Measure::of(im, t, cancel)?.clipped() <= allowed {
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
        Measure::of(im, r, &AtomicBool::new(false))
            .unwrap()
            .luma(0.5)
    }

    #[test]
    fn white_balance_neutralises_a_colour_cast() {
        let cast = [1.4, 1., 0.6];
        let gains = gray_world(&scene(cast, 0.5).pixels).unwrap();
        for c in 0..3 {
            assert!(
                (gains[c] * cast[c] - 1.).abs() < 0.02,
                "{gains:?} does not undo {cast:?}"
            );
        }
    }

    /// Temperature and Tint Lightroom's Auto gives a photo whose gray world is `r`'s.
    fn lightroom_auto(r: &Recipe) -> (f32, f32) {
        let temperature = 1e6 / (1e6 / r.temperature - WARM_MIRED).max(1e6 / AUTO_TEMPERATURE.1);
        (
            temperature.clamp(AUTO_TEMPERATURE.0, AUTO_TEMPERATURE.1),
            (r.tint + TINT_SHIFT).clamp(AUTO_TINT.0, AUTO_TINT.1),
        )
    }

    #[test]
    fn white_balance_is_gray_world_made_warmer_as_in_lightroom() {
        let mut im = scene([1.1, 1., 0.9], 0.5);
        // A camera matrix gives the recipe the default camera-matrix profile.
        im.metadata.cam_xyz = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        let base = Recipe::for_metadata(&im.metadata);
        let auto = auto_white_balance(&im, &base).unwrap();
        let mut gray = base.clone();
        gray.wb = gray_world(&im.pixels).unwrap();
        gray.sync_white_balance_controls(&im.metadata);
        let (temperature, tint) = lightroom_auto(&gray);
        assert!(
            (auto.temperature - temperature).abs() < 1. && (auto.tint - tint).abs() < 0.01,
            "{} {} vs {temperature} {tint}",
            auto.temperature,
            auto.tint
        );
        assert!(auto.temperature > gray.temperature, "{}", auto.temperature);
        assert_eq!(auto.auto_white_balance, Some([auto.temperature, auto.tint]));
    }

    #[test]
    fn white_balance_stays_within_lightroom_auto_limits() {
        for cast in [[0.3, 1., 3.], [3., 1., 0.3], [1., 0.5, 1.], [1., 2., 1.]] {
            let mut im = scene(cast, 0.3);
            im.metadata.cam_xyz = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
            let auto = auto_white_balance(&im, &Recipe::for_metadata(&im.metadata)).unwrap();
            assert!(
                (AUTO_TEMPERATURE.0..=AUTO_TEMPERATURE.1).contains(&auto.temperature)
                    && (AUTO_TINT.0..=AUTO_TINT.1).contains(&auto.tint),
                "{cast:?}: {} {}",
                auto.temperature,
                auto.tint
            );
        }
    }

    #[test]
    fn legacy_recipes_get_white_balance_controls_that_match_the_gains() {
        // Engine 3 without a camera profile uses the fallback white balance model.
        let im = scene([1.3, 1., 0.7], 0.5);
        let base = Recipe {
            engine: 3,
            ..Default::default()
        };
        assert!(base.color_profile(&im.metadata).is_none());
        let auto = auto_white_balance(&im, &base).unwrap();
        // Moving no slider from here keeps Auto's gains.
        let mut replayed = auto.clone();
        replayed.update_wb(&im.metadata);
        for c in 0..3 {
            assert!(
                (replayed.wb[c] / auto.wb[c] - 1.).abs() < 0.01,
                "{:?} from temperature {} tint {} vs {:?}",
                replayed.wb,
                auto.temperature,
                auto.tint,
                auto.wb
            );
        }
    }

    #[test]
    fn legacy_white_balance_beyond_the_controls_follows_the_clamped_controls() {
        let im = scene([1.; 3], 0.5);
        let mut r = Recipe {
            engine: 3,
            // Far bluer and greener than any Temperature and Tint can show.
            wb: [0.05, 1., 20.],
            ..Default::default()
        };
        r.sync_white_balance_controls(&im.metadata);
        let mut replayed = r.clone();
        replayed.update_wb(&im.metadata);
        assert_eq!(
            replayed.wb, r.wb,
            "temperature {} tint {}",
            r.temperature, r.tint
        );
    }

    #[test]
    fn tight_crops_are_measured_on_enough_pixels() {
        let im = scene([1.; 3], 0.5);
        let r = Recipe {
            crop: [0.45, 0.45, 0.5, 0.5],
            ..Default::default()
        };
        // A 960 × 640 photo, whose 5% crop is 48 × 32 source pixels: a 256 px copy of
        // the whole photo would leave about 13 × 9 of them.
        let mut big = scene([1.; 3], 0.5);
        big.width *= 10;
        big.height *= 10;
        big.metadata.width = big.width;
        big.metadata.height = big.height;
        big.pixels = (0..big.width * big.height)
            .map(|i| im.pixels[((i / big.width / 10) * im.width + i % big.width / 10) as usize])
            .collect();
        let out = render(
            &tone_copy(&big, &r, &AtomicBool::new(false)).unwrap(),
            &r,
            0,
        )
        .unwrap();
        // Under ANALYSIS_EDGE, so every source pixel of the crop is measured.
        assert!(
            out.width >= 48 && out.height >= 32,
            "{}x{}",
            out.width,
            out.height
        );
    }

    #[test]
    fn tight_crops_of_large_photos_are_measured_on_a_bounded_copy() {
        // A thin 3000 px photo keeps the test small; a 1% crop of it would ask for the
        // full-size photo.
        let (width, height) = (3000u32, 60u32);
        let mut im = scene([1.; 3], 0.5);
        im.width = width;
        im.height = height;
        im.metadata.width = width;
        im.metadata.height = height;
        im.pixels = vec![[0.2; 3]; (width * height) as usize];
        let r = Recipe {
            crop: [0.5, 0.5, 0.51, 0.51],
            ..Default::default()
        };
        let copy = tone_copy(&im, &r, &AtomicBool::new(false)).unwrap();
        assert_eq!(copy.width.max(copy.height), ANALYSIS_SOURCE_MAX);
    }

    #[test]
    fn white_balance_beyond_the_profile_controls_follows_the_clamped_controls() {
        let mut im = scene([1.; 3], 0.5);
        // A camera matrix gives the recipe the default camera-matrix profile.
        im.metadata.cam_xyz = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        let mut r = Recipe {
            wb: [0.5, 1., 0.5],
            ..Default::default()
        };
        assert!(r.color_profile(&im.metadata).is_some());
        r.sync_white_balance_controls(&im.metadata);
        let mut replayed = r.clone();
        replayed.update_wb(&im.metadata);
        for c in 0..3 {
            assert!(
                (replayed.wb[c] / r.wb[c] - 1.).abs() < 1e-3,
                "{:?} vs {:?} (temperature {} tint {})",
                replayed.wb,
                r.wb,
                r.temperature,
                r.tint
            );
        }
    }

    #[test]
    fn highlights_are_recovered_before_the_photo_is_reduced() {
        // Red clips across the bright side, so highlight recovery changes those pixels.
        let im = scene([1.4, 1., 1.], 1.);
        let r = Recipe::default();
        let recovered = super::super::quality::recover_highlights(&im);
        assert_ne!(recovered.pixels, im.pixels);
        let copy = tone_copy(&im, &r, &AtomicBool::new(false)).unwrap();
        let expected = preview(&recovered, copy.width.max(copy.height));
        assert_eq!(copy.pixels, expected.pixels);
        // Rendering the copy does not recover it a second time.
        assert_eq!(copy.recovered.get().unwrap().pixels, copy.pixels);
    }

    #[test]
    fn fallback_white_balance_sync_under_a_profile_matches_the_gains() {
        let mut im = scene([1.; 3], 0.5);
        im.metadata.cam_xyz = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        for wb in [
            [0.8, 1., 1.3],
            [1.3, 1., 0.8],
            [0.7, 1., 0.9],
            [1.1, 1., 1.4],
        ] {
            let mut r = Recipe {
                wb,
                ..Default::default()
            };
            assert!(r.color_profile(&im.metadata).is_some());
            r.sync_fallback_white_balance_controls(&im.metadata);
            let mut replayed = r.clone();
            replayed.update_wb(&im.metadata);
            for c in 0..3 {
                assert!(
                    (replayed.wb[c] / r.wb[c] - 1.).abs() < 1e-3,
                    "{wb:?}: {:?} vs {:?}",
                    replayed.wb,
                    r.wb
                );
            }
        }
    }

    #[test]
    fn a_cancelled_white_balance_estimate_stops_with_an_error() {
        let im = scene([1.; 3], 0.5);
        let cancel = AtomicBool::new(true);
        assert!(auto_white_balance_cancellable(&im, &Recipe::default(), &cancel).is_err());
    }

    #[test]
    fn white_balance_ignores_colour_invented_by_highlight_recovery() {
        // A warm left half where every other pixel clipped in red, and a cool right
        // half. Recovery rebuilds the clipped pixels as warm, below the clipping cutoff,
        // which would weigh the warm half double.
        let mut im = scene([1.; 3], 0.5);
        let width = im.width as usize;
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = (i % width, i / width);
            *p = if x >= width / 2 {
                [0.4, 0.5, 0.6]
            } else if (x + y) % 2 == 0 {
                [1., 0.5, 0.4]
            } else {
                [0.6, 0.5, 0.4]
            };
        }
        let r = Recipe::for_metadata(&im.metadata);
        assert!(r.engine >= 3);
        let recovered = super::super::quality::recover_highlights(&im);
        assert_ne!(recovered.pixels, im.pixels);
        let mut decoded = r.clone();
        fit_white_balance_to(&crop_samples(&im, &r), &im.metadata, &mut decoded).unwrap();
        let decoded = decoded.wb;
        let estimated = auto_white_balance(&im, &r).unwrap().wb;
        for c in 0..3 {
            assert!(
                (estimated[c] / decoded[c] - 1.).abs() < 1e-4,
                "{estimated:?} vs {decoded:?}"
            );
        }
    }

    #[test]
    fn white_balance_samples_through_lens_distortion_correction() {
        use crate::lens::{LensCorrection, Radial};
        // A neutral photo with a coloured strip down each side, which the distortion
        // correction pulls out of the frame.
        let mut im = scene([1.; 3], 0.5);
        let width = im.width as usize;
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let x = i % width;
            *p = if x < 10 || x >= width - 10 {
                [0.46, 0.4, 0.34]
            } else {
                [0.4; 3]
            };
        }
        im.metadata.lens = Some(LensCorrection {
            source: "test".into(),
            default_on: true,
            vignetting: None,
            distortion: Some(Radial {
                knots: vec![0., 1.],
                values: vec![1., 0.7],
            }),
            chromatic: None,
        });
        let r = Recipe::for_metadata(&im.metadata);
        assert!(r.lens_correction(&im.metadata).is_some());
        let samples = crop_samples(&im, &r);
        assert!(!samples.is_empty());
        assert!(
            samples.iter().all(|p| *p == [0.4; 3]),
            "white balance sampled the strips the correction removes"
        );
    }

    #[test]
    fn a_cancelled_estimate_stops_with_an_error() {
        let im = scene([1.; 3], 0.5);
        let cancel = AtomicBool::new(true);
        assert!(auto_tone_cancellable(&im, &Recipe::default(), &cancel).is_err());
    }

    #[test]
    fn white_balance_comes_from_the_crop() {
        // The left half is under a warm light, the right half under a cool one.
        let mut im = scene([1.; 3], 0.5);
        let width = im.width as usize;
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let cast = if i % width < width / 2 {
                [1.3, 1., 0.7]
            } else {
                [0.7, 1., 1.3]
            };
            *p = std::array::from_fn(|c| p[c] * cast[c]);
        }
        let base = Recipe {
            crop: [0., 0., 0.45, 1.],
            ..Default::default()
        };
        let auto = auto_white_balance(&im, &base).unwrap();
        // As if the whole photo were under the warm light.
        let warm = auto_white_balance(&scene([1.3, 1., 0.7], 0.5), &Recipe::default()).unwrap();
        for c in 0..3 {
            assert!(
                (auto.wb[c] / warm.wb[c] - 1.).abs() < 0.02,
                "{:?} vs {:?}",
                auto.wb,
                warm.wb
            );
        }
    }

    #[test]
    fn neutral_photos_need_no_gray_world_correction() {
        let gains = gray_world(&scene([1.; 3], 0.5).pixels).unwrap();
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
    fn highlights_clipped_in_the_camera_do_not_hold_exposure_back() {
        let clean = scene([1.; 3], 0.25);
        let mut clipped = clean.clone();
        let n = clipped.pixels.len();
        // A light source far beyond the sensor's range: 3% of the photo clips at any
        // exposure.
        for p in &mut clipped.pixels[n - n * 3 / 100..] {
            *p = [8.; 3];
        }
        let base = Recipe::default();
        let expected = auto_tone(&clean, &base).unwrap().exposure;
        let exposure = auto_tone(&clipped, &base).unwrap().exposure;
        assert!(expected > 0.3, "exposure {expected}");
        assert!(
            (exposure - expected).abs() < 0.25,
            "{exposure} vs {expected}"
        );
    }

    #[test]
    fn crushed_shadows_leave_blacks_alone() {
        let mut im = scene([1.; 3], 0.5);
        // Shadows crushed in the camera: 1% of the photo is black.
        let n = im.pixels.len();
        for p in &mut im.pixels[..n / 100] {
            *p = [0.; 3];
        }
        let auto = auto_tone(&im, &Recipe::default()).unwrap();
        assert_eq!(auto.blacks, 0., "blacks {}", auto.blacks);
    }

    #[test]
    fn auto_tone_sets_only_the_tone_sliders() {
        let im = scene([1.2, 1., 0.8], 0.1);
        let mut base = Recipe {
            saturation: 0.3,
            crop: [0.1, 0.1, 0.9, 0.9],
            exposure: -3.,
            contrast: 0.9,
            temperature: 4300.,
            tint: 12.,
            wb: [1.1, 1., 0.7],
            ..Default::default()
        };
        base.effects.clarity = -0.2;
        let auto = auto_tone(&im, &base).unwrap();
        auto.validate().unwrap();
        assert_ne!(auto.exposure, base.exposure);
        // White balance, even a manual one, and everything else is as it was.
        let mut expected = auto.clone();
        expected.exposure = base.exposure;
        expected.contrast = base.contrast;
        expected.highlights = base.highlights;
        expected.shadows = base.shadows;
        expected.whites = base.whites;
        expected.blacks = base.blacks;
        assert_eq!(expected, base);
        // Estimates are independent of the tone sliders they replace.
        let untouched = Recipe {
            exposure: 0.,
            contrast: 0.,
            ..base.clone()
        };
        let fresh = auto_tone(&im, &untouched).unwrap();
        assert_eq!(fresh.exposure, auto.exposure);
        assert_eq!(fresh.whites, auto.whites);
    }

    #[test]
    fn auto_tone_measures_the_photo_before_its_adjustments() {
        let im = scene([1.2, 1., 0.8], 0.1);
        let plain = Recipe::default();
        // A lifted point curve, color and presence edits, as a film-look preset makes.
        let mut adjusted = Recipe {
            saturation: -0.2,
            vibrance: 0.3,
            ..Default::default()
        };
        adjusted.curve.points = vec![[0., 42. / 255.], [1., 1.]];
        adjusted.hsl[1] = [0.1, -0.3, -0.2];
        adjusted.effects.clarity = 0.4;
        adjusted.effects.dehaze = 0.3;
        adjusted.effects.channels[2].points = vec![[0., 0.], [0.3, 0.2], [1., 1.]];
        adjusted.effects.vignette = -0.5;
        let a = auto_tone(&im, &plain).unwrap();
        let b = auto_tone(&im, &adjusted).unwrap();
        let tone = |r: &Recipe| {
            [
                r.exposure,
                r.contrast,
                r.highlights,
                r.shadows,
                r.whites,
                r.blacks,
            ]
        };
        assert_eq!(tone(&a), tone(&b));
        // The adjustments themselves are kept.
        assert_eq!(b.curve, adjusted.curve);
        assert_eq!(b.effects, adjusted.effects);
        assert_eq!(b.hsl, adjusted.hsl);
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
