use super::{Event, Latest, RenderJob, RenderStage, TaskKind, send};
use crate::{develop, raw};
use eframe::egui;
use std::{
    sync::{atomic::Ordering, mpsc::Sender},
    time::Instant,
};
#[derive(Clone, Copy)]
pub(in crate::app) enum RenderBackend {
    Cpu,
    Gpu,
}

/// CPU-only compatibility entry point, also suitable for headless UI tests.
pub fn renderer(tx: Sender<Event>, ctx: egui::Context) -> Latest<RenderJob> {
    renderer_with_backend(tx, ctx, RenderBackend::Cpu)
}
pub(in crate::app) fn renderer_with_backend(
    tx: Sender<Event>,
    ctx: egui::Context,
    backend: RenderBackend,
) -> Latest<RenderJob> {
    let mut processor = None;
    Latest::new(move |job: RenderJob| {
        let processor = processor.get_or_insert_with(|| match backend {
            RenderBackend::Cpu => develop::PreviewRenderer::default(),
            RenderBackend::Gpu => develop::PreviewRenderer::with_gpu(),
        });
        let t = Instant::now();
        let publish =
            |out: develop::Rendered, stage: RenderStage, gpu: bool| -> anyhow::Result<()> {
                if job.cancel.load(Ordering::Relaxed) {
                    return Ok(());
                }
                let mut rgb = out.rgb8();
                let mut warning = String::new();
                if let Some(p) = &job.monitor
                    && let Err(e) = raw::display_transform(p, &mut rgb)
                {
                    warning = format!(" • ICC failed: {e}");
                }
                if job.clipping {
                    for (p, orig) in rgb.as_chunks_mut::<3>().0.iter_mut().zip(&out.pixels) {
                        if orig.iter().any(|v| *v >= 0.999) {
                            p.copy_from_slice(&[255, 40, 40]);
                        } else if orig.iter().all(|v| *v <= 0.001) {
                            p.copy_from_slice(&[40, 80, 255]);
                        }
                    }
                }
                send(
                    &tx,
                    &ctx,
                    Event::Rendered {
                        id: job.id,
                        image: out,
                        display_rgb: rgb,
                        stage,
                        status: format!(
                            "{} • {} • {:.0} ms{warning}",
                            stage.label(),
                            if gpu { "GPU finish" } else { "CPU" },
                            t.elapsed().as_secs_f64() * 1000.
                        ),
                    },
                );
                Ok(())
            };
        let result = (|| -> anyhow::Result<()> {
            if job.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            if let Some(region) = job.region {
                if job.recipe.engine < 3 {
                    let out = develop::render_region(&job.image, &job.recipe, region)?;
                    return publish(out, RenderStage::Region, false);
                }
                // A reduced preview first keeps dragging at 100% responsive; a newer
                // job cancels the full-resolution render that follows.
                if let Some(out) =
                    processor.render_region_preview(&job.image, &job.recipe, region, &job.cancel)?
                {
                    publish(out, RenderStage::Draft, false)?;
                }
                let out =
                    processor.render(&job.image, &job.recipe, 0, Some(region), &job.cancel)?;
                return publish(out, RenderStage::Region, processor.used_gpu());
            }
            // Fit renders from the photo's resolution pyramid, fast enough to follow
            // a slider without a separate draft.
            let out = processor.render(&job.image, &job.recipe, job.max_edge, None, &job.cancel)?;
            publish(out, RenderStage::Fit, processor.used_gpu())
        })();
        if let Err(e) = result
            && !job.cancel.load(Ordering::Relaxed)
        {
            send(
                &tx,
                &ctx,
                Event::Failed {
                    id: job.id,
                    task: TaskKind::Render,
                    error: e.to_string(),
                },
            );
        }
    })
}
