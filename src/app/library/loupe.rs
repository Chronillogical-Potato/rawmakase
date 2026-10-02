//! Lightroom's Loupe in the Library: the active photo large, with the
//! filmstrip below. Its preview is built for the screen in the background:
//! for an unedited RAW the embedded JPEG first, labelled "Embedded Preview"
//! as in Lightroom, then the photo developed with its edit or the defaults.
//! JPEG, TIFF and PNG files are decoded instead. Nothing larger than the
//! view is ever uploaded. The next photo in the direction of travel is
//! prepared ahead and kept, within a budget, until it is shown.
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
/// Prepared neighbours kept, in bytes; the oldest go first.
const PREFETCHED_BYTES: usize = 64 << 20;

/// A preview as asked for: photo, edit tag and edge.
type Key = (i64, Option<String>, u32);

struct Job {
    ticket: u64,
    /// A neighbour prepared ahead, kept under this key; only its final
    /// stage is wanted.
    ahead: Option<Key>,
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
    ahead: Option<Key>,
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
    pub zoom: super::zoom::Zoom,
    worker: Latest<Job>,
    /// Prepares the neighbour, on its own so it never holds up the photo shown.
    ahead: Latest<Job>,
    ahead_cancel: Arc<AtomicBool>,
    /// The neighbour asked for last, and those ready, oldest first.
    ahead_requested: Option<Key>,
    prefetched: std::collections::VecDeque<(Key, image::RgbImage)>,
    results: Receiver<Done>,
    ticket: u64,
    /// What the current ticket asked for: photo, edit and edge.
    requested: Option<Key>,
    /// The edit of the photo asked for, read from the catalog once.
    edit: Option<EditSource>,
    cancel: Arc<AtomicBool>,
    texture: Option<egui::TextureHandle>,
    pub state: State,
}
impl Loupe {
    pub(super) fn new(ctx: &egui::Context) -> Self {
        let (tx, results) = channel();
        let worker = |tx: std::sync::mpsc::Sender<Done>, ctx: egui::Context| {
            Latest::new(move |job: Job| {
                if job.cancel.load(Ordering::Relaxed) {
                    return;
                }
                let send = |result| {
                    let _ = tx.send(Done {
                        ticket: job.ticket,
                        ahead: job.ahead.clone(),
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
            })
        };
        Self {
            open: false,
            zoom: super::zoom::Zoom::new(ctx),
            worker: worker(tx.clone(), ctx.clone()),
            ahead: worker(tx, ctx.clone()),
            ahead_cancel: Default::default(),
            ahead_requested: None,
            prefetched: Default::default(),
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
    fn request(&mut self, ctx: &egui::Context, photo: &Photo, edit: Option<EditSource>, edge: u32) {
        let wanted = key(photo, &edit, edge);
        if self.requested.as_ref() == Some(&wanted) {
            return;
        }
        // Prepared ahead: shown at once.
        if let Some(at) = self.prefetched.iter().position(|(k, _)| *k == wanted) {
            let (_, image) = self.prefetched.remove(at).unwrap();
            self.cancel.store(true, Ordering::Relaxed);
            self.ticket += 1;
            self.requested = Some(wanted);
            self.edit = edit;
            self.show(ctx, Stage::Rendered, &image);
            return;
        }
        let edge = wanted.2;
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
            ahead: None,
            path: photo.path.clone(),
            edge,
            edit,
            cancel: self.cancel.clone(),
        });
    }
    /// Prepares `photo`, the next one along, unless it is ready or on its way.
    fn prepare_ahead(&mut self, photo: &Photo, edit: Option<EditSource>, edge: u32) {
        let wanted = key(photo, &edit, edge);
        if self.ahead_requested.as_ref() == Some(&wanted)
            || self.prefetched.iter().any(|(k, _)| *k == wanted)
        {
            return;
        }
        self.ahead_cancel.store(true, Ordering::Relaxed);
        self.ahead_cancel = Default::default();
        self.ahead_requested = Some(wanted.clone());
        self.ahead.submit(Job {
            ticket: 0,
            ahead: Some(wanted.clone()),
            path: photo.path.clone(),
            edge: wanted.2,
            edit,
            cancel: self.ahead_cancel.clone(),
        });
    }
    fn show(&mut self, ctx: &egui::Context, stage: Stage, image: &image::RgbImage) {
        let size = [image.width() as usize, image.height() as usize];
        self.texture = Some(ctx.load_texture(
            "library-loupe",
            egui::ColorImage::from_rgb(size, image.as_raw()),
            egui::TextureOptions::LINEAR,
        ));
        self.state = State::Ready(stage);
    }
    /// Forgets the photo shown, e.g. after its edit changed elsewhere.
    pub(super) fn reset(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.ahead_cancel.store(true, Ordering::Relaxed);
        self.ahead_requested = None;
        self.prefetched.clear();
        self.zoom.release();
        self.requested = None;
        self.texture = None;
        self.state = State::Loading;
    }
    fn poll(&mut self, ctx: &egui::Context) {
        self.zoom.poll(ctx);
        while let Ok(done) = self.results.try_recv() {
            if let Some(key) = done.ahead {
                if let Ok((Stage::Rendered, image)) = done.result {
                    self.prefetched.push_back((key, image));
                    let bytes = |p: &std::collections::VecDeque<(Key, image::RgbImage)>| {
                        p.iter().map(|(_, i)| i.as_raw().len()).sum::<usize>()
                    };
                    while bytes(&self.prefetched) > PREFETCHED_BYTES {
                        self.prefetched.pop_front();
                    }
                }
                continue;
            }
            if done.ticket != self.ticket {
                continue;
            }
            match done.result {
                // A rendered preview is never replaced by the embedded one.
                Ok((Stage::Embedded, _)) if self.state == State::Ready(Stage::Rendered) => {}
                Ok((stage, image)) => self.show(ctx, stage, &image),
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
    pub(super) fn wait_ahead(&mut self, ctx: &egui::Context) {
        let started = std::time::Instant::now();
        while self.prefetched.is_empty() && started.elapsed() < std::time::Duration::from_secs(20) {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.poll(ctx);
        }
    }
    #[cfg(test)]
    pub(super) fn texture_size(&self) -> Option<[usize; 2]> {
        self.texture.as_ref().map(|t| t.size())
    }
}

fn key(photo: &Photo, edit: &Option<EditSource>, edge: u32) -> Key {
    (
        photo.id,
        edit.as_ref().map(EditSource::tag),
        edge.div_ceil(EDGE_STEP).max(1) * EDGE_STEP,
    )
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
        && job.ahead.is_none()
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
    /// The photo's edit, as the Loupe renders it.
    fn edit_of(&self, photo: &Photo) -> Option<EditSource> {
        let (recipe, lightroom) = self.catalog.edit_texts(photo.id).ok()?;
        recipe
            .map(EditSource::Recipe)
            .or(lightroom.map(EditSource::Lightroom))
    }
    /// Z, a click, Cmd+= and Cmd+-: 1:1 or Fit, keeping the place in the photo.
    pub(super) fn zoom_loupe(&mut self, on: Option<bool>) {
        let on = on.unwrap_or(!self.loupe.zoom.on);
        self.loupe.zoom.set(on, None);
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
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        ui.painter().rect_filled(rect, 0., theme::gray(36));
        let ppp = ui.ctx().pixels_per_point();
        let available = self.is_available(&photo.path);
        let edit = if self
            .loupe
            .requested
            .as_ref()
            .is_some_and(|r| r.0 == photo.id)
        {
            self.loupe.edit.clone()
        } else {
            self.edit_of(&photo)
        };
        let edge = (rect.width().max(rect.height()) * ppp) as u32;
        if available {
            self.loupe.request(ui.ctx(), &photo, edit.clone(), edge);
            // Once this photo is ready, the next one along is prepared.
            if self.loupe.state == State::Ready(Stage::Rendered)
                && let Some(next) = self
                    .navigate(photo.id, self.loupe_direction)
                    .filter(|n| *n != photo.id)
                    .and_then(|n| self.photo(n).cloned())
                && self.is_available(&next.path)
            {
                let next_edit = self.edit_of(&next);
                self.loupe.prepare_ahead(&next, next_edit, edge);
            }
        } else if self.loupe.requested.is_some() {
            self.loupe.reset();
        }
        self.request_previews(&photo, ui.ctx());
        let inset = rect.shrink(16.);
        // Until its own preview is in, the grid's stands in, enlarged.
        let texture = self
            .loupe
            .texture
            .clone()
            .filter(|_| available)
            .or_else(|| self.texture(&photo).cloned());
        let fit = texture.as_ref().map(|t| {
            let size = t.size_vec2();
            let scale = (inset.width() / size.x).min(inset.height() / size.y);
            egui::Rect::from_center_size(inset.center(), size * scale)
        });
        // A click zooms to 1:1 at the point clicked, and back; a drag pans.
        if response.clicked() && available {
            let at = response.interact_pointer_pos().zip(fit).map(|(pos, fit)| {
                [
                    (pos.x - fit.left()) / fit.width(),
                    (pos.y - fit.top()) / fit.height(),
                ]
            });
            let on = !self.loupe.zoom.on;
            self.loupe.zoom.set(on, if on { at } else { None });
        }
        let zoom = &mut self.loupe.zoom;
        let full = zoom
            .full
            .filter(|_| zoom.on && available && zoom.photo == Some(photo.id));
        let shown = match full {
            Some(full) => {
                let size = egui::vec2(full[0] as f32, full[1] as f32) / ppp;
                if response.dragged() {
                    let delta = response.drag_delta();
                    zoom.center[0] -= delta.x / size.x;
                    zoom.center[1] -= delta.y / size.y;
                }
                // The photo covers the view where it is larger, and is
                // centred where it is smaller.
                for i in 0..2 {
                    let half = rect.size()[i] / 2. / size[i];
                    zoom.center[i] = if half >= 0.5 {
                        0.5
                    } else {
                        zoom.center[i].clamp(half, 1. - half)
                    };
                }
                Some(egui::Rect::from_min_size(
                    rect.center() - egui::vec2(zoom.center[0] * size.x, zoom.center[1] * size.y),
                    size,
                ))
            }
            None => fit,
        };
        if zoom.on && available {
            let size = [(rect.width() * ppp) as u32, (rect.height() * ppp) as u32];
            zoom.request(photo.id, &photo.path, edit, size);
        }
        let painter = ui.painter().with_clip_rect(rect);
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.));
        if let (Some(texture), Some(at)) = (&texture, shown) {
            painter.image(texture.id(), at, uv, Color32::WHITE);
        }
        // The 1:1 region over the enlarged Fit preview.
        if let (Some(at), true) = (shown, full.is_some())
            && let Some((region, [x, y, w, h])) = &zoom.region
        {
            let place = egui::Rect::from_min_size(
                at.min + egui::vec2(x * at.width(), y * at.height()),
                egui::vec2(w * at.width(), h * at.height()),
            );
            painter.image(region.id(), place, uv, Color32::WHITE);
        }
        if zoom.on && available {
            ui.ctx().set_cursor_icon(if response.dragged() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
        let note = match (&self.loupe.state, available, texture.is_some()) {
            (_, false, true) => {
                "Offline: showing the cached preview. Full detail needs the original.".into()
            }
            (_, false, false) => "Offline, and there is no cached preview".into(),
            _ if self.loupe.zoom.on && self.loupe.zoom.error.is_some() => format!(
                "1:1 unavailable: {}",
                self.loupe.zoom.error.clone().unwrap_or_default()
            ),
            _ if self.loupe.zoom.on && (self.loupe.zoom.pending || full.is_none()) => {
                "Loading 1:1…".into()
            }
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
