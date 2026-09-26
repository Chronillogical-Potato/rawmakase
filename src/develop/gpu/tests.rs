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
            let expected =
                super::super::quality::render_cancellable(&image, &recipe, 70, region, &cancel)?;
            let actual = renderer.render(&image, &recipe, 70, region, &cancel)?;
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
    let expected = super::super::quality::render(&image, &recipe, 50, None)?;
    let actual = renderer.render(&image, &recipe, 50, None, &cancel)?;
    assert_eq!(actual.pixels, expected.pixels);
    assert!(!renderer.used_gpu());
    Ok(())
}
