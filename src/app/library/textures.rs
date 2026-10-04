//! The previews the grid and filmstrip draw, as GPU textures: each file's
//! embedded preview, and each photo's edited preview once rendered. Both are
//! bounded; requests go to the workers in `previews`, and results are
//! matched to the latest request so a late or stale render never shows.
use super::previews::{self, EditJob, EditResult, EditSource, PreviewResult, Progress, Wanted};
use crate::catalog::Photo;
use eframe::egui;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, Sender, SyncSender},
};

/// Textures kept per cache; older ones are dropped first.
const KEPT: usize = 192;

pub(super) struct PreviewTextures {
    /// Embedded previews by file. Virtual copies share a file, and so this.
    pub(super) thumbs: HashMap<PathBuf, egui::TextureHandle>,
    pub(super) thumb_order: VecDeque<PathBuf>,
    pub(super) pending: HashSet<PathBuf>,
    pub(super) failed: HashSet<PathBuf>,
    pub(super) thumb_tx: SyncSender<PathBuf>,
    pub(super) thumb_rx: Receiver<PreviewResult>,
    /// Edited previews: rendered from each photo's edit on a second worker.
    pub(super) edit_tx: Sender<EditJob>,
    pub(super) edit_rx: Receiver<EditResult>,
    /// Photos with an edited preview requested, by the ticket of the latest
    /// request; results of earlier requests are dropped.
    pub(super) edited_requested: HashMap<i64, u64>,
    next_ticket: u64,
    /// Photos asking for an edited preview this frame, and last frame's
    /// as the worker sees them.
    pub(super) edit_seen: HashSet<i64>,
    edit_wanted: Wanted,
    /// Edited previews queued and not yet back.
    pub(super) edits_pending: usize,
    /// Previews showing each photo's edit, by photo.
    pub(super) edited: HashMap<i64, egui::TextureHandle>,
    pub(super) edited_order: VecDeque<i64>,
    pub(super) progress: Progress,
    /// Photos one view shows at once, e.g. a large survey; never fewer
    /// textures are kept, so none is dropped while it is shown.
    pub(super) shown_at_once: usize,
}
impl PreviewTextures {
    /// Starts both preview workers on the shared disk cache.
    pub(super) fn new(ctx: &egui::Context) -> Self {
        let cache = crate::catalog::preview_cache::PreviewCache::path();
        let (thumb_tx, thumb_rx) = previews::spawn(cache.clone(), ctx.clone());
        let edit_wanted = Wanted::default();
        let (edit_tx, edit_rx) = previews::spawn_edited(cache, edit_wanted.clone(), ctx.clone());
        Self {
            thumbs: HashMap::new(),
            thumb_order: VecDeque::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
            thumb_tx,
            thumb_rx,
            edit_tx,
            edit_rx,
            edited_requested: HashMap::new(),
            next_ticket: 0,
            edit_seen: HashSet::new(),
            edit_wanted,
            edits_pending: 0,
            edited: HashMap::new(),
            edited_order: VecDeque::new(),
            progress: Progress::default(),
            shown_at_once: 0,
        }
    }
    /// Takes every finished preview. Drain in every workspace so the bounded
    /// worker never waits for the grid.
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(result) = self.thumb_rx.try_recv() {
            self.progress.finish(&result);
            let PreviewResult {
                path, image: im, ..
            } = result;
            self.pending.remove(&path);
            match im {
                Some(im) => self.insert_thumb(ctx, path, &im),
                None => {
                    self.failed.insert(path);
                }
            }
        }
        while let Ok(result) = self.edit_rx.try_recv() {
            if let EditResult::CacheError(error) = result {
                self.progress.cache_failed(error);
                continue;
            }
            self.edits_pending = self.edits_pending.saturating_sub(1);
            match result {
                EditResult::Ready(id, ticket, im) => {
                    if self.edited_requested.get(&id) == Some(&ticket) {
                        self.insert_edited(ctx, id, &im);
                    }
                }
                EditResult::Skipped(id, ticket) => {
                    if self.edited_requested.get(&id) == Some(&ticket) {
                        self.edited_requested.remove(&id);
                    }
                }
                EditResult::Failed | EditResult::CacheError(_) => {}
            }
        }
    }
    /// Hands the worker the photos shown last frame. Call once per frame.
    pub(super) fn publish_shown(&mut self) {
        let shown = std::mem::take(&mut self.edit_seen);
        *self.edit_wanted.lock().unwrap() = shown;
    }
    pub(super) fn insert_thumb(
        &mut self,
        ctx: &egui::Context,
        path: PathBuf,
        im: &image::RgbImage,
    ) {
        if !self.thumbs.contains_key(&path) {
            while self.thumbs.len() >= KEPT.max(self.shown_at_once) {
                let Some(old) = self.thumb_order.pop_front() else {
                    break;
                };
                self.thumbs.remove(&old);
            }
            self.thumb_order.push_back(path.clone());
        }
        self.thumbs
            .insert(path.clone(), texture(ctx, path.display().to_string(), im));
    }
    pub(super) fn insert_edited(&mut self, ctx: &egui::Context, id: i64, im: &image::RgbImage) {
        if !self.edited.contains_key(&id) {
            while self.edited.len() >= KEPT.max(self.shown_at_once) {
                let Some(old) = self.edited_order.pop_front() else {
                    break;
                };
                self.edited.remove(&old);
                self.edited_requested.remove(&old);
            }
            self.edited_order.push_back(id);
        }
        self.edited
            .insert(id, texture(ctx, format!("edited-{id}"), im));
    }
    fn ticket(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }
    /// Forgets a removed photo's previews, so a later photo given its id
    /// starts afresh.
    pub(super) fn forget(&mut self, id: i64) {
        self.edited.remove(&id);
        self.edited_order.retain(|other| *other != id);
        self.edited_requested.remove(&id);
        self.edit_seen.remove(&id);
    }
    /// The photo's preview: its edit once rendered, else the embedded one.
    pub(super) fn texture(&self, photo: &Photo) -> Option<&egui::TextureHandle> {
        self.edited
            .get(&photo.id)
            .or_else(|| self.thumbs.get(&photo.path))
    }
    /// Photos whose thumbnail shows an edit.
    pub(super) fn edited_ids(&self) -> impl Iterator<Item = i64> + '_ {
        self.edited.keys().copied()
    }
    /// Whether the photo's thumbnail already shows its edit (crop included).
    pub(super) fn has_edited(&self, id: i64) -> bool {
        self.edited.contains_key(&id)
    }
    /// Queues the previews a shown photo needs: the embedded one until its
    /// edited one is in, and the edited one for a saved or Lightroom edit,
    /// which `edit` looks up when it is first asked for.
    pub(super) fn request(
        &mut self,
        photo: &Photo,
        ctx: &egui::Context,
        edit: impl FnOnce() -> Option<EditSource>,
    ) {
        if !self.edited.contains_key(&photo.id) {
            self.request_thumbnail(&photo.path, ctx);
        }
        self.request_edited(photo, edit);
    }
    pub(super) fn request_thumbnail(&mut self, path: &Path, ctx: &egui::Context) {
        if !self.thumbs.contains_key(path)
            && !self.pending.contains(path)
            && !self.failed.contains(path)
            && self.thumb_tx.try_send(path.to_path_buf()).is_ok()
        {
            self.pending.insert(path.to_path_buf());
            self.progress.queued();
            ctx.request_repaint();
        }
    }
    /// Queues an edited preview for a photo with a saved or Lightroom edit.
    /// A photo without one is asked about once, until it is forgotten.
    pub(super) fn request_edited(
        &mut self,
        photo: &Photo,
        edit: impl FnOnce() -> Option<EditSource>,
    ) {
        self.edit_seen.insert(photo.id);
        if self.edited_requested.contains_key(&photo.id) {
            return;
        }
        let ticket = self.ticket();
        self.edited_requested.insert(photo.id, ticket);
        if let Some(source) = edit()
            && self
                .edit_tx
                .send(EditJob::Render {
                    id: photo.id,
                    ticket,
                    path: photo.path.clone(),
                    source,
                })
                .is_ok()
        {
            self.edits_pending += 1;
        }
    }
    /// Shows Develop's latest render as the photo's thumbnail and caches it
    /// under the edit it was rendered with. Renders still in flight are older.
    pub(super) fn store_edited(
        &mut self,
        ctx: &egui::Context,
        id: i64,
        path: PathBuf,
        image: image::RgbImage,
        recipe_json: String,
    ) {
        let tag = EditSource::Recipe(recipe_json).tag();
        let ticket = self.ticket();
        self.edited_requested.insert(id, ticket);
        self.insert_edited(ctx, id, &image);
        let _ = self.edit_tx.send(EditJob::Store { path, tag, image });
    }
    /// Worth a status line: still working, or something could not be prepared.
    pub(super) fn progress_active(&self) -> bool {
        self.progress.active() || self.edits_pending > 0
    }
    pub(super) fn show_progress(&self, ui: &mut egui::Ui) {
        self.progress.show(ui, self.edits_pending);
    }
}
fn texture(ctx: &egui::Context, name: String, im: &image::RgbImage) -> egui::TextureHandle {
    ctx.load_texture(
        name,
        egui::ColorImage::from_rgb([im.width() as usize, im.height() as usize], im.as_raw()),
        egui::TextureOptions::LINEAR,
    )
}
