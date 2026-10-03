use super::*;
use crate::develop::curve::ToneCurve;
fn adjust(p: [f32; 3], m: &Metadata, r: &Recipe) -> [f32; 3] {
    process_pixel(
        p,
        m,
        r,
        &CurveSet::new(r),
        profile_matrix(m, r),
        [0., 0.],
        None,
    )
}
fn fixture() -> CameraImage {
    let m = Metadata {
        width: 12,
        height: 8,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    CameraImage {
        recovered: Default::default(),
        width: 12,
        height: 8,
        pixels: (0..96)
            .map(|i| [0.02 + i as f32 / 200., 0.1 + i as f32 / 500., 0.04])
            .collect(),
        metadata: m,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }
}
#[test]
fn old_recipes_keep_original_profile_tones() {
    let mut json = serde_json::to_value(Recipe::default()).unwrap();
    for field in [
        "profile_tone",
        "camera_exposure",
        "wide_gamut_curves",
        "reference_curves",
        "reference_calibration",
        "reference_color",
    ] {
        json.as_object_mut().unwrap().remove(field);
    }
    let old: Recipe = serde_json::from_value(json).unwrap();
    assert!(!old.profile_tone);
    assert_eq!(old.camera_exposure, 0.);
    assert!(!old.wide_gamut_curves);
    assert!(!old.reference_color);
    assert!(!old.reference_curves);
    assert!(!old.reference_calibration);
    assert!(Recipe::default().profile_tone);
}

#[test]
fn switched_off_panels_render_as_if_at_their_defaults() -> anyhow::Result<()> {
    use crate::develop::panels::{Panel, PanelState};
    let im = fixture();
    let mut edited = Recipe::default();
    edited.effects.vignette = -0.8;
    edited.effects.grain = 0.6;
    edited.hsl[0] = [0.4, -0.6, 0.3];
    edited.exposure = 0.3;
    let mut off = edited.clone();
    off.panels.set(Panel::Effects, PanelState::Off);
    off.panels.set(Panel::ColorMixer, PanelState::Off);
    let plain = Recipe {
        exposure: 0.3,
        ..Default::default()
    };
    let pixels = |r: &Recipe| crate::develop::render(&im, r, 0).map(|out| out.pixels);
    assert_ne!(pixels(&edited)?, pixels(&plain)?);
    assert_eq!(pixels(&off)?, pixels(&plain)?);
    // Through the preview renderer too, which the editor and thumbnails use.
    let mut renderer = crate::develop::PreviewRenderer::with_processor(Err(anyhow::anyhow!("CPU")));
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let preview = renderer.render(&im, &off, 0, None, &cancel)?.pixels;
    assert_eq!(preview, pixels(&plain)?);
    Ok(())
}
#[test]
fn panel_switches_round_trip_and_old_recipes_have_every_panel_on() {
    use crate::develop::panels::{Panel, PanelState};
    let old: Recipe =
        serde_json::from_value(serde_json::to_value(Recipe::default()).unwrap()).unwrap();
    assert!(old.panels.all_on());
    assert!(!serde_json::to_string(&old).unwrap().contains("panels"));
    let mut r = Recipe::default();
    r.panels.set(Panel::Detail, PanelState::Off);
    let back: Recipe = serde_json::from_str(&serde_json::to_string(&r).unwrap()).unwrap();
    assert_eq!(back.panels.state(Panel::Detail), PanelState::Off);
    assert!(back.unknown.is_empty());
}
#[test]
fn reference_color_extremes_stay_finite_and_in_gamut() {
    let im = fixture();
    let mut r = Recipe {
        reference_color: true,
        wide_gamut_curves: true,
        ..Default::default()
    };
    for amount in [-1., 0., 1.] {
        r.saturation = amount;
        r.vibrance = amount;
        r.hsl = [[amount; 3]; 8];
        r.grading = [[0.62, 1., amount]; 3];
        r.effects.global_grade = [0.3, 1., amount];
        r.effects.balance = amount;
        r.effects.blending = (amount + 1.) * 0.5;
        for p in [
            [0.; 3],
            [1.; 3],
            [1., 0., 0.],
            [0., 0., 1.],
            [0.1, 0.5, 0.2],
        ] {
            let output = adjust(p, &im.metadata, &r);
            assert!(
                output
                    .iter()
                    .all(|v| v.is_finite() && (0. ..=1.).contains(v)),
                "{output:?}"
            );
        }
    }
}
#[test]
fn inactive_reference_color_preserves_the_baseline() {
    let im = fixture();
    let mut r = Recipe::default();
    for p in &im.pixels {
        r.reference_color = false;
        let old = adjust(*p, &im.metadata, &r);
        r.reference_color = true;
        let new = adjust(*p, &im.metadata, &r);
        assert!(old.iter().zip(new).all(|(a, b)| (a - b).abs() < 2e-6));
    }
}

#[test]
fn geometry_orientations_and_crop() {
    let im = fixture();
    let mut r = Recipe::default();
    for rotation in 0..4 {
        r.rotation = rotation;
        let g = Geometry::new(&im, &r, 0);
        let expected = if rotation % 2 == 0 { (12, 8) } else { (8, 12) };
        assert_eq!((g.width, g.height), expected);
        let p = g.source(0.5, 0.5);
        assert!((p[0] - 5.5).abs() < 1e-5 && (p[1] - 3.5).abs() < 1e-5);
    }
    r.rotation = 0;
    r.crop = [0.25, 0.25, 0.75, 0.75];
    let g = Geometry::new(&im, &r, 0);
    assert_eq!((g.width, g.height), (6, 4));
    assert_eq!(g.source(0., 0.), [2.5, 1.5]);
    r.crop = [0., 0., 1., 1.];
    r.straighten = 40.;
    let g = Geometry::new(&im, &r, 0);
    for u in [0., 1.] {
        for v in [0., 1.] {
            let [x, y] = g.source(u, v);
            assert!((-0.501..=11.501).contains(&x));
            assert!((-0.501..=7.501).contains(&y));
        }
    }
}
#[test]
fn point_curves_match_lightroom_ramp_references() {
    // Generated sRGB ramps, exported by Lightroom 15.5.1 as 16-bit TIFF.
    // See tests/data/README.md. These isolate curves from RAW/profile errors.
    let samples: Vec<[[u16; 3]; 5]> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/lightroom-point-curves.json"
    )))
    .unwrap();
    let curve = |points: &[[f32; 2]]| ToneCurve {
        points: points.iter().map(|p| p.map(|v| v / 255.)).collect(),
        ..Default::default()
    };
    let clipped = curve(&[[32., 16.], [96., 120.], [200., 224.]]);
    let mut recipes = [
        Recipe::default(),
        Recipe::default(),
        Recipe::default(),
        Recipe::default(),
    ];
    recipes[0].curve = curve(&[
        [0., 0.],
        [48., 24.],
        [128., 142.],
        [200., 224.],
        [255., 255.],
    ]);
    recipes[1].effects.channels[0] = curve(&[[0., 12.], [72., 52.], [170., 195.], [255., 244.]]);
    recipes[1].effects.channels[2] = curve(&[[0., 0.], [64., 86.], [192., 168.], [255., 255.]]);
    recipes[2].curve = clipped.clone();
    recipes[3].effects.channels[0] = clipped;
    recipes[3].effects.channels[2] = curve(&[[0., 0.], [64., 180.], [192., 64.], [255., 255.]]);
    for (case, recipe) in recipes.iter().enumerate() {
        let lut = CurveSet::new(recipe);
        let mut sum = 0.;
        let mut count = 0;
        let mut peak = 0f32;
        for (i, sample) in samples.iter().enumerate() {
            // Master curves still have a measured saturated-color residual;
            // only the neutral strip has the tight equivalence established here.
            if case % 2 == 0 && i >= 32 {
                continue;
            }
            let input = sample[0].map(|v| srgb_decode(v as f32 / 65535.));
            let actual = apply_reference_curves(input, recipe, &lut, None)
                .map(|v| srgb_encode(v).clamp(0., 1.));
            for (a, expected) in actual.into_iter().zip(sample[case + 1]) {
                let error = (a - expected as f32 / 65535.).abs();
                sum += error;
                peak = peak.max(error);
                count += 1;
            }
        }
        let mean = sum / count as f32;
        assert!(
            mean < 0.0001 && peak < 0.006,
            "case {case}: MAE {mean}, max {peak}"
        );
        if case % 2 == 0 {
            assert!(mean < 0.00003, "neutral MAE {mean}");
        }
    }
}

