use super::*;

#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
fn gpu_matches_cpu_finishing_and_reuses_buffers() -> Result<()> {
    let mut gpu = Processor::new()?;
    eprintln!("GPU: {}", gpu.name());
    let cancel = AtomicBool::new(false);
    for (w, h) in [(97, 63), (1, 33), (33, 1), (1, 1)] {
        let image = Rendered {
            width: w,
            height: h,
            pixels: (0..w * h)
                .map(|i| {
                    let x = (i % w) as f32 / w as f32;
                    let y = (i / w) as f32 / h as f32;
                    [x, y, if i % 7 < 3 { 0.03 } else { 0.96 }]
                })
                .collect(),
        };
        for amount in [0., 0.7] {
            for max_edge in [0, 1, 17, 90] {
                for radius in [0.5, 3.] {
                    let recipe = Recipe {
                        sharpening: amount,
                        sharpening_radius: radius,
                        sharpening_masking: 0.6,
                        sharpening_detail: 0.8,
                        ..Recipe::default()
                    };
                    let mut expected = image.clone();
                    crate::develop::quality::sharpen(&mut expected, &recipe);
                    let expected = crate::develop::quality::resize(expected, max_edge);
                    for _ in 0..2 {
                        let actual = gpu.finish(&image, &recipe, max_edge, &cancel)?;
                        assert_eq!(
                            (actual.width, actual.height),
                            (expected.width, expected.height)
                        );
                        let error = actual
                            .pixels
                            .iter()
                            .flatten()
                            .zip(expected.pixels.iter().flatten())
                            .map(|(a, b)| (a - b).abs())
                            .fold(0f32, f32::max);
                        assert!(
                            error < 2e-5,
                            "{w}x{h}, edge={max_edge}, amount={amount}, radius={radius}: {error}"
                        );
                    }
                }
            }
        }
    }
    cancel.store(true, Ordering::Relaxed);
    let image = Rendered {
        width: 1,
        height: 1,
        pixels: vec![[0.5; 3]],
    };
    assert!(gpu.finish(&image, &Recipe::default(), 1, &cancel).is_err());
    Ok(())
}

