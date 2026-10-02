//! Catalog browsing; thumbnail work is bounded and independent of RAW development.
use super::widgets::{COMPACT_SEGMENT_HEIGHT, section, segmented};
use crate::app::theme;
use crate::catalog::{Catalog, Collection, CollectionKind, Folder, Photo};
use anyhow::Result;
use eframe::egui::{self, Color32, Vec2};
use std::collections::{HashMap, HashSet};

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
    /// Each collection's photos, limited to the ones the Library shows.
    collection_photos: HashMap<i64, HashSet<i64>>,
    roots: Vec<(i64, String, Option<String>)>,
    volumes: volumes::Volumes,
    /// The source and filter bar; `visible` is their result.
    filters: filter::Filters,
    /// The folder shown, as a tree key ("" is All Photographs).
    selected_folder: String,
    expanded: HashSet<String>,
    thumb_size: f32,
    strip_current: Option<i64>,
    /// Indices into `photos` of the ones shown, in display order.
    visible: Vec<usize>,
    availability: availability::Availability,
    ctx: egui::Context,
    cache: textures::PreviewTextures,
    /// A virtual copy command from a thumbnail menu, for the editor.
    copy_request: Option<CopyAction>,
    copy_names: copy_name::CopyNames,
    /// Reads capture times for photos added from folders.
    capture: Option<capture::Backfill>,
    /// Photos the capture-time backfill tried since the last online check.
    capture_tried: HashSet<i64>,
    /// A photo to keep in place in the grid after a re-sort, with its
    /// position before it.
    keep_in_place: Option<(i64, usize)>,
    /// The grid's scroll offset last frame.
    grid_offset: f32,
    /// Positions in `visible` the grid drew last frame; all until it is drawn.
    grid_shown: std::ops::Range<usize>,
    /// External volumes attached at the last check, to notice one returning.
    attached: HashSet<std::path::PathBuf>,
    pub message: String,
}
impl Library {
    pub fn load(path: &std::path::Path, ctx: egui::Context) -> Result<Self> {
        crate::platform::network::prepare_filesystem_bridge();
        let mut catalog = Catalog::open(path)?;
        // Catalogs imported before history was kept: recover it from the
        // stored Lightroom catalog. Best effort; a failure only hides history.
        let _ = catalog.backfill_lightroom_history();
        let mut s = Self {
            catalog,
            photos: Vec::new(),
            selected: None,
            folders: Vec::new(),
            collections: Vec::new(),
            collection_photos: HashMap::new(),
            roots: Vec::new(),
            volumes: Default::default(),
            filters: Default::default(),
            selected_folder: String::new(),
            expanded: HashSet::new(),
            thumb_size: 190.,
            strip_current: None,
            visible: Vec::new(),
            availability: Default::default(),
            cache: textures::PreviewTextures::new(&ctx),
            ctx,
            copy_request: None,
            copy_names: Default::default(),
            capture: None,
            capture_tried: HashSet::new(),
            keep_in_place: None,
            grid_offset: 0.,
            grid_shown: 0..usize::MAX,
            attached: HashSet::new(),
            message: String::new(),
        };
        s.refresh()?;
        // Start with a selection, as Lightroom does, so the side panels are filled.
        s.selected = s.visible.first().map(|i| s.photos[*i].id);
        Ok(s)
    }
    pub fn refresh(&mut self) -> Result<()> {
        self.reload()?;
        self.availability.start(&self.photos, &self.ctx);
        self.cache.failed.clear();
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
        let ids: HashSet<i64> = self.photos.iter().map(|p| p.id).collect();
        self.collection_photos = self.catalog.collection_photos()?;
        for members in self.collection_photos.values_mut() {
            members.retain(|id| ids.contains(id));
        }
        if let Some(id) = self.filters.collection {
            if self.collections.iter().any(|c| c.id == id) {
                self.filters.members = self.collection_photos.get(&id).cloned().unwrap_or_default();
            } else {
                self.filters.collection = None;
                self.filters.members.clear();
            }
        }
        self.roots = self.catalog.roots()?;
        // Copy commands save a name being typed before they run.
        self.copy_names.clear();
        self.filter();
        Ok(())
    }
    /// Waits for the online check, for callers that report on it.
    pub fn wait_for_availability(&mut self) {
        if self.availability.poll(true, &self.photos) {
            self.availability_known();
        }
    }
    /// Once it is known which photos are online, filters again and reads the
    /// capture times still missing, including ones that were offline before.
    fn availability_known(&mut self) {
        self.filter();
        self.capture_tried.clear();
        self.start_capture_times();
    }
    fn start_capture_times(&mut self) {
        if self.capture.is_some() {
            return;
        }
        let todo: Vec<_> = self
            .photos
            .iter()
            .filter(|p| {
                p.captured.is_empty()
                    && p.master.is_none()
                    && !self.capture_tried.contains(&p.id)
                    && capture::readable(&p.path)
                    && self.is_available(&p.path)
            })
            .map(|p| (p.id, p.path.clone()))
            .collect();
        if !todo.is_empty() {
            self.capture_tried.extend(todo.iter().map(|(id, _)| *id));
            self.capture = Some(capture::Backfill::start(todo, &self.ctx));
        }
    }
    /// Saves the capture times read so far and sorts the photos again.
    fn poll_capture_times(&mut self) {
        let Some(backfill) = &self.capture else {
            return;
        };
        let (read, done) = backfill.poll();
        if done {
            self.capture = None;
        }
        let dated: Vec<(i64, String)> = read
            .into_iter()
            .filter_map(|(id, read)| match read {
                capture::Read::Dated(time) => Some((id, time)),
                _ => None,
            })
            .collect();
        if !dated.is_empty() {
            match self.catalog.fill_capture_times(&dated) {
                Ok(()) => self.apply_capture_times(&dated),
                Err(e) => self.message = format!("Capture times could not be saved: {e}"),
            }
        }
        if done {
            // Photos added while it ran.
            self.start_capture_times();
        }
    }
    /// Re-sorts after capture times were filled in, as the catalog orders
    /// photos, keeping the selected photo selected and where it was on screen.
    fn apply_capture_times(&mut self, times: &[(i64, String)]) {
        let times: HashMap<i64, &String> = times.iter().map(|(id, t)| (*id, t)).collect();
        for photo in &mut self.photos {
            if photo.captured.is_empty()
                && let Some(time) = times
                    .get(&photo.id)
                    .or_else(|| photo.master.and_then(|m| times.get(&m)))
            {
                photo.captured = (*time).clone();
            }
        }
        let anchor = self.selected.and_then(|id| {
            self.visible
                .iter()
                .position(|i| self.photos[*i].id == id)
                .map(|at| (id, at))
        });
        // A selected photo scrolled out of view is no anchor: the view stays.
        let anchor = anchor.filter(|(_, at)| self.grid_shown.contains(at));
        self.photos.sort_by(|a, b| {
            (&a.captured, &a.filename, a.id).cmp(&(&b.captured, &b.filename, b.id))
        });
        self.filter();
        // Several batches before the grid is drawn again: the first position counts.
        if self.keep_in_place.is_none() {
            self.keep_in_place = anchor;
        }
    }
    /// Checks again which photos are online when an external volume comes
    /// back, so they show and their capture times are read.
    fn volumes_checked(&mut self, online: &HashMap<std::path::PathBuf, volumes::VolumeState>) {
        let attached: HashSet<_> = online
            .iter()
            .filter(|(_, (on, _))| *on)
            .map(|(mount, _)| mount.clone())
            .collect();
        let returned = online
            .iter()
            .any(|(mount, (on, _))| *on && !self.attached.contains(mount))
            && !self.attached.is_empty();
        self.attached = attached;
        if returned {
            self.availability.start(&self.photos, &self.ctx);
        }
    }
    fn is_available(&self, path: &std::path::Path) -> bool {
        self.availability.is_available(path)
    }
    pub fn available_count(&self) -> usize {
        self.availability.count(&self.photos)
    }
    fn filter(&mut self) {
        self.visible = self
            .filters
            .visible(&self.photos, |path| self.availability.is_available(path));
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
        self.cache.forget(id);
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
    /// Selects `id`, leaving filters that would hide it so it stays in view.
    fn show(&mut self, id: i64) {
        if !self.visible.iter().any(|i| self.photos[*i].id == id) {
            if !self.filters.members.contains(&id) {
                self.filters.collection = None;
            }
            self.filters.clear_bar();
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
                    let all = self.filters.folder_scope.is_none()
                        && self.filters.collection.is_none()
                        && !self.filters.only_missing;
                    if source_row(ui, "All Photographs", self.photos.len(), all).clicked() {
                        self.filters.folder_scope = None;
                        self.selected_folder.clear();
                        self.filters.collection = None;
                        self.filters.only_missing = false;
                        self.filter()
                    }
                    if offline > 0
                        && source_row(
                            ui,
                            "Offline Photographs",
                            offline,
                            self.filters.only_missing,
                        )
                        .clicked()
                    {
                        self.filters.only_missing = !self.filters.only_missing;
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
                    self.volumes.check(ui.ctx(), volumes.keys());
                    let online = self.volumes.snapshot();
                    self.volumes_checked(&online);
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
                        if volumes::volume_row(ui, &volume, attached, space, photos, !collapsed)
                            .clicked()
                        {
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
                                    self.filters.folder_scope = Some(ids);
                                    self.filters.collection = None;
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
                section(ui, "Collections", false, |ui| {
                    let tree = collections::tree(&self.collections, &self.collection_photos);
                    for node in &tree {
                        if let Some(id) = collections::collection_row(
                            ui,
                            node,
                            0,
                            &mut self.expanded,
                            self.filters.collection,
                        ) {
                            self.select_collection(id);
                        }
                    }
                    if tree.is_empty() {
                        ui.add_space(4.);
                        ui.label(
                            egui::RichText::new("No collections")
                                .size(11.)
                                .color(theme::gray(120)),
                        );
                    }
                });
            });
        action
    }
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
                        Some(p) => {
                            match self.copy_names.row(ui, p, &self.catalog, &mut self.photos) {
                                Ok(true) => self.filter(),
                                Ok(false) => {}
                                Err(e) => {
                                    self.message = format!("Copy name could not be saved: {e}")
                                }
                            }
                        }
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
    /// Drain in every workspace so the bounded worker never waits for the grid.
    pub(super) fn poll_previews(&mut self, ctx: &egui::Context) {
        if self.availability.poll(false, &self.photos) {
            self.availability_known();
        }
        self.poll_capture_times();
        self.cache.poll(ctx);
    }
    /// Hands the worker the photos shown last frame. Call once per frame.
    pub(super) fn publish_shown(&mut self) {
        self.cache.publish_shown();
    }
    /// The photo's preview: its edit once rendered, else the embedded one.
    fn texture(&self, photo: &Photo) -> Option<&egui::TextureHandle> {
        self.cache.texture(photo)
    }
    /// Queues the previews a shown photo needs; its edit comes from the catalog.
    fn request_previews(&mut self, photo: &Photo, ctx: &egui::Context) {
        let catalog = &self.catalog;
        self.cache.request(photo, ctx, || {
            let (recipe, lightroom) = catalog.edit_texts(photo.id).ok()?;
            recipe
                .map(previews::EditSource::Recipe)
                .or(lightroom.map(previews::EditSource::Lightroom))
        });
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
        self.cache.store_edited(ctx, id, path, image, recipe_json);
    }
    /// Shows collection `id`'s photos; the filter bar still applies.
    fn select_collection(&mut self, id: i64) {
        self.filters.collection = Some(id);
        self.filters.members = self.collection_photos.get(&id).cloned().unwrap_or_default();
        self.filters.folder_scope = None;
        self.filters.only_missing = false;
        self.selected_folder.clear();
        self.filter();
    }
    /// The source shown: a folder tree key ("" is All Photographs) or
    /// `collection:<id>`.
    pub(super) fn source_key(&self) -> String {
        match self.filters.collection {
            Some(id) => format!("collection:{id}"),
            None => self.selected_folder.clone(),
        }
    }
    /// Shows a folder saved with `source_key` again, including its subfolders,
    /// and selects `photo` if it is in it.
    pub(super) fn restore_source(&mut self, key: &str, photo: Option<i64>) {
        if let Some(id) = key
            .strip_prefix("collection:")
            .and_then(|id| id.parse::<i64>().ok())
            && self
                .collections
                .iter()
                .any(|c| c.id == id && c.kind == CollectionKind::Collection)
        {
            self.select_collection(id);
        }
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
                    self.filters.folder_scope = Some(ids);
                    self.filters.collection = None;
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
        self.cache.has_edited(id)
    }
    /// The Library's cached preview for a photo, if one is loaded.
    pub(super) fn thumbnail(&self, id: i64) -> Option<&egui::TextureHandle> {
        self.texture(self.photo(id)?)
    }
    pub(super) fn preview_progress_active(&self) -> bool {
        self.cache.progress_active()
    }
    pub(super) fn preview_progress(&self, ui: &mut egui::Ui) {
        self.cache.show_progress(ui);
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
                            egui::TextEdit::singleline(&mut self.filters.query)
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
                        &mut self.filters.flag,
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
                        let lit = self.filters.rating >= star;
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
                            self.filters.rating =
                                if self.filters.rating == star { 0 } else { star };
                            changed = true;
                        }
                    }
                    ui.add_space(10.);
                    if !compact {
                        ui.label(filter_caption("Color"));
                    }
                    for label in LABELS {
                        let active = self.filters.label_filter.as_deref() == Some(label);
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
                            self.filters.label_filter =
                                if active { None } else { Some(label.into()) };
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
                                self.filters
                                    .label_filter
                                    .as_deref()
                                    .filter(|l| custom.iter().any(|c| c == l))
                                    .unwrap_or("Other"),
                            )
                            .show_ui(ui, |ui| {
                                for label in custom {
                                    changed |= ui
                                        .selectable_value(
                                            &mut self.filters.label_filter,
                                            Some(label.clone()),
                                            label,
                                        )
                                        .changed();
                                }
                            });
                    }
                    let active = self.filters.bar_active();
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
                            self.filters.clear_bar();
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
                                egui::RichText::new(if self.filters.reverse {
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
                        self.filters.reverse = !self.filters.reverse;
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
    /// The selected source's name, as Lightroom shows it above the filmstrip.
    fn source_name(&self) -> String {
        if let Some(id) = self.filters.collection {
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
        let mut scroll = egui::ScrollArea::vertical()
            .id_salt("library-grid")
            .auto_shrink(false);
        // A re-sort moved the selected photo: scroll by the rows it moved.
        if let Some((id, before)) = self.keep_in_place.take()
            && let Some(after) = self.visible.iter().position(|i| self.photos[*i].id == id)
        {
            let rows = (after / columns) as f32 - (before / columns) as f32;
            scroll = scroll.vertical_scroll_offset((self.grid_offset + rows * width).max(0.));
        }
        egui::Frame::new().fill(theme::gray(44)).show(ui, |ui| {
            let output = scroll.show_rows(
                ui,
                width,
                self.visible.len().div_ceil(columns),
                |ui, rows| {
                    self.grid_shown = rows.start * columns..rows.end * columns;
                    for row in rows {
                        ui.horizontal(|ui| {
                            for col in 0..columns {
                                let Some(&index) = self.visible.get(row * columns + col) else {
                                    break;
                                };
                                let p = self.photos[index].clone();
                                let exists = self.is_available(&p.path);
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
            self.grid_offset = output.state.offset.y;
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
    /// Saves a Copy Name still being typed, e.g. when the Library panel
    /// goes away before the field loses focus. On failure the name stays
    /// pending, to be saved again or discarded.
    pub(super) fn commit_copy_name(&mut self) -> Result<()> {
        if self.copy_names.commit(&self.catalog, &mut self.photos)? {
            self.filter();
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn set_copy_name_draft(&mut self, id: i64, name: &str) {
        self.copy_names.draft = Some((id, name.into()));
    }
    /// Drops a Copy Name that could not be saved, e.g. closing without saving.
    pub(super) fn discard_copy_name(&mut self) {
        self.copy_names.discard();
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
mod availability;
mod capture;
mod cell;
mod collections;
mod copy_name;
mod filter;
mod previews;
mod textures;
mod thumbnails;
mod tree;
mod volumes;
use cell::photo_cell;
use thumbnails::thumbnail;
use tree::{FolderNode, TreeAction, folder_tree_row};
#[cfg(test)]
mod tests;
