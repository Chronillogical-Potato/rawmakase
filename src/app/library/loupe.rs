//! Lightroom's Loupe in the Library: the active photo large, with the
//! filmstrip below. Its preview is built for the screen in the background:
//! for an unedited RAW the embedded JPEG first, labelled "Embedded Preview"
//! as in Lightroom, then the photo developed with its edit or the defaults.
//! JPEG, TIFF and PNG files are decoded instead. Nothing larger than the
//! view is ever uploaded.
use super::previews::EditSource;
use super::{Action, Library, thumbnails};
use crate::app::theme;
use crate::app::worker::Latest;
use crate::catalog::Photo;
use eframe::egui::{self, Color32, Vec2};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, channel},
};

/// Views are rendered in steps of this many pixels, so resizing the window
/// a little does not render the photo again.
const EDGE_STEP: u32 = 512;

struct Job {
    ticket: u64,
    path: PathBuf,
    /// Longest side, in pixels, of the image wanted.
    edge: u32,
    edit: Option<EditSource>,
    cancel: Arc<AtomicBool>,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Stage {
    Embedded,
    Rendered,
}
struct Done {
    ticket: u64,
    result: Result<(Stage, image::RgbImage), String>,
}
/// What the Loupe shows under the photo.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum State {
    Loading,
    Ready(Stage),
    Failed(String),
}

pub(super) struct Loupe {
    pub open: bool,
    worker: Latest<Job>,
    results: Receiver<Done>,
    ticket: u64,
    /// What the current ticket asked for: photo, edit and edge.
    requested: Option<(i64, Option<String>, u32)>,
    /// The edit of the photo asked for, read from the catalog once.
    edit: Option<EditSource>,
    cancel: Arc<AtomicBool>,
    texture: Option<egui::TextureHandle>,
    pub state: State,
}
impl Loupe {
    pub(super) fn new(ctx: &egui::Context) -> Self {
        let (tx, results) = channel();
        let ctx = ctx.clone();
        let worker = Latest::new(move |job: Job| {
            if job.cancel.load(Ordering::Relaxed) {
                return;
            }
            let send = |result| {
                let _ = tx.send(Done {
                    ticket: job.ticket,
                    result,
                });
                ctx.request_repaint();
            };
            let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                prepare(&job, &|stage, image| send(Ok((stage, image))))
            }));
            match prepared {
                Ok(Ok(())) => {}
                _ if job.cancel.load(Ordering::Relaxed) => {}
                Ok(Err(e)) => send(Err(format!("{e:#}"))),
                Err(_) => send(Err("The preview could not be built".into())),
            }
        });
        Self {
            open: false,
            worker,
            results,
            ticket: 0,
            requested: None,
            edit: None,
            cancel: Default::default(),
            texture: None,
            state: State::Loading,
        }
    }
    /// Asks for `photo` at `edge` pixels, unless that is already on its way.
    fn request(&mut self, photo: &Photo, edit: Option<EditSource>, edge: u32) {
        let edge = edge.div_ceil(EDGE_STEP).max(1) * EDGE_STEP;
        let wanted = (photo.id, edit.as_ref().map(EditSource::tag), edge);
        if self.requested.as_ref() == Some(&wanted) {
            return;
        }
        self.edit = edit.clone();
        let same_photo = self.requested.as_ref().is_some_and(|r| r.0 == photo.id);
        self.requested = Some(wanted);
        // Moving on cancels the photo being prepared; its result is dropped.
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Default::default();
        self.ticket += 1;
        if !same_photo {
            self.texture = None;
            self.state = State::Loading;
        }
        self.worker.submit(Job {
            ticket: self.ticket,
            path: photo.path.clone(),
            edge,
            edit,
            cancel: self.cancel.clone(),
        });
    }
    /// Forgets the photo shown, e.g. after its edit changed elsewhere.
    pub(super) fn reset(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.requested = None;
        self.texture = None;
        self.state = State::Loading;
    }
    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(done) = self.results.try_recv() {
            if done.ticket != self.ticket {
                continue;
            }
            match done.result {
                // A rendered preview is never replaced by the embedded one.
                Ok((Stage::Embedded, _)) if self.state == State::Ready(Stage::Rendered) => {}
                Ok((stage, image)) => {
                    let size = [image.width() as usize, image.height() as usize];
                    self.texture = Some(ctx.load_texture(
                        "library-loupe",
                        egui::ColorImage::from_rgb(size, image.as_raw()),
                        egui::TextureOptions::LINEAR,
                    ));
                    self.state = State::Ready(stage);
                }
                Err(e) => self.state = State::Failed(e),
            }
        }
    }
    #[cfg(test)]
    pub(super) fn wait(&mut self, ctx: &egui::Context) {
        let started = std::time::Instant::now();
        while matches!(self.state, State::Loading | State::Ready(Stage::Embedded))
            && started.elapsed() < std::time::Duration::from_secs(20)
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.poll(ctx);
        }
    }
    #[cfg(test)]
    pub(super) fn texture_size(&self) -> Option<[usize; 2]> {
        self.texture.as_ref().map(|t| t.size())
    }
}