#[test]
fn viewport_matches_full_export_with_detail_and_geometry() -> Result<()> {
    let im = fixture();
    let mut r = Recipe {
        rotation: 1,
        straighten: 8.,
        noise_luma: 0.2,
        noise_chroma: 0.3,
        sharpening: 0.4,
        exposure: 0.5,
        camera_exposure: 0.15,
        wide_gamut_curves: true,
        reference_color: true,
        reference_curves: true,
        reference_calibration: true,
        vibrance: 0.6,
        grading: [[0.6, 0.3, 0.1]; 3],
        ..Default::default()
    };
    r.effects.calibration = [[0.2, -0.3], [-0.4, 0.5], [0.6, -0.7]];
    r.effects.shadow_tint = 0.3;
    r.curve.insert([0.4, 0.5]);
    r.effects.channels[0].insert([0.6, 0.7]);
    let full = render(&im, &r, 0)?;
    let tile = render_region(&im, &r, [1, 2, 4, 5])?;
    for y in 0..5 {
        for x in 0..4 {
            let a = full.pixels[(y + 2) * full.width as usize + x + 1];
            let b = tile.pixels[y * 4 + x];
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 1e-6, "{a:?} vs {b:?}");
            }
        }
    }
    Ok(())
}
#[test]
fn wb_at_estimated_as_shot_is_continuous() {
    let m = fixture().metadata;
    let mut r = Recipe::for_metadata(&m);
    r.update_wb(&m);
    for v in r.wb {
        assert!((v - 1.).abs() < 1e-6);
    }
    r.temperature += 1.;
    r.update_wb(&m);
    for v in r.wb {
        assert!((v - 1.).abs() < 0.01);
    }
}
#[test]
fn neutral_picker_balances_channels() {
    let mut im = fixture();
    im.pixels.fill([0.4, 0.2, 0.1]);
    let r = Recipe::default();
    assert_eq!(neutral_pick(&im, &r, 0.5, 0.5), [0.5, 1., 2.]);
}
#[test]
fn clipped_channels_do_not_make_magenta_highlights() {
    let mut m = fixture().metadata;
    m.wb = [2.5, 1., 1.4];
    m.matrix = [
        [2.0124, -0.9049, -0.1075],
        [-0.1196, 1.564, -0.4444],
        [0.0402, -0.4608, 1.4206],
    ];
    for exposure in [-3., 0., 2.] {
        let p = adjust(
            [2.5, 1., 1.4],
            &m,
            &Recipe {
                engine: 2,
                exposure,
                ..Default::default()
            },
        );
        assert!(
            (p[0] - p[1]).abs() < 0.0001 && (p[1] - p[2]).abs() < 0.0001,
            "{p:?}"
        );
    }
}
#[test]
fn exposure_reveals_retained_highlights() {
    let im = fixture();
    let a = adjust([2.; 3], &im.metadata, &Recipe::default());
    let b = adjust(
        [2.; 3],
        &im.metadata,
        &Recipe {
            exposure: -2.,
            ..Default::default()
        },
    );
    assert!(a[0] > b[0] && b[0] > 0.5);
    assert!(a.iter().all(|v| v.is_finite()));
}
#[test]
fn wide_contrast_preserves_endpoints_and_is_monotonic() {
    let mut recipe = Recipe {
        wide_gamut_curves: true,
        ..Default::default()
    };
    let lut = CurveSet::new(&recipe);
    for contrast in [-1., -0.4, 0., 0.2, 1.] {
        recipe.contrast = contrast;
        assert_eq!(apply_curve(0., 0, &recipe, &lut), 0.);
        assert_eq!(apply_curve(1., 0, &recipe, &lut), 1.);
        assert!((apply_curve(0.5, 0, &recipe, &lut) - 0.5).abs() < 1e-6);
        let mut previous = 0.;
        for i in 0..=1000 {
            let value = apply_curve(i as f32 / 1000., 0, &recipe, &lut);
            assert!(value >= previous && value <= 1.);
            previous = value;
        }
    }
}
#[test]
fn invalid_recipes_rejected() {
    let mut r = Recipe {
        exposure: f32::NAN,
        ..Default::default()
    };
    assert!(r.validate().is_err());
    r = Recipe::default();
    r.crop = [0.5, 0., 0.4, 1.];
    assert!(r.validate().is_err());
    r = Recipe::default();
    r.curve.points = vec![[0., 0.], [0.5, 0.8], [0.4, 0.5], [1., 1.]];
    assert!(r.validate().is_err());
}
#[test]
fn perceptual_roundtrip() {
    for p in [[0.1, 0.2, 0.4], [0.8, 0.2, 0.01], [-0.01, 0.3, 1.2]] {
        let q = lab_to_srgb(srgb_to_lab(p));
        for c in 0..3 {
            assert!((p[c] - q[c]).abs() < 1e-5);
        }
    }
}
#[test]
fn hue_bands_wrap_without_discontinuity() {
    let a = hue_weights(-0.00001);
    let b = hue_weights(0.00001);
    for c in 0..8 {
        assert!((a[c] - b[c]).abs() < 0.001);
    }
    for i in 0..1000 {
        assert!((hue_weights(i as f32 / 1000.).iter().sum::<f32>() - 1.).abs() < 1e-6);
    }
}
#[test]
fn monotonic_curve() {
    let k = ToneCurve {
        points: vec![[0., 0.], [0.25, 0.1], [0.5, 0.4], [0.75, 0.8], [1., 1.]],
        smooth: true,
        natural: false,
    };
    let mut prev = 0.;
    for i in 0..=1000 {
        let v = k.evaluate(i as f32 / 1000.);
        assert!(v >= prev);
        prev = v;
    }
}
#[test]
fn matrix_preserves_negative_and_headroom() {
    let p = mul([[2., -1., 0.], [0., 1., 0.], [0., 0., 1.]], [0.1, 1.3, 2.]);
    assert!(p[0] < 0. && p[2] > 1.);
}

