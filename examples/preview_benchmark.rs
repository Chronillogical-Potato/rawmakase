//! Read-only preview timing harness: cargo run --release --example preview_benchmark -- PHOTO [ITERATIONS]
//!
//! Times what the desktop does: the first Fit after opening, Fit renders while a
//! slider moves, a 100% region, and the full-resolution render used for export.
use anyhow::{Context, Result};
use rawmakase::{
    develop::{self, Recipe},
    raw,
};
use std::{sync::atomic::AtomicBool, time::Instant};

/// Fit size of a 1600-pixel viewport, the size used by earlier measurements.
const FIT: u32 = 1600;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.
}
fn mean_error(a: &develop::Rendered, b: &develop::Rendered) -> f32 {
    assert_eq!((a.width, a.height), (b.width, b.height));
    let d: f32 = a
        .pixels
        .iter()
        .flatten()
        .zip(b.pixels.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .sum();
    d / (a.pixels.len() * 3) as f32
}

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
        ms(start)
    );
    let (profiles, _) = rawmakase::camera_profiles::installed(&image.metadata);
    let recipe = Recipe::with_profiles(&image.metadata, &profiles);
    println!(
        "Camera: {} {}; profile: {:?}",
        image.metadata.make,
        image.metadata.model,
        recipe.profile.as_ref().map(|p| &p.name)
    );
    let mut gpu = develop::PreviewRenderer::with_gpu();
    println!(
        "GPU: {:?}; fallback: {:?}",
        gpu.adapter_name(),
        gpu.fallback_reason()
    );
    let mut cpu = develop::PreviewRenderer::default();
    let image = std::sync::Arc::new(image);
    // First Fit after opening: highlight recovery and any per-photo preparation.
    let t = Instant::now();
    gpu.render(&image, &recipe, FIT, None, &cancel)?;
    println!("First Fit after open: {:.1} ms", ms(t));
    let g = develop::Geometry::new(&image, &recipe, 0);
    let (rw, rh) = (1600.min(g.width), 1000.min(g.height));
    let region = [(g.width - rw) / 2, (g.height - rh) / 2, rw, rh];
    for local in [false, true] {
        let mut recipe = recipe.clone();
        if local {
            recipe.shadows = 0.4;
            recipe.highlights = -0.3;
            recipe.effects.clarity = 0.2;
        }
        recipe.exposure = iterations as f32 * 0.1;
        let t = Instant::now();
        let full = develop::quality::render_cancellable(&image, &recipe, 0, None, &cancel)?;
        println!("Export resolution, local={local}: {:.1} ms", ms(t));
        let reference = develop::quality::resize(full, FIT);
        let modes: &[&str] = if local {
            &[
                "cpu-fit",
                "gpu-fit",
                "gpu-region",
                "clarity-fit",
                "clarity-region",
            ]
        } else {
            &["cpu-fit", "gpu-fit", "gpu-region"]
        };
        for &mode in modes {
            let mut times = Vec::new();
            let mut last = None;
            for i in 0..=iterations {
                // Exposure edits, or Clarity edits, which change the local-tone stage.
                if mode.starts_with("clarity") {
                    recipe.effects.clarity = 0.2 + (iterations - i) as f32 * 0.05;
                } else {
                    recipe.exposure = i as f32 * 0.1;
                }
                let t = Instant::now();
                let out = match mode {
                    "cpu-fit" => cpu.render(&image, &recipe, FIT, None, &cancel)?,
                    "gpu-fit" | "clarity-fit" => gpu.render(&image, &recipe, FIT, None, &cancel)?,
                    _ => gpu.render(&image, &recipe, 0, Some(region), &cancel)?,
                };
                if i > 0 {
                    times.push(ms(t));
                }
                last = Some(out);
            }
            times.sort_by(f64::total_cmp);
            let last = last.unwrap();
            let error = if mode.ends_with("region") {
                String::new()
            } else {
                let e = mean_error(&last, &reference);
                anyhow::ensure!(e < 0.01, "{mode} differs from the export render: {e}");
                format!(", mean error vs export {e:.5}")
            };
            println!(
                "{mode}, local={local}: median {:.1} ms, max {:.1} ms{error}",
                times[times.len() / 2],
                times[times.len() - 1]
            );
        }
    }
    Ok(())
}
