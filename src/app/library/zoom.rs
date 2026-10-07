//! The Loupe's zoomed view of JPEG, TIFF and PNG files (a RAW zooms in
//! Develop's viewport), at the shared `navigator::Zoom`. Only the part of
//! the image in view is uploaded; the Fit preview, enlarged, fills in around
//! it.
//!
//! Memory is budgeted in three parts. Active: the photo's full decode, held
//! by the worker while 1:1 is on and dropped when the photo changes or the
//! Loupe goes back to Fit. In flight: one region at a time, the latest asked
//! for. Retained: one region texture, no larger than the view.
use crate::app::worker::Latest;
use crate::catalog::PhotoId;
use eframe::egui;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, channel},
};

pub(super) struct RegionJob {
    ticket: u64,
    path: PathBuf,
    /// The view's centre, as a fraction of the photo's width and height.
    center: [f32; 2],
    /// The view, in pixels.
    size: [u32; 2],
    /// Below 1 (zoomed out past 100%), the region is shrunk by it before it
    /// is uploaded, so the texture stays the size of the view.
    scale: f32,
    cancel: Arc<AtomicBool>,
}
/// A rendered part of the photo at 1:1.
pub(super) struct Region {
    image: image::RgbImage,
    /// Where it lies, as fractions of the whole photo: x, y, width, height.
    rect: [f32; 4],
    /// The whole photo's size at 1:1, in pixels.
    full: [u32; 2],
}
struct Done {
    ticket: u64,
    result: Result<Region, String>,
}
/// The photo the worker keeps decoded at full resolution.
struct Held {
    path: PathBuf,
    image: image::RgbImage,
}

/// The image pixels in view, read from the full decode in the background.
pub(super) struct Regions {
    /// The photo the region and size below belong to.
    pub photo: Option<PhotoId>,
    pub full: Option<[u32; 2]>,
    pub region: Option<(egui::TextureHandle, [f32; 4])>,
    /// Asked for and not in yet.
    pub pending: bool,
    worker: Latest<Option<RegionJob>>,
    results: Receiver<Done>,
    ticket: u64,
    requested: Option<(PhotoId, [i32; 2], [u32; 2], u32)>,
    cancel: Arc<AtomicBool>,
    pub error: Option<String>,
}
impl Regions {
    pub(super) fn new(ctx: &egui::Context) -> Self {
        let (tx, results) = channel();
        let ctx = ctx.clone();
        let mut held: Option<Held> = None;
        let worker = Latest::new(move |job: Option<RegionJob>| {
            // None lets the full decode go.
            let Some(job) = job else {
                held = None;
                return;
            };
            if job.cancel.load(Ordering::Relaxed) {
                return;
            }
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| region(&job, &mut held)))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("The zoomed view could not be built")));
            if job.cancel.load(Ordering::Relaxed) {
                return;
            }
            let _ = tx.send(Done {
                ticket: job.ticket,
                result: result.map_err(|e| format!("{e:#}")),
            });
            ctx.request_repaint();
        });
        Self {
            photo: None,
            full: None,
            region: None,
            pending: false,
            worker,
            results,
            ticket: 0,
            requested: None,
            cancel: Default::default(),
            error: None,
        }
    }
    /// Drops the region and lets the worker free the full decode.
    pub(super) fn release(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        // A region already sent belongs to the old ticket and is dropped.
        self.ticket += 1;
        self.requested = None;
        self.region = None;
        self.pending = false;
        self.worker.submit(None);
    }
    /// Asks for the `size` image pixels of `photo` around `center` (fractions
    /// of the photo), unless they are already on their way.
    pub(super) fn request(
        &mut self,
        photo: PhotoId,
        path: &std::path::Path,
        center: [f32; 2],
        size: [u32; 2],
        scale: f32,
    ) {
        if self.photo != Some(photo) {
            self.photo = Some(photo);
            self.full = None;
            self.region = None;
            self.error = None;
        }
        // Positions in steps of 8 pixels, so a slow drag asks less often.
        let full = self.full.unwrap_or([4096, 4096]);
        let at = [0, 1].map(|i| (center[i] * full[i] as f32 / 8.).round() as i32);
        // The scale counts too: the same pixels shrunk or not are another texture.
        let wanted = (photo, at, size, (scale.min(1.) * 1000.) as u32);
        if self.requested == Some(wanted) {
            return;
        }
        self.requested = Some(wanted);
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Default::default();
        self.ticket += 1;
        self.pending = true;
        self.worker.submit(Some(RegionJob {
            ticket: self.ticket,
            path: path.into(),
            center,
            size,
            scale: scale.min(1.),
            cancel: self.cancel.clone(),
        }));
    }
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(done) = self.results.try_recv() {
            if done.ticket != self.ticket {
                continue;
            }
            self.pending = false;
            match done.result {
                Ok(region) => {
                    let size = [
                        region.image.width() as usize,
                        region.image.height() as usize,
                    ];
                    let texture = ctx.load_texture(
                        "library-loupe-region",
                        egui::ColorImage::from_rgb(size, region.image.as_raw()),
                        egui::TextureOptions::NEAREST,
                    );
                    self.full = Some(region.full);
                    self.region = Some((texture, region.rect));
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }
    }
    #[cfg(test)]
    pub(super) fn wait(&mut self, ctx: &egui::Context) {
        let started = std::time::Instant::now();
        while self.pending && started.elapsed() < std::time::Duration::from_secs(20) {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.poll(ctx);
        }
    }
}

/// Renders the part of the photo `job` asks for, decoding it at full
/// resolution first unless `held` already has it.
fn region(job: &RegionJob, held: &mut Option<Held>) -> anyhow::Result<Region> {
    if !held.as_ref().is_some_and(|h| h.path == job.path) {
        // The previous photo's decode goes before the next is made.
        *held = None;
        *held = Some(Held {
            path: job.path.clone(),
            image: super::thumbnails::raster(&job.path)?,
        });
    }
    let image = &held.as_ref().unwrap().image;
    let (width, height) = image.dimensions();
    let w = job.size[0].clamp(1, width);
    let h = job.size[1].clamp(1, height);
    let x = (job.center[0] * width as f32 - w as f32 / 2.)
        .round()
        .clamp(0., (width - w) as f32) as u32;
    let y = (job.center[1] * height as f32 - h as f32 / 2.)
        .round()
        .clamp(0., (height - h) as f32) as u32;
    let crop = image::imageops::crop_imm(image, x, y, w, h).to_image();
    let image = if job.scale < 1. {
        let shrink = |side: u32| ((side as f32 * job.scale).round() as u32).max(1);
        image::imageops::thumbnail(&crop, shrink(w), shrink(h))
    } else {
        crop
    };
    Ok(Region {
        image,
        rect: [
            x as f32 / width as f32,
            y as f32 / height as f32,
            w as f32 / width as f32,
            h as f32 / height as f32,
        ],
        full: [width, height],
    })
}
