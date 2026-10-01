//! Catalog browsing; thumbnail work is bounded and independent of RAW development.
use super::widgets::{COMPACT_SEGMENT_HEIGHT, section, segmented};
use crate::app::theme;
use crate::catalog::{Catalog, Collection, Folder, Photo};
use anyhow::Result;
use eframe::egui::{self, Color32, Vec2};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::mpsc::{Receiver, SyncSender},
};

pub enum Action {
    None,
    Develop(i64),
    RelinkRoot(i64),
    RelinkFolder(i64),
    AddFolder,
}
/// Lightroom's virtual copy commands, carried out by the editor so the open
/// edit is saved first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CopyAction {
    Create(i64),
    SetMaster(i64),
    /// Asks first, as Lightroom does.
    Remove(i64),
}
pub struct Library {
    pub catalog: Catalog,
    pub photos: Vec<Photo>,
    pub selected: Option<i64>,
    folders: Vec<Folder>,
    collections: Vec<Collection>,
    roots: Vec<(i64, String, Option<String>)>,
    /// Whether each external volume's mount point exists, checked off the UI
    /// thread so a hung network mount can't stall drawing.
    /// Per volume mount (the startup disk as "/"): attached, and free/total bytes.
    volumes_online: std::sync::Arc<std::sync::Mutex<HashMap<PathBuf, VolumeState>>>,
    volumes_checked: Option<std::time::Instant>,
    /// A check is still running, perhaps stuck on a stalled mount: start no other.
    volumes_busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    folder_scope: Option<HashSet<i64>>,
    selected_folder: String,
    expanded: HashSet<String>,
    collection: Option<i64>,
    members: HashSet<i64>,
    query: String,
    rating: i32,
    flag: i32,
    label_filter: Option<String>,
    only_missing: bool,
    reverse: bool,
    thumb_size: f32,
    strip_current: Option<i64>,
    visible: Vec<usize>,
    available: HashSet<PathBuf>,
    thumbs: HashMap<PathBuf, egui::TextureHandle>,
    thumb_order: VecDeque<PathBuf>,
    pending: HashSet<PathBuf>,
    failed: HashSet<PathBuf>,
    thumb_tx: SyncSender<PathBuf>,
    thumb_rx: Receiver<previews::PreviewResult>,
    /// Edited previews: rendered from each photo's edit on a second worker.
    edit_tx: std::sync::mpsc::Sender<previews::EditJob>,
    edit_rx: Receiver<previews::EditResult>,
    /// Photos with an edited preview requested, by the ticket of the latest
    /// request; results of earlier requests are dropped.
    edited_requested: HashMap<i64, u64>,
    next_ticket: u64,
    /// Photos asking for an edited preview this frame, and last frame's
    /// as the worker sees them.
    edit_seen: HashSet<i64>,
    edit_wanted: previews::Wanted,
    /// Edited previews queued and not yet back.
    edits_pending: usize,
    /// Previews showing each photo's edit, by photo: virtual copies share a
    /// file, and so its embedded preview in `thumbs`, but not an edit.
    edited: HashMap<i64, egui::TextureHandle>,
    edited_order: VecDeque<i64>,
    preview_progress: previews::Progress,
    /// A virtual copy command from a thumbnail menu, for the editor.
    copy_request: Option<CopyAction>,
    /// The Copy Name being typed, for the photo it belongs to.
    copy_name: Option<(i64, String)>,
    pub message: String,
}
impl Library {
    pub fn load(path: &std::path::Path, ctx: egui::Context) -> Result<Self> {
        crate::platform::network::prepare_filesystem_bridge();
        let mut catalog = Catalog::open(path)?;
        // Catalogs imported before history was kept: recover it from the
        // stored Lightroom catalog. Best effort; a failure only hides history.
        let _ = catalog.backfill_lightroom_history();
        let (tx, result_rx) = previews::spawn(
            crate::catalog::preview_cache::PreviewCache::path(),
            ctx.clone(),
        );
        let edit_wanted = previews::Wanted::default();
        let (edit_tx, edit_rx) = previews::spawn_edited(
            crate::catalog::preview_cache::PreviewCache::path(),
            edit_wanted.clone(),
            ctx,
        );
        let mut s = Self {
            catalog,
            photos: Vec::new(),
            selected: None,
            folders: Vec::new(),
            collections: Vec::new(),
            roots: Vec::new(),
            volumes_online: Default::default(),
            volumes_checked: None,
            volumes_busy: Default::default(),
            folder_scope: None,
            selected_folder: String::new(),
            expanded: HashSet::new(),
            collection: None,
            members: HashSet::new(),
            query: String::new(),
            rating: 0,
            flag: 2,
            label_filter: None,
            only_missing: false,
            reverse: false,
            thumb_size: 190.,
            strip_current: None,
            visible: Vec::new(),
            available: HashSet::new(),
            thumbs: HashMap::new(),
            thumb_order: VecDeque::new(),
            pending: HashSet::new(),
            failed: HashSet::new(),
            thumb_tx: tx,
            thumb_rx: result_rx,
            edit_tx,
            edit_rx,
            edited_requested: HashMap::new(),
            next_ticket: 0,
            edit_seen: HashSet::new(),
            edit_wanted,
            edits_pending: 0,
            edited: HashMap::new(),
            edited_order: VecDeque::new(),
            preview_progress: Default::default(),
            copy_request: None,
            copy_name: None,
            message: String::new(),
        };
        s.refresh()?;
        // Start with a selection, as Lightroom does, so the side panels are filled.
        s.selected = s.visible.first().map(|i| s.photos[*i].id);
        Ok(s)
    }
    pub fn refresh(&mut self) -> Result<()> {
        self.reload()?;
        self.available = available_paths(&self.photos);
        self.failed.clear();
        self.filter();
        Ok(())
    }
    /// Reads the catalog again without checking which files are online,
    /// for changes that add or remove no file, such as virtual copies.
    fn reload(&mut self) -> Result<()> {
        // Earlier imports could pick up macOS "._" metadata files; never show them.
        self.photos = self.catalog.photos()?;
        self.photos
            .retain(|p| !crate::storage::is_hidden(std::path::Path::new(&p.filename)));
        self.folders = self.catalog.folders()?;
        for folder in &mut self.folders {
            folder.count = self.photos.iter().filter(|p| p.folder == folder.id).count();
        }
        self.collections = self.catalog.collections()?;
        self.roots = self.catalog.roots()?;
        self.filter();
        Ok(())
    }
    pub fn available_count(&self) -> usize {
        self.photos
            .iter()
            .filter(|p| self.available.contains(&p.path))
            .count()
    }
    fn filter(&mut self) {
        let q = self.query.to_lowercase();
        self.visible = self
            .photos
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                self.folder_scope
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&p.folder))
                    && (self.collection.is_none() || self.members.contains(&p.id))
                    && self
                        .label_filter
                        .as_ref()
                        .is_none_or(|label| &p.label == label)
                    && p.rating >= self.rating
                    && (self.flag == 2 || p.flag == self.flag)
                    && (!self.only_missing || !self.available.contains(&p.path))
                    && (q.is_empty()
                        || format!(
                            "{} {} {} {} {}",
                            p.filename, p.copy_name, p.keywords, p.captured, p.label
                        )
                        .to_lowercase()
                        .contains(&q))
            })
            .map(|(i, _)| i)
            .collect();
        if self.reverse {
            self.visible.reverse()
        }
        if self
            .selected
            .is_some_and(|id| !self.visible.iter().any(|i| self.photos[*i].id == id))
        {
            self.selected = None;
        }
    }
    pub fn photo(&self, id: i64) -> Option<&Photo> {
        self.photos.iter().find(|p| p.id == id)
    }
    pub fn navigate(&self, id: i64, delta: i32) -> Option<i64> {
        let at = self.visible.iter().position(|i| self.photos[*i].id == id)?;
        let n = (at as i32 + delta).clamp(0, self.visible.len().saturating_sub(1) as i32) as usize;
        self.visible.get(n).map(|i| self.photos[*i].id)
    }
    fn labels(&self) -> Vec<String> {
        let mut labels: Vec<String> = crate::app::photo_metadata::LABELS
            .iter()
            .map(|s| (*s).into())
            .collect();
        let mut custom: Vec<_> = self
            .photos
            .iter()
            .map(|p| &p.label)
            .filter(|s| !s.is_empty() && !labels.contains(s))
            .cloned()
            .collect();
        custom.sort();
        custom.dedup();
        labels.extend(custom);
        labels
    }
    pub fn edit_metadata(
        &mut self,
        id: i64,
        edit: crate::app::photo_metadata::Edit,
        advance: bool,
    ) -> Result<Option<i64>> {
        let p = self
            .photo(id)
            .ok_or_else(|| anyhow::anyhow!("Unknown photo"))?;
        let (rating, flag, label) = edit.values(p);
        let position = self.visible.iter().position(|i| self.photos[*i].id == id);
        let following: Vec<_> = position.map_or_else(Vec::new, |at| {
            self.visible[at + 1..]
                .iter()
                .map(|i| self.photos[*i].id)
                .collect()
        });
        self.catalog.set_metadata(id, rating, flag, &label)?;
        let p = self.photos.iter_mut().find(|p| p.id == id).unwrap();
        p.rating = rating;
        p.flag = flag;
        p.label = label;
        self.message = format!(
            "{} · {} stars · {} · {}",
            p.filename,
            rating,
            match flag {
                1 => "Pick",
                -1 => "Reject",
                _ => "Unflagged",
            },
            if p.label.is_empty() {
                "No label"
            } else {
                &p.label
            }
        );
        self.filter();
        let next = following
            .into_iter()
            .find(|next| self.visible.iter().any(|i| self.photos[*i].id == *next));
        let still_visible = self.visible.iter().any(|i| self.photos[*i].id == id);
        if advance || !still_visible {
            self.selected = next.or_else(|| {
                if still_visible {
                    Some(id)
                } else {
                    self.visible.last().map(|i| self.photos[*i].id)
                }
            });
        }
        Ok(if advance { next } else { None })
    }
    /// A virtual copy command chosen from a thumbnail menu since last asked.
    pub(super) fn take_copy_request(&mut self) -> Option<CopyAction> {
        self.copy_request.take()
    }
    /// Creates a virtual copy of `id` and selects it.
    pub(super) fn create_virtual_copy(&mut self, id: i64) -> Result<i64> {
        let copy = self.catalog.create_virtual_copy(id)?;
        self.reload()?;
        self.show(copy);
        if let Some(p) = self.photo(copy) {
            self.message = format!("Created {} of {}", p.copy_name, p.filename);
        }
        Ok(copy)
    }
    pub(super) fn set_copy_as_master(&mut self, id: i64) -> Result<()> {
        self.catalog.set_copy_as_master(id)?;
        self.reload()?;
        self.show(id);
        if let Some(p) = self.photo(id) {
            self.message = format!("This copy is now the master of {}", p.filename);
        }
        Ok(())
    }
    /// Removes virtual copy `id`; returns its master, which is selected.
    pub(super) fn remove_virtual_copy(&mut self, id: i64) -> Result<Option<i64>> {
        let photo = self.photo(id).cloned();
        self.catalog.remove_virtual_copy(id)?;
        self.forget_previews(id);
        self.reload()?;
        let master = photo.as_ref().and_then(|p| p.master);
        if let Some(master) = master {
            self.show(master);
        }
        if let Some(p) = photo {
            self.message = format!("Removed {} of {}", p.copy_name, p.filename);
        }
        Ok(master)
    }
    fn rename_copy(&mut self, id: i64, name: &str) {
        match self.catalog.set_copy_name(id, name) {
            Ok(()) => {
                if let Some(p) = self.photos.iter_mut().find(|p| p.id == id) {
                    p.copy_name = name.trim().to_string();
                }
                self.filter();
            }
            Err(e) => self.message = format!("Copy name could not be saved: {e}"),
        }
    }
    /// Selects `id`, leaving filters that would hide it so it stays in view.
    fn show(&mut self, id: i64) {
        if !self.visible.iter().any(|i| self.photos[*i].id == id) {
            self.query.clear();
            self.rating = 0;
            self.flag = 2;
            self.label_filter = None;
            self.filter();
        }
        self.selected = Some(id);
    }
    pub fn metadata_controls(&mut self, ui: &mut egui::Ui, id: i64) -> bool {
        if let Some(photo) = self.photo(id).cloned()
            && let Some(edit) = crate::app::photo_metadata::controls(ui, &photo, &self.labels())
        {
            if let Err(e) = self.edit_metadata(id, edit, false) {
                self.message = format!("Metadata could not be saved: {e}");
            }
            return true;
        }
        false
    }
    pub fn sidebar(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::None;
        ui.spacing_mut().item_spacing.y = 0.;
        egui::ScrollArea::vertical()
            .id_salt("library-sources")
            .show(ui, |ui| {
                section(ui, "Navigator", false, |ui| {
                    let texture = self
                        .selected
                        .and_then(|id| self.photo(id))
                        .and_then(|p| self.texture(p));
                    let (rect, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), ui.available_width() * 0.66),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(rect, 0., theme::gray(22));
                    if texture.is_none() {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "No photo selected",
                            egui::FontId::proportional(11.),
                            theme::gray(95),
                        );
                    }
                    if let Some(texture) = texture {
                        let size = texture.size_vec2();
                        let scale = (rect.width() / size.x).min(rect.height() / size.y);
                        ui.painter().image(
                            texture.id(),
                            egui::Rect::from_center_size(rect.center(), size * scale),
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                            Color32::WHITE,
                        );
                    }
                });
                section(ui, "Catalog", false, |ui| {
                    let offline = self.photos.len() - self.available_count();
                    let all = self.folder_scope.is_none()
                        && self.collection.is_none()
                        && !self.only_missing;
                    if source_row(ui, "All Photographs", self.photos.len(), all).clicked() {
                        self.folder_scope = None;
                        self.selected_folder.clear();
                        self.collection = None;
                        self.only_missing = false;
                        self.filter()
                    }
                    if offline > 0
                        && source_row(ui, "Offline Photographs", offline, self.only_missing)
                            .clicked()
                    {
                        self.only_missing = !self.only_missing;
                        self.filter()
                    }
                });
                section(ui, "Folders", false, |ui| {
                    // Lightroom-style volume headers with an attached light.
                    let mut volumes: std::collections::BTreeMap<
                        crate::platform::volume::Volume,
                        Vec<(i64, String, Option<String>)>,
                    > = Default::default();
                    for root in self.roots.clone() {
                        let path = std::path::PathBuf::from(root.2.as_deref().unwrap_or(&root.1));
                        volumes
                            .entry(crate::platform::volume::volume_of(&path))
                            .or_default()
                            .push(root);
                    }
                    self.check_volumes(ui.ctx(), volumes.keys());
                    let online = self.volumes_online.lock().unwrap().clone();
                    // The startup disk first, then other drives by name.
                    let mut volumes: Vec<_> = volumes.into_iter().collect();
                    volumes.sort_by_key(|(v, _)| (v.mount.is_some(), v.name.to_lowercase()));
                    for (volume, roots) in volumes {
                        let state = online
                            .get(volume.mount.as_deref().unwrap_or(std::path::Path::new("/")))
                            .copied();
                        let attached = match &volume.mount {
                            None => Some(true),
                            Some(_) => state.map(|s| s.0),
                        };
                        let space = state.and_then(|s| s.1);
                        let photos: usize = roots
                            .iter()
                            .map(|(id, _, _)| {
                                self.folders
                                    .iter()
                                    .filter(|f| f.root == *id)
                                    .map(|f| f.count)
                                    .sum::<usize>()
                            })
                            .sum();
                        let key = format!("volume-collapsed:{}", volume.name);
                        let collapsed = self.expanded.contains(&key);
                        if volume_row(ui, &volume, attached, space, photos, !collapsed).clicked() {
                            if collapsed {
                                self.expanded.remove(&key);
                            } else {
                                self.expanded.insert(key);
                            }
                        }
                        if collapsed {
                            continue;
                        }
                        ui.add_space(2.);
                        for (root, original, mapped) in roots {
                            let root_path = mapped.as_deref().unwrap_or(&original);
                            let name = std::path::Path::new(&original)
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            let mut tree = FolderNode::root(root, name, root_path.into());
                            for f in self.folders.iter().filter(|f| f.root == root) {
                                tree.insert(f);
                            }
                            tree.finish();
                            match folder_tree_row(
                                ui,
                                &tree,
                                0,
                                &mut self.expanded,
                                &self.selected_folder,
                            ) {
                                Some(TreeAction::Select(key, ids)) => {
                                    self.selected_folder = key;
                                    self.folder_scope = Some(ids);
                                    self.collection = None;
                                    self.filter();
                                }
                                Some(TreeAction::Relink(root, id)) => {
                                    action = if root {
                                        Action::RelinkRoot(id)
                                    } else {
                                        Action::RelinkFolder(id)
                                    }
                                }
                                None => {}
                            }
                        }
                    }
                    if self.roots.is_empty() {
                        ui.add_space(4.);
                        ui.label(
                            egui::RichText::new("No folders yet")
                                .size(11.)
                                .color(theme::gray(120)),
                        );
                    }
                    ui.add_space(12.);
                    if add_row(ui, "Add Folder…")
                        .on_hover_text(
                            "Add a folder of photos to this catalog. Photos stay where they are.",
                        )
                        .clicked()
                    {
                        action = Action::AddFolder;
                    }
                });
            });
        action
    }
    /// Right panel: the selected photo's rating, flag, label and file details.
    /// Right panel: the selected photo's rating, flag, label and file details.
    /// The layout is identical with or without a selection, so nothing moves.
    pub fn info_panel(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::None;
        ui.spacing_mut().item_spacing.y = 0.;
        let photo = self.selected.and_then(|id| self.photo(id)).cloned();
        egui::ScrollArea::vertical()
            .id_salt("library-info")
            .auto_shrink(false)
            .show(ui, |ui| {
                section(ui, "Quick Develop", false, |ui| {
                    let open = ui
                        .add_enabled(
                            photo.is_some(),
                            egui::Button::new("Open in Develop")
                                .min_size(Vec2::new(ui.available_width(), 24.)),
                        )
                        .on_hover_text("Develop · D, or double-click the photo");
                    if open.clicked()
                        && let Some(p) = &photo
                    {
                        action = Action::Develop(p.id);
                    }
                    ui.add_space(4.);
                    info_text(
                        ui,
                        if photo.as_ref().is_some_and(|p| p.has_lightroom_edits) {
                            "Has Lightroom edits"
                        } else {
                            ""
                        },
                    );
                });
                section(ui, "Metadata", false, |ui| {
                    match &photo {
                        Some(p) => {
                            self.metadata_controls(ui, p.id);
                        }
                        None => {
                            ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 22.),
                                egui::Sense::hover(),
                            );
                        }
                    }
                    ui.add_space(6.);
                    let folder = photo
                        .as_ref()
                        .and_then(|p| p.path.parent()?.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let field = |f: fn(&Photo) -> &str| photo.as_ref().map_or("", f).to_string();
                    metadata_row(ui, "File Name", &field(|p| &p.filename));
                    match photo.as_ref().filter(|p| p.master.is_some()) {
                        Some(p) => self.copy_name_row(ui, p),
                        None => {
                            metadata_row(ui, "Copy Name", "");
                        }
                    }
                    for (key, value, hover) in [
                        (
                            "Folder",
                            folder,
                            photo.as_ref().map(|p| p.path.display().to_string()),
                        ),
                        ("Capture Time", field(|p| &p.captured), None),
                        ("Format", field(|p| &p.format), None),
                    ] {
                        let response = metadata_row(ui, key, &value);
                        if let Some(hover) = hover {
                            response.on_hover_text(hover);
                        } else if !value.is_empty() {
                            response.on_hover_text(value);
                        }
                    }
                });
                section(ui, "Keywording", false, |ui| {
                    info_text(
                        ui,
                        match &photo {
                            Some(p) if !p.keywords.is_empty() => &p.keywords,
                            Some(_) => "No keywords",
                            None => "",
                        },
                    );
                });
            });
        action
    }
    /// Re-checks every few seconds, on a background thread, whether external
    /// volumes are attached.
    fn check_volumes<'a>(
        &mut self,
        ctx: &egui::Context,
        volumes: impl Iterator<Item = &'a crate::platform::volume::Volume>,
    ) {
        let due = self
            .volumes_checked
            .is_none_or(|t| t.elapsed() > std::time::Duration::from_secs(3));
        if !due {
            return;
        }
        self.volumes_checked = Some(std::time::Instant::now());
        let mounts: Vec<PathBuf> = volumes
            .map(|v| v.mount.clone().unwrap_or_else(|| PathBuf::from("/")))
            .collect();
        ctx.request_repaint_after(std::time::Duration::from_secs(3));
        let ctx = ctx.clone();
        spawn_volume_check(
            &self.volumes_online,
            &self.volumes_busy,
            mounts,
            move || ctx.request_repaint(),
            |m| {
                let attached = m.is_dir();
                let space = attached
                    .then(|| crate::platform::volume::space(m))
                    .flatten();
                (attached, space)
            },
        );
    }
    /// Drain in every workspace so the bounded worker never waits for the grid.
    pub(super) fn poll_previews(&mut self, ctx: &egui::Context) {
        while let Ok(result) = self.thumb_rx.try_recv() {
            self.preview_progress.finish(&result);
            let previews::PreviewResult {
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
            if let previews::EditResult::CacheError(error) = result {
                self.preview_progress.cache_failed(error);
                continue;
            }
            self.edits_pending = self.edits_pending.saturating_sub(1);
            match result {
                previews::EditResult::Ready(id, ticket, im) => {
                    if self.edited_requested.get(&id) == Some(&ticket) {
                        self.insert_edited(ctx, id, &im);
                    }
                }
                previews::EditResult::Skipped(id, ticket) => {
                    if self.edited_requested.get(&id) == Some(&ticket) {
                        self.edited_requested.remove(&id);
                    }
                }
                previews::EditResult::Failed | previews::EditResult::CacheError(_) => {}
            }
        }
    }
    /// Hands the worker the photos shown last frame. Call once per frame.
    pub(super) fn publish_shown(&mut self) {
        let shown = std::mem::take(&mut self.edit_seen);
        *self.edit_wanted.lock().unwrap() = shown;
    }

    fn insert_thumb(&mut self, ctx: &egui::Context, path: PathBuf, im: &image::RgbImage) {
        if !self.thumbs.contains_key(&path) {
            while self.thumbs.len() >= 192 {
                let Some(old) = self.thumb_order.pop_front() else {
                    break;
                };
                self.thumbs.remove(&old);
            }
            self.thumb_order.push_back(path.clone());
        }
        self.thumbs.insert(
            path.clone(),
            ctx.load_texture(
                path.display().to_string(),
                egui::ColorImage::from_rgb(
                    [im.width() as usize, im.height() as usize],
                    im.as_raw(),
                ),
                egui::TextureOptions::LINEAR,
            ),
        );
    }
    fn insert_edited(&mut self, ctx: &egui::Context, id: i64, im: &image::RgbImage) {
        if !self.edited.contains_key(&id) {
            while self.edited.len() >= 192 {
                let Some(old) = self.edited_order.pop_front() else {
                    break;
                };
                self.edited.remove(&old);
                self.edited_requested.remove(&old);
            }
            self.edited_order.push_back(id);
        }
        self.edited.insert(
            id,
            ctx.load_texture(
                format!("edited-{id}"),
                egui::ColorImage::from_rgb(
                    [im.width() as usize, im.height() as usize],
                    im.as_raw(),
                ),
                egui::TextureOptions::LINEAR,
            ),
        );
    }
    fn ticket(&mut self) -> u64 {
        self.next_ticket += 1;
        self.next_ticket
    }
    /// Forgets a removed photo's previews, so a later photo given its id
    /// starts afresh.
    fn forget_previews(&mut self, id: i64) {
        self.edited.remove(&id);
        self.edited_order.retain(|other| *other != id);
        self.edited_requested.remove(&id);
        self.edit_seen.remove(&id);
    }
    /// The photo's preview: its edit once rendered, else the embedded one.
    fn texture(&self, photo: &Photo) -> Option<&egui::TextureHandle> {
        self.edited
            .get(&photo.id)
            .or_else(|| self.thumbs.get(&photo.path))
    }
    /// Queues the previews a shown photo needs: the embedded one until its
    /// edited one is in, and the edited one for a saved or Lightroom edit.
    fn request_previews(&mut self, photo: &Photo, ctx: &egui::Context) {
        if !self.edited.contains_key(&photo.id) {
            self.request_thumbnail(&photo.path, ctx);
        }
        self.request_edited(photo);
    }
    /// Queues an edited preview for a photo with a saved or Lightroom edit.
    fn request_edited(&mut self, photo: &Photo) {
        self.edit_seen.insert(photo.id);
        if self.edited_requested.contains_key(&photo.id) {
            return;
        }
        let ticket = self.ticket();
        self.edited_requested.insert(photo.id, ticket);
        let Ok((recipe, lightroom)) = self.catalog.edit_texts(photo.id) else {
            return;
        };
        let source = recipe
            .map(previews::EditSource::Recipe)
            .or(lightroom.map(previews::EditSource::Lightroom));
        if let Some(source) = source
            && self
                .edit_tx
                .send(previews::EditJob::Render {
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
    /// under the edit it was rendered with.
    pub(super) fn update_edited(
        &mut self,
        ctx: &egui::Context,
        id: i64,
        image: image::RgbImage,
        recipe_json: String,
    ) {
        let Some(path) = self.photo(id).map(|p| p.path.clone()) else {
            return;
        };
        let tag = previews::EditSource::Recipe(recipe_json).tag();
        // Renders still in flight are older than Develop's.
        let ticket = self.ticket();
        self.edited_requested.insert(id, ticket);
        self.insert_edited(ctx, id, &image);
        let _ = self
            .edit_tx
            .send(previews::EditJob::Store { path, tag, image });
    }
    /// The folder shown, as a tree key ("" is All Photographs).
    pub(super) fn source_key(&self) -> &str {
        &self.selected_folder
    }
    /// Shows a folder saved with `source_key` again, including its subfolders,
    /// and selects `photo` if it is in it.
    pub(super) fn restore_source(&mut self, key: &str, photo: Option<i64>) {
        if let Some(rest) = key.strip_prefix("root:") {
            let (root, relative) = rest.split_once('/').unwrap_or((rest, ""));
            if let Ok(root) = root.parse::<i64>() {
                let ids: HashSet<i64> = self
                    .folders
                    .iter()
                    .filter(|f| {
                        let path = f.relative.trim_end_matches('/');
                        f.root == root
                            && (relative.is_empty()
                                || path == relative
                                || path.starts_with(&format!("{relative}/")))
                    })
                    .map(|f| f.id)
                    .collect();
                if !ids.is_empty() {
                    self.selected_folder = key.to_string();
                    self.folder_scope = Some(ids);
                    self.collection = None;
                    // Unfold the path down to the folder.
                    let mut open = format!("root:{root}");
                    self.expanded.insert(open.clone());
                    for part in relative.split('/').filter(|p| !p.is_empty()) {
                        open = format!("{open}/{part}");
                        self.expanded.insert(open.clone());
                    }
                }
            }
        }
        self.filter();
        if let Some(id) = photo
            && self.visible.iter().any(|i| self.photos[*i].id == id)
        {
            self.selected = Some(id);
        }
    }
    /// The selected photo, or else the first one shown in the current
    /// folder or filter (which then becomes selected), as Lightroom does
    /// when switching to Develop.
    pub(super) fn selected_or_first(&mut self) -> Option<i64> {
        if self.selected.is_none() {
            self.selected = self.visible.first().map(|i| self.photos[*i].id);
        }
        self.selected
    }
    /// Whether the photo's thumbnail already shows its edit (crop included).
    pub(super) fn has_edited_thumbnail(&self, id: i64) -> bool {
        self.edited.contains_key(&id)
    }
    /// The Library's cached preview for a photo, if one is loaded.
    pub(super) fn thumbnail(&self, id: i64) -> Option<&egui::TextureHandle> {
        self.texture(self.photo(id)?)
    }
    pub(super) fn preview_progress_active(&self) -> bool {
        self.preview_progress.active() || self.edits_pending > 0
    }
    pub(super) fn preview_progress(&self, ui: &mut egui::Ui) {
        self.preview_progress.show(ui, self.edits_pending);
    }
    fn filter_bar(&mut self, ui: &mut egui::Ui) {
        use crate::app::photo_metadata::{LABELS, label_color};
        let mut changed = false;
        egui::Frame::new()
            .fill(theme::gray(38))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.;
                    ui.spacing_mut().interact_size.y = 20.;
                    let compact = ui.available_width() < 640.;
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut self.query)
                                .hint_text("Search")
                                .font(egui::FontId::proportional(12.))
                                .desired_width(if compact { 120. } else { 190. })
                                // As tall as the flag switcher beside it.
                                .min_size(Vec2::new(0., COMPACT_SEGMENT_HEIGHT))
                                .vertical_align(egui::Align::Center),
                        )
                        .on_hover_text("Search filename, keyword, capture date or label")
                        .changed();
                    ui.add_space(10.);
                    if !compact {
                        ui.label(filter_caption("Flag"));
                    }
                    changed |= segmented(
                        ui,
                        &mut self.flag,
                        &[
                            (2, "All"),
                            (1, "Picked"),
                            (0, "Unflagged"),
                            (-1, "Rejected"),
                        ],
                        if compact { 190. } else { 230. },
                    );
                    ui.add_space(10.);
                    if !compact {
                        ui.label(filter_caption("Rating ≥"));
                    }
                    for star in 1..=5 {
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(13., 20.), egui::Sense::click());
                        let lit = self.rating >= star;
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            "★",
                            egui::FontId::proportional(12.),
                            theme::gray(if lit {
                                225
                            } else if response.hovered() {
                                140
                            } else {
                                80
                            }),
                        );
                        if response
                            .on_hover_text(format!("{star} stars or more · click again to clear"))
                            .clicked()
                        {
                            self.rating = if self.rating == star { 0 } else { star };
                            changed = true;
                        }
                    }
                    ui.add_space(10.);
                    if !compact {
                        ui.label(filter_caption("Color"));
                    }
                    for label in LABELS {
                        let active = self.label_filter.as_deref() == Some(label);
                        let (rect, response) =
                            ui.allocate_exact_size(Vec2::new(15., 20.), egui::Sense::click());
                        let chip = egui::Rect::from_center_size(rect.center(), Vec2::splat(10.));
                        ui.painter()
                            .rect_filled(chip, 1., label_color(label).unwrap_or_default());
                        if active {
                            ui.painter().rect_stroke(
                                chip.expand(2.),
                                2.,
                                egui::Stroke::new(1.2, theme::gray(225)),
                                egui::StrokeKind::Outside,
                            );
                        }
                        if response.on_hover_text(label).clicked() {
                            self.label_filter = if active { None } else { Some(label.into()) };
                            changed = true;
                        }
                    }
                    let custom: Vec<_> = self
                        .labels()
                        .into_iter()
                        .filter(|l| !LABELS.contains(&l.as_str()))
                        .collect();
                    if !custom.is_empty() {
                        egui::ComboBox::from_id_salt("library-label")
                            .width(70.)
                            .selected_text(
                                self.label_filter
                                    .as_deref()
                                    .filter(|l| custom.iter().any(|c| c == l))
                                    .unwrap_or("Other"),
                            )
                            .show_ui(ui, |ui| {
                                for label in custom {
                                    changed |= ui
                                        .selectable_value(
                                            &mut self.label_filter,
                                            Some(label.clone()),
                                            label,
                                        )
                                        .changed();
                                }
                            });
                    }
                    let active = !self.query.is_empty()
                        || self.rating > 0
                        || self.flag != 2
                        || self.label_filter.is_some();
                    if active {
                        ui.add_space(10.);
                        if ui
                            .add(
                                egui::Button::new(filter_caption("Filters Off"))
                                    .small()
                                    .frame(false),
                            )
                            .on_hover_text("Clear search, flag, rating and color filters")
                            .clicked()
                        {
                            self.query.clear();
                            self.rating = 0;
                            self.flag = 2;
                            self.label_filter = None;
                            changed = true;
                        }
                    }
                });
            });
        if changed {
            self.filter()
        }
    }
    fn grid_toolbar(&mut self, ui: &mut egui::Ui) {
        egui::Frame::new()
            .fill(theme::gray(38))
            .inner_margin(egui::Margin::symmetric(10, 4))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(filter_caption("Sort"));
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(if self.reverse {
                                    "Capture Time ↓"
                                } else {
                                    "Capture Time ↑"
                                })
                                .size(11.),
                            )
                            .small()
                            .frame(false),
                        )
                        .on_hover_text("Reverse the sort order")
                        .clicked()
                    {
                        self.reverse = !self.reverse;
                        self.filter();
                    }
                    ui.add_space(12.);
                    ui.small(if self.visible.len() == self.photos.len() {
                        format!("{} photos", self.photos.len())
                    } else {
                        format!("{} of {} photos", self.visible.len(), self.photos.len())
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().slider_width = 110.;
                        ui.add(
                            egui::Slider::new(&mut self.thumb_size, 110. ..=360.).show_value(false),
                        );
                        ui.label(filter_caption("Thumbnails"));
                    });
                });
            });
    }
    fn request_thumbnail(&mut self, path: &std::path::Path, ctx: &egui::Context) {
        if !self.thumbs.contains_key(path)
            && !self.pending.contains(path)
            && !self.failed.contains(path)
            && self.thumb_tx.try_send(path.to_path_buf()).is_ok()
        {
            self.pending.insert(path.to_path_buf());
            self.preview_progress.queued();
            ctx.request_repaint();
        }
    }
    /// The selected source's name, as Lightroom shows it above the filmstrip.
    fn source_name(&self) -> String {
        if let Some(id) = self.collection {
            return self
                .collections
                .iter()
                .find(|c| c.id == id)
                .map_or_else(|| "Collection".into(), |c| c.name.clone());
        }
        if self.selected_folder.is_empty() {
            return "All Photographs".into();
        }
        match self.selected_folder.rsplit_once('/') {
            Some((_, name)) => name.into(),
            None => self
                .roots
                .iter()
                .find(|(id, _, _)| format!("root:{id}") == self.selected_folder)
                .and_then(|(_, original, _)| {
                    std::path::Path::new(original)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .unwrap_or_default(),
        }
    }
    /// Lightroom's filmstrip for Develop: the current source's photos with the
    /// open one highlighted. Returns a photo to open and whether metadata changed.
    pub fn filmstrip(&mut self, ui: &mut egui::Ui, current: i64) -> (Option<i64>, bool) {
        let mut target = None;
        let mut changed = false;
        let photo = self.photo(current).cloned();
        let position = self
            .visible
            .iter()
            .position(|i| self.photos[*i].id == current);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(10, 3))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(self.source_name())
                            .size(11.)
                            .color(theme::gray(200)),
                    );
                    ui.label(filter_caption(&match position {
                        Some(at) => format!("{} of {} photos", at + 1, self.visible.len()),
                        None => format!("{} photos", self.visible.len()),
                    }));
                    if let Some(p) = &photo {
                        ui.label(filter_caption(&format!(
                            "{}{}",
                            p.filename,
                            cell::copy_suffix(p)
                        )));
                    }
                    if photo.is_some() {
                        ui.add_space((ui.available_width() - 250.).max(8.));
                        changed = self.metadata_controls(ui, current);
                    }
                });
            });
        let height = ui.available_height().max(40.);
        egui::ScrollArea::horizontal()
            .id_salt("develop-filmstrip")
            .auto_shrink(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.;
                    for n in 0..self.visible.len() {
                        let p = self.photos[self.visible[n]].clone();
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(height * 1.25, height),
                            egui::Sense::click(),
                        );
                        let active = p.id == current;
                        // Scroll only to bring the open photo into view: a photo
                        // already visible, e.g. one just clicked, stays put.
                        if active && self.strip_current != Some(current) {
                            if !ui.clip_rect().contains_rect(rect) {
                                response.scroll_to_me(None);
                            }
                            self.strip_current = Some(current);
                        }
                        if !ui.is_rect_visible(rect) {
                            continue;
                        }
                        self.request_previews(&p, ui.ctx());
                        let cell = rect.shrink(2.);
                        let base = theme::gray(if active {
                            120
                        } else if response.hovered() {
                            58
                        } else {
                            40
                        });
                        // Same cues as the grid: the label tints the cell, and
                        // flag and stars sit on a strip below the photo.
                        let fill = crate::app::photo_metadata::label_color(&p.label)
                            .map_or(base, |label| {
                                base.lerp_to_gamma(label, if active { 0.35 } else { 0.25 })
                            });
                        ui.painter().rect_filled(cell, 2., fill);
                        let strip = 14.;
                        if let Some(texture) = self.texture(&p) {
                            let area = egui::Rect::from_min_max(
                                cell.min + Vec2::splat(5.),
                                cell.max - Vec2::new(5., strip + 2.),
                            );
                            let size = texture.size_vec2();
                            let scale = (area.width() / size.x).min(area.height() / size.y);
                            let image = egui::Rect::from_center_size(area.center(), size * scale);
                            ui.painter().image(
                                texture.id(),
                                image,
                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.)),
                                Color32::WHITE,
                            );
                            if p.master.is_some() {
                                cell::copy_badge(ui.painter(), image, fill);
                            }
                        }
                        let y = cell.bottom() - strip / 2. - 2.;
                        let mut x = cell.left() + 6.;
                        if p.flag != 0 {
                            crate::app::photo_metadata::flag_icon(
                                ui.painter(),
                                egui::pos2(x + 4., y),
                                p.flag,
                                active,
                            );
                            x += 13.;
                        }
                        if p.rating > 0 {
                            ui.painter().text(
                                egui::pos2(x, y),
                                egui::Align2::LEFT_CENTER,
                                "★".repeat(p.rating as usize),
                                egui::FontId::proportional(9.),
                                theme::gray(if active { 30 } else { 200 }),
                            );
                        }
                        if let Some(menu) = cell::photo_menu(&response, &p) {
                            let is_edit = matches!(menu, cell::PhotoAction::Edit(_));
                            if let Some(id) = self.photo_action(ui.ctx(), &p, menu) {
                                target = Some(id).filter(|id| *id != current);
                            }
                            if is_edit {
                                // Filters may have changed the visible list.
                                changed = true;
                                break;
                            }
                        }
                        let context = crate::app::widgets::context_clicked(&response);
                        if response
                            .on_hover_text(format!("{}{}", p.filename, cell::copy_suffix(&p)))
                            .clicked()
                            && !active
                            && !context
                        {
                            target = Some(p.id);
                        }
                    }
                });
            });
        (target, changed)
    }
    pub fn grid(&mut self, ui: &mut egui::Ui) -> Action {
        self.poll_previews(ui.ctx());
        let mut action = Action::None;
        self.filter_bar(ui);
        egui::Panel::bottom("library-grid-toolbar")
            .frame(egui::Frame::new())
            .show_separator_line(false)
            .show(ui, |ui| self.grid_toolbar(ui));
        let columns = ((ui.available_width() / self.thumb_size).floor() as usize).max(1);
        let width = (ui.available_width() / columns as f32).floor().max(80.);
        let mut metadata_edit = None;
        let spacing = ui.spacing().item_spacing;
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        egui::Frame::new().fill(theme::gray(44)).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("library-grid")
                .auto_shrink(false)
                .show_rows(
                    ui,
                    width,
                    self.visible.len().div_ceil(columns),
                    |ui, rows| {
                        for row in rows {
                            ui.horizontal(|ui| {
                                for col in 0..columns {
                                    let Some(&index) = self.visible.get(row * columns + col) else {
                                        break;
                                    };
                                    let p = self.photos[index].clone();
                                    let exists = self.available.contains(&p.path);
                                    self.request_previews(&p, ui.ctx());
                                    let (response, edit) = photo_cell(
                                        ui,
                                        &p,
                                        self.texture(&p),
                                        self.selected == Some(p.id),
                                        row * columns + col + 1,
                                        exists,
                                        width,
                                    );
                                    if response.clicked() || response.secondary_clicked() {
                                        self.selected = Some(p.id);
                                    }
                                    if response.double_clicked() {
                                        self.selected = Some(p.id);
                                        action = Action::Develop(p.id);
                                    }
                                    if let Some(edit) = edit {
                                        metadata_edit = Some((p.clone(), edit));
                                    }
                                }
                            });
                        }
                    },
                );
        });
        ui.spacing_mut().item_spacing = spacing;
        if let Some((photo, menu)) = metadata_edit
            && let Some(id) = self.photo_action(ui.ctx(), &photo, menu)
        {
            action = Action::Develop(id);
        }
        action
    }
    /// Carries out a thumbnail menu choice; returns a photo to open in Develop.
    fn photo_action(
        &mut self,
        ctx: &egui::Context,
        photo: &Photo,
        action: cell::PhotoAction,
    ) -> Option<i64> {
        use cell::PhotoAction;
        match action {
            PhotoAction::Develop => return Some(photo.id),
            PhotoAction::Reveal => {
                if let Err(e) = crate::platform::reveal::reveal(&photo.path) {
                    self.message = format!("Could not show {}: {e}", photo.filename);
                }
            }
            PhotoAction::CopyPath => {
                ctx.copy_text(photo.path.display().to_string());
                self.message = format!("Copied {}", photo.path.display());
            }
            PhotoAction::Edit(edit) => {
                if let Err(e) = self.edit_metadata(photo.id, edit, false) {
                    self.message = format!("Metadata could not be saved: {e}");
                }
            }
            PhotoAction::Copy(copy) => self.copy_request = Some(copy),
        }
        None
    }
}
impl Library {
    /// The Copy Name row of a virtual copy, editable in place like
    /// Lightroom's Metadata panel; saved on Return or when focus leaves.
    fn copy_name_row(&mut self, ui: &mut egui::Ui, photo: &Photo) {
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.), egui::Sense::hover());
        ui.painter().text(
            egui::pos2(rect.left() + 84., rect.center().y),
            egui::Align2::RIGHT_CENTER,
            "Copy Name",
            egui::FontId::proportional(11.),
            theme::gray(135),
        );
        if self
            .copy_name
            .as_ref()
            .is_none_or(|(id, _)| *id != photo.id)
        {
            self.copy_name = Some((photo.id, photo.copy_name.clone()));
        }
        let Some((_, text)) = &mut self.copy_name else {
            return;
        };
        let field = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 88., rect.top() + 1.),
            egui::pos2(rect.right(), rect.bottom() - 1.),
        );
        let response = ui.put(
            field,
            egui::TextEdit::singleline(text)
                .font(egui::FontId::proportional(11.))
                .text_color(theme::gray(205))
                .margin(egui::Margin::symmetric(4, 1))
                .vertical_align(egui::Align::Center),
        );
        if response.lost_focus() {
            self.commit_copy_name();
        }
    }
    /// Saves a Copy Name still being typed, e.g. when the Library panel
    /// goes away before the field loses focus.
    pub(super) fn commit_copy_name(&mut self) {
        let Some((id, text)) = &self.copy_name else {
            return;
        };
        let (id, name) = (*id, text.trim().to_string());
        if self
            .photo(id)
            .is_some_and(|p| p.master.is_some() && p.copy_name != name)
        {
            self.rename_copy(id, &name);
        }
        self.copy_name = Some((id, name));
    }
}
/// A fixed-height metadata row: caption column, then the truncated value
/// (a dash when empty), so the panel never widens or reflows.
fn metadata_row(ui: &mut egui::Ui, key: &str, value: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.), egui::Sense::hover());
    let y = rect.center().y;
    ui.painter().text(
        egui::pos2(rect.left() + 84., y),
        egui::Align2::RIGHT_CENTER,
        key,
        egui::FontId::proportional(11.),
        theme::gray(135),
    );
    let left = rect.left() + 92.;
    let galley = egui::WidgetText::from(if value.is_empty() { "—" } else { value }).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        (rect.right() - left).max(1.),
        egui::FontId::proportional(11.),
    );
    ui.painter().galley(
        egui::pos2(left, y - galley.size().y / 2.),
        galley,
        theme::gray(if value.is_empty() { 90 } else { 205 }),
    );
    response
}
/// One truncated line of secondary text at a fixed height.
fn info_text(ui: &mut egui::Ui, text: &str) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.), egui::Sense::hover());
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        rect.width().max(1.),
        egui::FontId::proportional(11.),
    );
    ui.painter().galley(
        egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.),
        galley,
        theme::gray(150),
    );
}
fn filter_caption(text: &str) -> egui::RichText {
    egui::RichText::new(text).size(11.).color(theme::gray(150))
}
type VolumeState = (bool, Option<(u64, u64)>);
/// Probes `mounts` on a background thread into `online`, calling `changed` when the
/// result differs, unless the previous check is still running: probing a stalled
/// mount can hang, and new threads would pile up behind it. Returns whether a check
/// started.
fn spawn_volume_check(
    online: &std::sync::Arc<std::sync::Mutex<HashMap<PathBuf, VolumeState>>>,
    busy: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    mounts: Vec<PathBuf>,
    changed: impl FnOnce() + Send + 'static,
    probe: impl Fn(&std::path::Path) -> VolumeState + Send + 'static,
) -> bool {
    use std::sync::atomic::Ordering;
    if busy.swap(true, Ordering::Acquire) {
        return false;
    }
    /// Clears the flag however the check ends.
    struct Done(std::sync::Arc<std::sync::atomic::AtomicBool>);
    impl Drop for Done {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let done = Done(busy.clone());
    let online = online.clone();
    std::thread::spawn(move || {
        let _done = done;
        let state: HashMap<PathBuf, VolumeState> = mounts
            .into_iter()
            .map(|m| {
                let state = probe(&m);
                (m, state)
            })
            .collect();
        let mut shared = online.lock().unwrap();
        if *shared != state {
            *shared = state;
            changed();
        }
    });
    true
}
/// A Lightroom volume header bar: an LED lit green when the drive is
/// attached, the drive name, free / total space (or Offline), and a
/// disclosure arrow that folds its folders away.
fn volume_row(
    ui: &mut egui::Ui,
    volume: &crate::platform::volume::Volume,
    attached: Option<bool>,
    space: Option<(u64, u64)>,
    photos: usize,
    open: bool,
) -> egui::Response {
    ui.add_space(4.);
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.), egui::Sense::click());
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        3.,
        theme::gray(if response.hovered() { 64 } else { 56 }),
    );
    let y = rect.center().y;
    let led = egui::Rect::from_center_size(egui::pos2(rect.left() + 14., y), Vec2::new(5., 11.));
    if attached == Some(true) {
        painter.rect_filled(led, 1., Color32::from_rgb(110, 200, 90));
    } else {
        painter.rect_filled(led, 1., theme::gray(26));
        painter.rect_stroke(
            led,
            1.,
            egui::Stroke::new(
                1.,
                theme::gray(if attached == Some(false) { 150 } else { 90 }),
            ),
            egui::StrokeKind::Inside,
        );
    }
    painter.text(
        egui::pos2(rect.left() + 26., y),
        egui::Align2::LEFT_CENTER,
        &volume.name,
        egui::FontId::proportional(12.5),
        theme::gray(225),
    );
    let gb = |bytes: u64| bytes as f64 / 1e9;
    let detail = match (attached, space) {
        (Some(false), _) => "Offline".to_string(),
        (_, Some((free, total))) => format!("{:.0} / {:.0} GB", gb(free), gb(total)),
        _ => String::new(),
    };
    painter.text(
        egui::pos2(rect.right() - 26., y),
        egui::Align2::RIGHT_CENTER,
        detail,
        egui::FontId::proportional(11.),
        theme::gray(160),
    );
    let c = egui::pos2(rect.right() - 13., y);
    let arrow = if open {
        vec![
            c + Vec2::new(-4., -2.),
            c + Vec2::new(4., -2.),
            c + Vec2::new(0., 3.),
        ]
    } else {
        vec![
            c + Vec2::new(3., -4.),
            c + Vec2::new(3., 4.),
            c + Vec2::new(-3., 0.),
        ]
    };
    painter.add(egui::Shape::convex_polygon(
        arrow,
        theme::gray(200),
        egui::Stroke::NONE,
    ));
    response.on_hover_text(match (&volume.mount, attached) {
        (None, _) => format!("Startup disk · {photos} photos"),
        (Some(mount), Some(false)) => format!("{} is not attached", mount.display()),
        (Some(mount), _) => format!("{} · {photos} photos", mount.display()),
    })
}
/// A quiet full-width "+ label" row, Lightroom's add action in a panel.
fn add_row(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.), egui::Sense::click());
    let hovered = response.hovered();
    if hovered {
        ui.painter().rect_filled(rect, 3., theme::gray(43));
    }
    let color = theme::gray(if hovered { 235 } else { 165 });
    // Same columns as folder rows: icon at 10 px, text at 29 px.
    let c = egui::pos2(rect.left() + 16., rect.center().y);
    crate::app::icons::paint_at(ui.painter(), crate::app::icons::Icon::Add, c, 13., color);
    ui.painter().text(
        egui::pos2(rect.left() + 29., rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.),
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
/// A Catalog panel row: name on the left, photo count right-aligned.
fn source_row(ui: &mut egui::Ui, name: &str, count: usize, active: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.), egui::Sense::click());
    if active || response.hovered() {
        ui.painter().rect_filled(
            rect,
            2.,
            if active {
                theme::selected_row()
            } else {
                theme::gray(43)
            },
        );
    }
    let y = rect.center().y;
    ui.painter().text(
        egui::pos2(rect.left() + 10., y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.),
        theme::gray(if active { 235 } else { 190 }),
    );
    ui.painter().text(
        egui::pos2(rect.right() - 10., y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::proportional(11.),
        theme::gray(125),
    );
    response
}
/// Lightroom-style grid cells: the label tints the cell, while selection uses
/// a lighter surround instead of the app's blue button fill.
mod cell;
mod previews;
mod thumbnails;
mod tree;
use cell::photo_cell;
use thumbnails::{available_paths, thumbnail};
use tree::{FolderNode, TreeAction, folder_tree_row};
#[cfg(test)]
mod tests;