/// Builds the preview for `job`, handing each stage to `out` as it is ready.
fn prepare(job: &Job, out: &dyn Fn(Stage, image::RgbImage)) -> anyhow::Result<()> {
    let cancelled = || job.cancel.load(Ordering::Relaxed);
    if !crate::storage::is_raw(&job.path) {
        let image = thumbnails::raster(&job.path)?;
        out(Stage::Rendered, thumbnails::downscale(&image, job.edge));
        return Ok(());
    }
    let mut raw = crate::raw::Raw::open(&job.path)?;
    // The camera's JPEG shows an unedited photo at once; an edited one waits
    // for its edit, with its grid preview standing in.
    if job.edit.is_none()
        && let Ok(embedded) = crate::raw::thumbnail(&mut raw)
    {
        out(Stage::Embedded, thumbnails::downscale(&embedded, job.edge));
    }
    if cancelled() {
        return Ok(());
    }
    let recipe = EditSource::recipe(job.edit.as_ref(), &raw)?;
    // The half-size decode has more pixels than a screen needs for Fit.
    let image = raw.develop(true, &job.cancel)?;
    if cancelled() {
        return Ok(());
    }
    let rendered = crate::develop::render(&image, &recipe, job.edge)?;
    let image = image::RgbImage::from_raw(rendered.width, rendered.height, rendered.rgb8())
        .ok_or_else(|| anyhow::anyhow!("Invalid preview size"))?;
    out(Stage::Rendered, image);
    Ok(())
}

impl Library {
    pub fn loupe_open(&self) -> bool {
        self.loupe.open
    }
    /// E, Return or a double-click: the active photo, large.
    pub fn open_loupe(&mut self) {
        if self.selection.active.is_none() {
            self.select(self.visible.first().map(|i| self.photos[*i].id));
        }
        self.loupe.open = self.selection.active.is_some();
        self.scroll_to_active = true;
    }
    /// G or Esc: back to the grid, at the active photo.
    pub fn close_loupe(&mut self) {
        if self.loupe.open {
            self.loupe.open = false;
            self.loupe.reset();
            self.scroll_to_active = true;
        }
    }
    /// The Loupe in place of the grid, with the filmstrip below.
    pub(super) fn loupe(&mut self, ui: &mut egui::Ui) -> Action {
        self.loupe.poll(ui.ctx());
        let Some(id) = self.selection.active else {
            self.close_loupe();
            return Action::None;
        };
        let mut target = None;
        egui::Panel::bottom("library-loupe-filmstrip")
            .exact_size(128.)
            .frame(egui::Frame::new().fill(theme::gray(26)))
            .show(ui, |ui| {
                // In Loupe a metadata change applies to the active photo only.
                target = self.filmstrip(ui, id).0;
            });
        match target {
            Some(super::Pick::Show(id)) => self.select(Some(id)),
            Some(super::Pick::Develop(id)) => return Action::Develop(id),
            None => {}
        }
        let Some(photo) = self.selection.active.and_then(|id| self.photo(id)).cloned() else {
            return Action::None;
        };
        let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
        ui.painter().rect_filled(rect, 0., theme::gray(36));
        let available = self.is_available(&photo.path);
        if available {
            let edit = if self
                .loupe
                .requested
                .as_ref()
                .is_some_and(|r| r.0 == photo.id)
            {
                self.loupe.edit.clone()
            } else {
                self.catalog
                    .edit_texts(photo.id)
                    .ok()
                    .and_then(|(recipe, lightroom)| {
                        recipe
                            .map(EditSource::Recipe)
                            .or(lightroom.map(EditSource::Lightroom))
                    })
            };
            let edge = (rect.width().max(rect.height()) * ui.ctx().pixels_per_point()) as u32;
            self.loupe.request(&photo, edit, edge);
        } else {
            self.loupe.reset();
        }
        self.request_previews(&photo, ui.ctx());
        let inset = rect.shrink(16.);
        // Until its own preview is in, the grid's stands in, enlarged.
        let texture = self
            .loupe
            .texture
            .as_ref()
            .filter(|_| available)
            .or_else(|| self.texture(&photo));
        if let Some(texture) = texture {
            let size = texture.size_vec2();
            let scale = (inset.width() / size.x).min(inset.height() / size.y);
            ui.painter().image(
                texture.id(),
                egui::Rect::from_center_size(inset.center(), size * scale),
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                Color32::WHITE,
            );
        }
        let note = match (&self.loupe.state, available, texture.is_some()) {
            (_, false, true) => {
                "Offline: showing the cached preview. Full detail needs the original.".into()
            }
            (_, false, false) => "Offline, and there is no cached preview".into(),
            (State::Loading, ..) => "Loading…".into(),
            (State::Ready(Stage::Embedded), ..) => "Embedded Preview".into(),
            (State::Ready(Stage::Rendered), ..) => String::new(),
            (State::Failed(e), ..) => format!("Preview unavailable: {e}"),
        };
        if !note.is_empty() {
            ui.painter().text(
                rect.left_bottom() + Vec2::new(12., -10.),
                egui::Align2::LEFT_BOTTOM,
                note,
                egui::FontId::proportional(11.),
                theme::gray(170),
            );
        }
        Action::None
    }
}
