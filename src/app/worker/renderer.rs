use super::{Event, Latest, RenderJob, RenderStage, TaskKind, send};
use crate::{develop, raw};
use eframe::egui;
use std::{
    sync::{Arc, atomic::Ordering, mpsc::Sender},
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
                return publish(
                    if job.recipe.engine >= 3 {
                        processor.render(&job.image, &job.recipe, 0, Some(region), &job.cancel)?
                    } else {
                        develop::render_region(&job.image, &job.recipe, region)?
                    },
                    RenderStage::Region,
                    job.recipe.engine >= 3 && processor.used_gpu(),
                );
            }
            let mut draft = job.recipe.clone();
            draft.sharpening = 0.;
            draft.noise_luma = 0.;
            draft.noise_chroma = 0.;
            // Small camera-space image is only an explicitly labeled interactive draft.
            let draft_image = if draft.engine >= 3 {
                job.draft
                    .recovered
                    .get_or_init(|| {
                        Arc::new(crate::develop::quality::recover_highlights(&job.draft))
                    })
                    .as_ref()
            } else {
                job.draft.as_ref()
            };
            let mut draft_out = develop::render_legacy(draft_image, &draft, super::DRAFT_EDGE)?;
            let draft_size = [draft_out.width, draft_out.height];
            crate::develop::effects::spatial_finish(&mut draft_out, &draft, [0, 0], draft_size);
            publish(draft_out, RenderStage::Draft, false)?;
            for _ in 0..15 {
                if job.cancel.load(Ordering::Relaxed) {
                    return Ok(());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let out = if job.recipe.engine >= 3 {
                processor.render(&job.image, &job.recipe, job.max_edge, None, &job.cancel)?
            } else {
                develop::render(&job.image, &job.recipe, job.max_edge)?
            };
            publish(
                out,
                RenderStage::Fit,
                job.recipe.engine >= 3 && processor.used_gpu(),
            )
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
