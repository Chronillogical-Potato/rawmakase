//! Read-only preview timing harness: cargo run --release --example preview_benchmark -- PHOTO [ITERATIONS]
use anyhow::{Context, Result};
use rawmakase::{
    develop::{self, Recipe},
    raw,
};
use std::{sync::atomic::AtomicBool, time::Instant};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let path = args.get(1).context("Supply a RAW path")?;
    let iterations: usize = args.get(2).map_or(Ok(3), |n| n.parse())?;
    anyhow::ensure!((1..=100).contains(&iterations), "Use 1–100 iterations");
    let cancel = AtomicBool::new(false);
    let start = Instant::now();
    let image = raw::Raw::open(std::path::Path::new(path))?.develop(false, &cancel)?;
    println!(
        "Decode {}x{}: {:.1} ms",
        image.width,
        image.height,
        start.elapsed().as_secs_f64() * 1000.
    );
    let (profiles, _) = rawmakase::camera_profiles::installed(&image.metadata);
    let recipe = Recipe::with_profiles(&image.metadata, &profiles);
    println!(
        "Camera: {} {}; profile: {:?}",
        image.metadata.make,
        image.metadata.model,
        recipe.profile.as_ref().map(|p| &p.name)
    );
    let draft = develop::preview(&image, 1024);
    let mut gpu = develop::PreviewRenderer::with_gpu();
    println!(
        "GPU: {:?}; fallback: {:?}",
        gpu.adapter_name(),
        gpu.fallback_reason()
    );
    for local in [false, true] {
        let mut recipe = recipe.clone();
        if local {
            recipe.shadows = 0.4;
            recipe.highlights = -0.3;
            recipe.effects.clarity = 0.2;
        }
        let mut reference: Option<develop::Rendered> = None;
        for mode in ["draft", "fit", "gpu-fit"] {
            let mut times = Vec::new();
            for i in 0..=iterations {
                recipe.exposure = i as f32 * 0.1;
                let t = Instant::now();
                let out = if mode == "draft" {
                    let mut r = recipe.clone();
                    r.sharpening = 0.;
                    r.noise_luma = 0.;
                    r.noise_chroma = 0.;
                    let recovered = draft.recovered.get_or_init(|| {
                        std::sync::Arc::new(develop::quality::recover_highlights(&draft))
                    });
                    develop::render_legacy(recovered, &r, 1024)?
                } else if mode == "gpu-fit" {
                    gpu.render(&image, &recipe, 1600, None, &cancel)?
                } else {
                    develop::quality::render_cancellable(&image, &recipe, 1600, None, &cancel)?
                };
                let elapsed = t.elapsed().as_secs_f64() * 1000.;
                if i > 0 {
                    times.push(elapsed);
                }
                if mode == "gpu-fit" {
                    anyhow::ensure!(gpu.used_gpu(), "GPU fallback: {:?}", gpu.fallback_reason());
                    if i == iterations {
                        let reference = reference.as_ref().unwrap();
                        // The desktop Fit renders from a reduced camera image, so it is
                        // close to, not identical with, the full-resolution reference.
                        let d: Vec<f32> = out
                            .pixels
                            .iter()
                            .flatten()
                            .zip(reference.pixels.iter().flatten())
                            .map(|(a, b)| (a - b).abs())
                            .collect();
                        let mean = d.iter().sum::<f32>() / d.len() as f32;
                        println!(
                            "Desktop Fit vs full-resolution reference: mean channel error {mean:.5}"
                        );
                        anyhow::ensure!(mean < 0.01, "Desktop Fit differs from reference");
                    }
                }
                if mode == "fit" && i == iterations {
                    reference = Some(out);
                } else {
                    std::hint::black_box(out);
                }
            }
            times.sort_by(f64::total_cmp);
            println!(
                "{mode}, local={local}: median {:.1} ms, max {:.1} ms",
                times[times.len() / 2],
                times[times.len() - 1]
            );
        }
    }
    Ok(())
}
