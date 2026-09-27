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

/// A finished render and the view it shows.
struct Shown {
    image: std::sync::Arc<crate::raw::CameraImage>,
    recipe: develop::Recipe,
    max_edge: u32,
    region: Option<[u32; 4]>,
    out: develop::Rendered,
}
impl Shown {
    fn new(job: &RenderJob, out: develop::Rendered) -> Self {
        Self {
            image: job.image.clone(),
            recipe: job.recipe.clone(),
            max_edge: job.max_edge,
            region: job.region,
            out,
        }
    }
    /// The same photo, view and edit.
    fn matches(&self, job: &RenderJob) -> bool {
        std::sync::Arc::ptr_eq(&self.image, &job.image)
            && self.max_edge == job.max_edge
            && self.region == job.region
            && self.recipe == job.recipe
    }
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
    // The last finished Fit and 100% region, so switching back to a view with the same
    // edit shows its sharp image at once instead of rendering it again.
    let (mut fit, mut zoomed): (Option<Shown>, Option<Shown>) = (None, None);
    // Whether the last finished image was a 100% region: then the region is being
    // edited or panned, and a reduced preview comes first.
    let mut showing_region = false;
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
            let cached = if job.region.is_some() { &zoomed } else { &fit };
            if let Some(shown) = cached.as_ref().filter(|s| s.matches(&job)) {
                let stage = if job.region.is_some() {
                    RenderStage::Region
                } else {
                    RenderStage::Fit
                };
                publish(shown.out.clone(), stage, false)?;
                showing_region = job.region.is_some();
                return Ok(());
            }
            if let Some(region) = job.region {
                let out = if job.recipe.engine < 3 {
                    develop::render_region(&job.image, &job.recipe, region)?
                } else {
                    // While editing at 100%, a reduced preview keeps sliders responsive;
                    // a newer job cancels the full-resolution render that follows.
                    // Zooming in goes straight to the full region, over the enlarged Fit.
                    if showing_region
                        && let Some(out) = processor.render_region_preview(
                            &job.image,
                            &job.recipe,
                            region,
                            &job.cancel,
                        )?
                    {
                        publish(out, RenderStage::Draft, false)?;
                    }
                    processor.render(&job.image, &job.recipe, 0, Some(region), &job.cancel)?
                };
                publish(out.clone(), RenderStage::Region, processor.used_gpu())?;
                zoomed = Some(Shown::new(&job, out));
                showing_region = true;
                return Ok(());
            }
            let out = processor.render(&job.image, &job.recipe, job.max_edge, None, &job.cancel)?;
            publish(out.clone(), RenderStage::Fit, processor.used_gpu())?;
            fit = Some(Shown::new(&job, out));
            showing_region = false;
            Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raw::{CameraImage, Metadata};
    use std::sync::{Arc, atomic::AtomicBool};
    fn image() -> Arc<CameraImage> {
        let (w, h) = (240, 160);
        Arc::new(CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels: (0..w * h)
                .map(|i| {
                    let v = 0.2 + 0.1 * ((i % w) as f32 * 0.2).sin();
                    [v * 1.1, v, v * 0.8]
                })
                .collect(),
            metadata: Metadata {
                width: w,
                height: h,
                wb: [1.; 3],
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        })
    }
    /// Renders one job and returns its published stages and final pixels.
    fn run(
        worker: &Latest<RenderJob>,
        rx: &std::sync::mpsc::Receiver<Event>,
        id: u64,
        image: &Arc<CameraImage>,
        recipe: &develop::Recipe,
        region: Option<[u32; 4]>,
    ) -> Vec<(RenderStage, Vec<[f32; 3]>)> {
        worker.submit(RenderJob {
            id,
            image: image.clone(),
            max_edge: 60,
            cancel: Arc::new(AtomicBool::new(false)),
            recipe: recipe.clone(),
            region,
            monitor: None,
            clipping: false,
        });
        let mut stages = Vec::new();
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap() {
                Event::Rendered {
                    image,
                    stage,
                    id: i,
                    ..
                } if i == id => {
                    stages.push((stage, image.pixels));
                    if stage != RenderStage::Draft {
                        return stages;
                    }
                }
                Event::Failed { error, .. } => panic!("{error}"),
                _ => {}
            }
        }
    }
    #[test]
    fn switching_views_shows_the_previous_images_without_drafts() {
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = renderer(tx, egui::Context::default());
        let image = image();
        let mut recipe = develop::Recipe::default();
        let region = Some([10, 10, 80, 60]);
        let fit = run(&worker, &rx, 1, &image, &recipe, None);
        assert_eq!(fit.len(), 1);
        // Zooming in renders the full region directly.
        let zoomed = run(&worker, &rx, 2, &image, &recipe, region);
        assert_eq!(zoomed.len(), 1);
        // Back to Fit and 100% again: the same images, no drafts.
        assert_eq!(run(&worker, &rx, 3, &image, &recipe, None), fit);
        assert_eq!(run(&worker, &rx, 4, &image, &recipe, region), zoomed);
        // Editing at 100% shows a reduced preview first.
        recipe.exposure = 0.5;
        let edited = run(&worker, &rx, 5, &image, &recipe, region);
        assert_eq!(edited.len(), 2);
        assert_eq!(edited[0].0, RenderStage::Draft);
        // Back to Fit after the edit: the new Fit, without drafts; the viewport keeps
        // showing its previous Fit under the region until then.
        let back = run(&worker, &rx, 6, &image, &recipe, None);
        assert_eq!(back.len(), 1);
        assert_ne!(back[0].1, fit[0].1);
    }
}
