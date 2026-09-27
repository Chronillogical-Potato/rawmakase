use super::{Event, Latest, Preview, RenderJob, RenderStage, TaskKind, send};
use crate::{
    develop::{self, gpu, quality::Output},
    raw,
};
use eframe::{egui, egui_wgpu};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, atomic::Ordering, mpsc::Sender},
    time::Instant,
};
#[derive(Clone)]
pub(in crate::app) enum RenderBackend {
    Cpu,
    /// The GPU; with the UI's render state, previews are presented into textures
    /// the viewport draws, and otherwise read back from a device of their own.
    Gpu(Option<egui_wgpu::RenderState>),
}
/// Long edges of the Navigator's and the library thumbnail's copies.
const NAVIGATOR: u32 = 360;
const THUMBNAIL: u32 = 640;
/// A full 100% region presented within this needs no reduced preview first.
const QUICK_REGION: std::time::Duration = std::time::Duration::from_millis(40);

/// A finished render and the view it shows.
struct Shown {
    image: Arc<crate::raw::CameraImage>,
    recipe: develop::Recipe,
    max_edge: u32,
    region: Option<[u32; 4]>,
    clipping: bool,
    monitor: Option<PathBuf>,
    navigator: bool,
    out: Shot,
}
enum Shot {
    Pixels(develop::Rendered),
    /// A presented frame, valid while its texture still holds `generation`.
    Frame {
        texture: wgpu::Texture,
        generation: u64,
        preview: Presented,
        histogram: Box<[[u32; 256]; 3]>,
    },
}
impl Shown {
    fn new(job: &RenderJob, out: Shot) -> Self {
        Self {
            image: job.image.clone(),
            recipe: job.recipe.clone(),
            max_edge: job.max_edge,
            region: job.region,
            clipping: job.clipping,
            monitor: job.monitor.clone(),
            navigator: job.navigator,
            out,
        }
    }
    /// The same photo, view and edit, with pixels that are still there.
    fn matches(&self, job: &RenderJob, gpu: Option<&gpu::Processor>) -> bool {
        let same = Arc::ptr_eq(&self.image, &job.image)
            && self.max_edge == job.max_edge
            && self.region == job.region
            && self.recipe == job.recipe;
        match &self.out {
            Shot::Pixels(_) => same,
            Shot::Frame {
                texture,
                generation,
                ..
            } => {
                same && self.clipping == job.clipping
                    && self.monitor == job.monitor
                    && self.navigator == job.navigator
                    && gpu.and_then(|g| g.generation(texture)) == Some(*generation)
            }
        }
    }
}
/// A presented frame's textures as the UI names them.
#[derive(Clone, Copy)]
struct Presented {
    id: egui::TextureId,
    size: [usize; 2],
    navigator: Option<(egui::TextureId, [usize; 2])>,
}
impl Presented {
    fn preview(self) -> Preview {
        Preview::Texture {
            id: self.id,
            size: self.size,
            navigator: self.navigator,
        }
    }
}
/// egui's names for the textures frames are presented into.
struct Textures {
    state: egui_wgpu::RenderState,
    ids: HashMap<wgpu::Texture, egui::TextureId>,
}
impl Textures {
    fn id(&mut self, texture: &wgpu::Texture) -> (egui::TextureId, [usize; 2]) {
        let size = [texture.width() as usize, texture.height() as usize];
        let id = *self.ids.entry(texture.clone()).or_insert_with(|| {
            self.state.renderer.write().register_native_texture(
                &self.state.device,
                &texture.create_view(&Default::default()),
                wgpu::FilterMode::Linear,
            )
        });
        (id, size)
    }
    /// Unregisters textures the GPU dropped; the UI shows newer frames by then.
    fn release(&mut self, textures: Vec<wgpu::Texture>) {
        for texture in textures {
            if let Some(id) = self.ids.remove(&texture) {
                self.state.renderer.write().free_texture(&id);
            }
        }
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
    let mut textures = None;
    // The monitor profile as the GPU applies it, per profile path.
    let mut lut: Option<(PathBuf, Result<Arc<gpu::MonitorLut>, String>)> = None;
    // The last finished Fit and 100% region, so switching back to a view with the same
    // edit shows its sharp image at once instead of rendering it again.
    let (mut fit, mut zoomed): (Option<Shown>, Option<Shown>) = (None, None);
    // Whether the last finished image was a 100% region: then the region is being
    // edited or panned, and a reduced preview comes first.
    let mut showing_region = false;
    // Whether the last full 100% region was presented on the GPU quickly enough that a
    // reduced preview before it would only add work and a blurry frame.
    let mut quick_region = false;
    Latest::new(move |job: RenderJob| {
        let processor = processor.get_or_insert_with(|| match &backend {
            RenderBackend::Cpu => develop::PreviewRenderer::default(),
            RenderBackend::Gpu(None) => develop::PreviewRenderer::with_gpu(),
            RenderBackend::Gpu(Some(state)) => {
                textures = Some(Textures {
                    state: state.clone(),
                    ids: HashMap::new(),
                });
                develop::PreviewRenderer::with_processor(gpu::Processor::with_device(
                    state.device.clone(),
                    state.queue.clone(),
                    &state.adapter.get_info(),
                ))
            }
        });
        let t = Instant::now();
        let mut warning = String::new();
        let monitor = match (&job.monitor, textures.is_some()) {
            (Some(path), true) => {
                if lut.as_ref().is_none_or(|(p, _)| p != path) {
                    let built = gpu::MonitorLut::new(path)
                        .map(Arc::new)
                        .map_err(|e| e.to_string());
                    lut = Some((path.clone(), built));
                }
                match &lut.as_ref().unwrap().1 {
                    Ok(lut) => Some(lut.clone()),
                    Err(e) => {
                        warning = format!(" • ICC failed: {e}");
                        None
                    }
                }
            }
            _ => None,
        };
        let display = |slot, navigator: bool, thumbnail: bool| {
            textures.is_some().then(|| gpu::Display {
                slot,
                clipping: job.clipping,
                monitor: monitor.clone(),
                navigator: navigator.then_some(NAVIGATOR),
                thumbnail: thumbnail.then_some(THUMBNAIL),
            })
        };
        let whole = display(gpu::Slot::Whole, job.navigator, job.thumbnail);
        let zoomed_display = display(gpu::Slot::Region, false, false);
        let status = |stage: RenderStage, backend: &str, warning: &str| {
            format!(
                "{} • {backend} • {:.0} ms{warning}",
                stage.label(),
                t.elapsed().as_secs_f64() * 1000.
            )
        };
        // CPU pixels: display bytes, overlays and reduced copies here, off the UI thread.
        let publish_pixels = |out: develop::Rendered, stage: RenderStage, gpu: bool| {
            if job.cancel.load(Ordering::Relaxed) {
                return;
            }
            let mut rgb = out.rgb8();
            let reduce = |rgb: &[u8], edge: u32| {
                let full = image::RgbImage::from_raw(out.width, out.height, rgb.to_vec())?;
                let k = (edge as f32 / out.width.max(out.height) as f32).min(1.);
                Some(image::imageops::thumbnail(
                    &full,
                    ((out.width as f32 * k) as u32).max(1),
                    ((out.height as f32 * k) as u32).max(1),
                ))
            };
            let thumbnail = (job.thumbnail && stage == RenderStage::Fit)
                .then(|| reduce(&rgb, THUMBNAIL))
                .flatten();
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
            let navigator = (job.navigator && job.region.is_none())
                .then(|| reduce(&rgb, NAVIGATOR))
                .flatten();
            send(
                &tx,
                &ctx,
                Event::Rendered {
                    id: job.id,
                    histogram: Box::new(out.histogram()),
                    preview: Preview::Pixels {
                        image: out,
                        display_rgb: rgb,
                        navigator,
                    },
                    thumbnail,
                    stage,
                    status: status(stage, if gpu { "GPU finish" } else { "CPU" }, &warning),
                },
            );
        };
        let result = (|| -> anyhow::Result<()> {
            if job.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let stage = if job.region.is_some() {
                RenderStage::Region
            } else {
                RenderStage::Fit
            };
            let mut publish = |out: Output, stage: RenderStage, cache: bool, gpu: bool| match out {
                Output::Pixels(out) => {
                    publish_pixels(out.clone(), stage, gpu);
                    Some(Shot::Pixels(out))
                }
                Output::Frame(frame) => {
                    let textures = textures.as_mut()?;
                    textures.release(frame.released);
                    let (id, size) = textures.id(&frame.texture);
                    let presented = Presented {
                        id,
                        size,
                        navigator: frame.navigator.as_ref().map(|t| textures.id(t)),
                    };
                    if !job.cancel.load(Ordering::Relaxed) {
                        send(
                            &tx,
                            &ctx,
                            Event::Rendered {
                                id: job.id,
                                preview: presented.preview(),
                                histogram: frame.histogram.clone(),
                                thumbnail: frame
                                    .thumbnail
                                    .and_then(|(w, h, rgb)| image::RgbImage::from_raw(w, h, rgb)),
                                stage,
                                status: status(stage, "GPU", &warning),
                            },
                        );
                    }
                    cache.then_some(Shot::Frame {
                        texture: frame.texture,
                        generation: frame.generation,
                        preview: presented,
                        histogram: frame.histogram,
                    })
                }
            };
            let cached = if job.region.is_some() { &zoomed } else { &fit };
            if let Some(shown) = cached.as_ref().filter(|s| s.matches(&job, processor.gpu())) {
                match &shown.out {
                    Shot::Pixels(out) => publish_pixels(out.clone(), stage, false),
                    Shot::Frame {
                        preview, histogram, ..
                    } => send(
                        &tx,
                        &ctx,
                        Event::Rendered {
                            id: job.id,
                            preview: preview.preview(),
                            histogram: histogram.clone(),
                            thumbnail: None,
                            stage,
                            status: status(stage, "GPU", &warning),
                        },
                    ),
                }
                showing_region = job.region.is_some();
                return Ok(());
            }
            if let Some(region) = job.region {
                let out = if job.recipe.engine < 3 {
                    Output::Pixels(develop::render_region(&job.image, &job.recipe, region)?)
                } else {
                    // While editing at 100%, a reduced preview keeps sliders responsive;
                    // a newer job cancels the full-resolution render that follows.
                    // Zooming in goes straight to the full region, over the enlarged Fit.
                    if showing_region
                        && !quick_region
                        && let Some(out) = processor.render_region_preview_to(
                            &job.image,
                            &job.recipe,
                            region,
                            &job.cancel,
                            zoomed_display.as_ref(),
                        )?
                    {
                        let gpu = processor.used_gpu();
                        publish(out, RenderStage::Draft, false, gpu);
                    }
                    let started = Instant::now();
                    let out = processor.render_to(
                        &job.image,
                        &job.recipe,
                        0,
                        Some(region),
                        &job.cancel,
                        zoomed_display.as_ref(),
                    )?;
                    quick_region =
                        matches!(out, Output::Frame(_)) && started.elapsed() < QUICK_REGION;
                    out
                };
                let gpu = processor.used_gpu();
                zoomed =
                    publish(out, RenderStage::Region, true, gpu).map(|out| Shown::new(&job, out));
                showing_region = true;
                return Ok(());
            }
            let out = processor.render_to(
                &job.image,
                &job.recipe,
                job.max_edge,
                None,
                &job.cancel,
                whole.as_ref(),
            )?;
            let gpu = processor.used_gpu();
            fit = publish(out, RenderStage::Fit, true, gpu).map(|out| Shown::new(&job, out));
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
            navigator: region.is_none(),
            thumbnail: false,
        });
        let mut stages = Vec::new();
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(20)).unwrap() {
                Event::Rendered {
                    preview: Preview::Pixels { image, .. },
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