#[test]
fn neutral_color_fast_path_matches_general_processing() {
    let metadata = Metadata {
        wb: [2., 1., 1.5],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    for engine in [2, 3] {
        for reference_color in [false, true] {
            let recipe = Recipe {
                engine,
                reference_color,
                exposure: 0.4,
                ..Recipe::default()
            };
            let fast = CurveSet::new(&recipe);
            let mut general = CurveSet::new(&recipe);
            general.color_adjustments = true;
            for i in 0..4096 {
                let p = [
                    (i % 16) as f32 / 8.,
                    ((i / 16) % 16) as f32 / 8.,
                    (i / 256) as f32 / 8.,
                ];
                let a = process_pixel(
                    p,
                    &metadata,
                    &recipe,
                    &fast,
                    metadata.matrix,
                    [0., 0.],
                    None,
                );
                let b = process_pixel(
                    p,
                    &metadata,
                    &recipe,
                    &general,
                    metadata.matrix,
                    [0., 0.],
                    None,
                );
                for c in 0..3 {
                    assert!(
                        (a[c] - b[c]).abs() < 2e-5,
                        "{engine}, {reference_color}, {p:?}: {a:?} vs {b:?}"
                    );
                }
            }
        }
    }
}
#[test]
fn builtin_lens_correction_brightens_corners_and_keeps_regions_consistent() {
    use crate::lens::{LensCorrection, Radial};
    let mut im = fixture();
    im.pixels = vec![[0.1; 3]; 96];
    im.metadata.lens = Some(LensCorrection {
        source: "test".into(),
        default_on: true,
        vignetting: Some(Radial {
            knots: vec![0., 1.],
            values: vec![1., 2.],
        }),
        distortion: Some(Radial {
            knots: vec![0., 1.],
            values: vec![1., 1.01],
        }),
        chromatic: None,
    });
    let on = Recipe::for_metadata(&im.metadata);
    assert!(on.lens_builtin);
    let off = Recipe {
        lens_builtin: false,
        ..on.clone()
    };
    let lum = |p: [f32; 3]| p.iter().sum::<f32>();
    let a = render_legacy(&im, &on, 0).unwrap();
    let b = render_legacy(&im, &off, 0).unwrap();
    let corner = |r: &Rendered| lum(r.pixels[0]);
    let centre = |r: &Rendered| lum(r.pixels[(4 * r.width + 6) as usize]);
    assert!(corner(&a) > corner(&b) + 0.01);
    assert!(corner(&a) > centre(&a) && (corner(&b) - centre(&b)).abs() < 1e-5);
    let region = render_region(&im, &on, [3, 2, 4, 3]).unwrap();
    let full = render(&im, &on, 0).unwrap();
    for y in 0..3 {
        for x in 0..4 {
            let p = region.pixels[(y * 4 + x) as usize];
            let q = full.pixels[((y + 2) * full.width + x + 3) as usize];
            assert!((0..3).all(|c| (p[c] - q[c]).abs() < 2e-6));
        }
    }
    // Older engines never apply the correction.
    let old = Recipe { engine: 3, ..on };
    assert!(old.lens_correction(&im.metadata).is_none());
}
#[test]
fn transform_scales_and_fills_uncovered_area_with_white() {
    let im = fixture();
    let mut r = Recipe::for_metadata(&im.metadata);
    let plain = render(&im, &r, 0).unwrap();
    r.transform.scale = 0.5;
    let small = render(&im, &r, 0).unwrap();
    assert_eq!((small.width, small.height), (plain.width, plain.height));
    // Halving the scale leaves the corners uncovered and keeps the centre.
    assert_eq!(small.pixels[0], [1.; 3]);
    let centre = |x: &Rendered| x.pixels[(4 * x.width + 6) as usize];
    assert!((0..3).all(|c| (centre(&small)[c] - centre(&plain)[c]).abs() < 0.05));
    r.transform = crate::develop::Transform {
        vertical: 0.6,
        horizontal: -0.3,
        rotate: 4.,
        aspect: 0.2,
        offset_x: 0.1,
        ..Default::default()
    };
    assert!(r.validate().is_ok());
    let full = render(&im, &r, 0).unwrap();
    let region = render_region(&im, &r, [2, 1, 5, 4]).unwrap();
    for y in 0..4 {
        for x in 0..5 {
            let p = region.pixels[(y * 5 + x) as usize];
            let q = full.pixels[((y + 1) * full.width + x + 2) as usize];
            assert!((0..3).all(|c| (p[c] - q[c]).abs() < 2e-6));
        }
    }
    r.transform.scale = 2.;
    assert!(r.validate().is_err());
}
#[test]
fn fringe_selector_reaches_the_hue_defringe_tests_through_grading() {
    // A purple fringe as Defringe sees it, shown greener by legacy colour grading.
    let hue = 0.85f32;
    let angle = hue * std::f32::consts::TAU;
    let lab = [0.6, angle.cos() * 0.08, angle.sin() * 0.08];
    let mut r = Recipe {
        engine: 3,
        ..Default::default()
    };
    r.effects.global_grade = [0.4, 0.4, 0.];
    let lut = CurveSet::new(&r);
    let shown = finish_color(lab, &r, &lut);
    let chroma = |lab: [f32; 3]| lab[1].hypot(lab[2]);
    assert_eq!(pick_fringe(&mut r, &Metadata::default(), shown), Some(0));
    assert!(chroma(r.effects.defringe_color(lab, hue)) < chroma(lab) * 0.6);
}
#[test]
fn fringe_selector_reaches_the_hue_defringe_tests_through_channel_curves() {
    // A green fringe as Defringe sees it, darkened and turned by a lowered legacy green
    // channel curve.
    let hue = 0.45f32;
    let angle = hue * std::f32::consts::TAU;
    let lab = [0.6, angle.cos() * 0.08, angle.sin() * 0.08];
    let mut r = Recipe {
        engine: 3,
        reference_curves: false,
        wide_gamut_curves: false,
        ..Default::default()
    };
    r.effects.channels[1] = ToneCurve {
        points: vec![[0., 0.], [1., 0.6]],
        ..Default::default()
    };
    let lut = CurveSet::new(&r);
    let shown = finish_color(lab, &r, &lut);
    let chroma = |lab: [f32; 3]| lab[1].hypot(lab[2]);
    assert_eq!(pick_fringe(&mut r, &Metadata::default(), shown), Some(1));
    assert!(chroma(r.effects.defringe_color(lab, hue)) < chroma(lab) * 0.6);
}

/// Refine Saturation 0 keeps each colour's channel spread (encoded ProPhoto, before the
/// point curve) and takes its luma from the curved colour; other amounts blend, and
/// amounts above 1 render as 1 (Camera Raw 18.7 on the synthetic chart).
#[test]
fn refine_saturation_zero_keeps_the_colours_saturation_through_the_point_curve() {
    let mut recipe = Recipe {
        reference_curves: true,
        ..Default::default()
    };
    recipe.curve = ToneCurve {
        points: vec![[0., 0.], [0.25, 0.1], [0.75, 0.9], [1., 1.]],
        ..Default::default()
    };
    let pro = |rgb: [f32; 3]| mul(crate::camera_profiles::RGB_TO_PRO, rgb).map(srgb_encode);
    let spread = |p: [f32; 3]| {
        p.iter().copied().fold(f32::MIN, f32::max) - p.iter().copied().fold(f32::MAX, f32::min)
    };
    let luma = |p: [f32; 3]| 0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2];
    let input = [0.25, 0.08, 0.04];
    let render = |amount: f32| {
        let mut r = recipe.clone();
        r.effects.curve_saturation = amount;
        pro(apply_reference_curves(input, &r, &CurveSet::new(&r), None))
    };
    let (full, none, half) = (render(1.), render(0.), render(0.5));
    assert!((spread(none) - spread(pro(input))).abs() < 1e-3, "{none:?}");
    assert!(spread(full) > spread(none) + 0.05, "{full:?} {none:?}");
    assert!((luma(none) - luma(full)).abs() < 1e-3);
    for c in 0..3 {
        assert!(
            (half[c] - (full[c] + none[c]) / 2.).abs() < 1e-3,
            "{half:?}"
        );
    }
    assert_eq!(render(2.), full);
}