#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
fn gpu_preview_preserves_regions_spatial_effects_and_falls_back() -> Result<()> {
    use crate::{
        develop::PreviewRenderer,
        raw::{CameraImage, Metadata},
    };
    let image = CameraImage {
        width: 137,
        height: 91,
        pixels: (0..137 * 91)
            .map(|i| {
                let x = (i % 137) as f32 / 137.;
                let y = (i / 137) as f32 / 91.;
                [x * 1.2, y * 0.8, 0.2 + x * y]
            })
            .collect(),
        metadata: Metadata {
            width: 137,
            height: 91,
            wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        recovered: Default::default(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    };
    let mut renderer = PreviewRenderer::with_gpu();
    assert!(
        renderer.adapter_name().is_some(),
        "{:?}",
        renderer.fallback_reason()
    );
    let cancel = AtomicBool::new(false);
    for spatial in [false, true] {
        for region in [None, Some([3, 2, 29, 41])] {
            let mut recipe = Recipe {
                exposure: 0.3,
                shadows: 0.2,
                sharpening_radius: 3.,
                rotation: 1,
                crop: [0.1, 0.05, 0.9, 0.95],
                ..Default::default()
            };
            if spatial {
                recipe.effects.grain = 0.4;
                recipe.effects.vignette = -0.3;
            }
            // Reduced Fit sizes render from the pyramid on the CPU; full size and
            // regions use GPU finishing.
            let expected =
                super::super::quality::render_cancellable(&image, &recipe, 0, region, &cancel)?;
            let actual = renderer.render(&image, &recipe, 0, region, &cancel)?;
            assert_eq!(renderer.used_gpu(), !spatial || region.is_none());
            assert_eq!(
                (actual.width, actual.height),
                (expected.width, expected.height)
            );
            let error = actual
                .pixels
                .iter()
                .flatten()
                .zip(expected.pixels.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0f32, f32::max);
            assert!(
                error < 2e-5,
                "spatial={spatial}, region={region:?}, max error={error}"
            );
        }
    }
    // A failed backend is disabled rather than retried on every interaction.
    let invalid = Rendered {
        width: 2,
        height: 1,
        pixels: vec![[0.5; 3]],
    };
    assert!(
        renderer
            .finish(&invalid, &Recipe::default(), 0, &cancel)
            .is_none()
    );
    assert!(renderer.adapter_name().is_none());
    assert!(renderer.fallback_reason().is_some());
    let recipe = Recipe::default();
    let expected = super::super::quality::render(&image, &recipe, 0, None)?;
    let actual = renderer.render(&image, &recipe, 0, None, &cancel)?;
    assert_eq!(actual.pixels, expected.pixels);
    assert!(!renderer.used_gpu());
    Ok(())
}

/// The GPU per-pixel stage against the CPU reference, over recipes that exercise every
/// table and branch of `develop.wgsl`.
#[test]
#[ignore = "Requires a hardware compute adapter; run explicitly on supported machines"]
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients.
fn gpu_develop_matches_cpu_pixel_stage() -> Result<()> {
    use crate::{
        camera_profiles::CameraProfile,
        develop::pipeline::{Samples, Source, develop_samples, pixel_params::pixel_params},
        raw::{CameraImage, Metadata},
    };
    use std::sync::Arc;
    let metadata = Metadata {
        make: "Fujifilm".into(),
        model: "X100F".into(),
        width: 64,
        height: 48,
        wb: [2.02, 1., 1.89],
        cam_xyz: [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.5235],
        ],
        ..Default::default()
    };
    let wave = |i: usize, k: f32| ((i as f32 * k).sin() * 0.5 + 0.5).powi(2);
    let image = Arc::new(CameraImage {
        width: 64,
        height: 48,
        pixels: (0..64 * 48)
            .map(|i| [wave(i, 0.37) * 1.3, wave(i, 0.21), wave(i, 0.13) * 1.1])
            .collect(),
        metadata: metadata.clone(),
        recovered: Default::default(),
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    let n = 4000;
    let samples = Arc::new(Samples {
        width: 80,
        height: 50,
        // Dark, mid, bright and out-of-range camera values in all hue directions.
        pixels: (0..n)
            .map(|i| {
                [
                    wave(i, 0.71) * 1.6,
                    wave(i, 0.53) * 1.2,
                    wave(i, 0.29) * 1.5,
                ]
            })
            .collect(),
        positions: (0..n)
            .map(|i| {
                if i % 97 == 0 {
                    [f32::NAN; 2]
                } else {
                    [(i % 64) as f32, (i / 64 % 48) as f32]
                }
            })
            .collect(),
    });
    let plain = CameraProfile::camera_matrix_default(&metadata).unwrap();
    let tables = plain.clone().with_test_tables();
    let base = |profile: &CameraProfile| Recipe {
        profile: Some(Arc::new(profile.clone())),
        reference_curves: true,
        reference_color: true,
        reference_calibration: true,
        temperature: 5000.,
        ..Default::default()
    };
    let mut recipes = vec![base(&plain), base(&tables)];
    let mut r = base(&tables);
    r.exposure = 0.7;
    r.contrast = 0.4;
    r.whites = -0.3;
    r.blacks = 0.2;
    r.effects.dehaze = 0.25;
    r.curve.insert([0.3, 0.25]);
    r.effects.channels[2].insert([0.6, 0.7]);
    r.effects.parametric = [0.2, -0.1, 0.3, 0.];
    r.black_point = 0.02;
    r.white_point = 0.97;
    r.midtone = 1.2;
    recipes.push(r.clone());
    r.shadows = 0.5;
    r.highlights = -0.6;
    recipes.push(r.clone());
    r.hsl[1] = [0.3, -0.5, 0.4];
    r.hsl[5] = [-0.2, 0.6, -0.3];
    r.saturation = 0.2;
    r.vibrance = -0.3;
    r.grading[0] = [0.6, 0.4, -0.2];
    r.effects.global_grade = [0.1, 0.2, 0.1];
    r.effects.calibration = [[0.3, -0.2], [-0.4, 0.5], [0.2, 0.1]];
    r.effects.shadow_tint = -0.4;
    recipes.push(r.clone());
    r.effects.defringe = [0.5, 0.3];
    recipes.push(r.clone());
    r.effects.monochrome = true;
    r.effects.gray_mix = [0.2, -0.3, 0.1, 0.4, -0.2, 0.3, 0., -0.1];
    recipes.push(r);
    let mut gpu = Processor::new()?;
    let cancel = AtomicBool::new(false);
    // A local-tone gain changes the Shadows/Highlights map's input.
    let gain: Vec<f32> = (0..64 * 48).map(|i| 0.6 + wave(i, 0.05)).collect();
    for (i, recipe) in recipes.iter().enumerate() {
        let source = Source::new(&image, (i % 2 == 1).then_some(gain.as_slice()));
        let params = pixel_params(source, recipe).expect("GPU port covers this recipe");
        let expected = develop_samples(source, recipe, &samples, &cancel)?;
        let actual = gpu.develop(&samples, &params, &cancel)?;
        let d: Vec<f32> = actual
            .pixels
            .iter()
            .flatten()
            .zip(expected.pixels.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .collect();
        let mean = d.iter().sum::<f32>() / d.len() as f32;
        let mut sorted = d.clone();
        sorted.sort_by(f32::total_cmp);
        let (p999, max) = (sorted[sorted.len() * 999 / 1000], sorted[sorted.len() - 1]);
        eprintln!("recipe {i}: max {max:.6}, 99.9% {p999:.6}, mean {mean:.8}");
        // Near-neutral pixels have an unstable Oklab hue angle, which can move them to
        // another Monochrome band; everything else agrees to float precision.
        assert!(
            p999 < 1e-3 && max < 0.02 && mean < 2e-5,
            "recipe {i}: max {max}, 99.9% {p999}, mean {mean}"
        );
    }
    // Older operators stay on the CPU.
    let mut legacy = base(&tables);
    legacy.effects.balance = 0.3;
    legacy.grading[0] = [0.6, 0.4, 0.];
    assert!(pixel_params(image.as_ref().into(), &legacy).is_none());
    legacy = base(&tables);
    legacy.engine = 3;
    assert!(pixel_params(image.as_ref().into(), &legacy).is_none());
    Ok(())
}
