//! Catalog browsing; thumbnail work is bounded and independent of RAW development.
use crate::catalog::{Catalog, Collection, Folder, Photo};
use anyhow::Result;
use eframe::egui;
use std::collections::{HashMap, HashSet};

pub enum Action {
    None,
    Develop(i64),
    RelinkRoot(i64),
    RelinkFolder(i64),
    AddFolder,
}
/// Where the Library was: its source, filter bar and selection, for undo to
/// return to.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    filters: filter::Filters,
    folder: String,
    selection: selection::Selection,
}
pub use filmstrip::Pick;
pub use metadata::{Metadata, MetadataCommand};
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
    /// The active photo and the photos selected with it.
    selection: selection::Selection,
    /// Grid columns last frame, for Up and Down.
    grid_columns: usize,
    /// Scroll the grid to the active photo, after a key moved it.
    scroll_to_active: bool,
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
    loupe: loupe::Loupe,
    /// Which way the Loupe last moved, so the photo after is prepared ahead.
    loupe_direction: i32,
    /// Metadata changes not yet handed to the shared undo log.
    done: Vec<MetadataCommand>,
    /// Reads capture times for photos added from folders.
    capture: Option<background::Reader<capture::Read>>,
    /// Reads camera settings and sizes for photos added from folders.
    info_reader: Option<background::Reader<Option<Option<crate::catalog::PhotoInfo>>>>,
    /// The Loupe's Info overlay.
    loupe_info: photo_info::Overlay,
    /// The active photo's info, as last read from the catalog.
    info: Option<(i64, Option<crate::catalog::PhotoInfo>)>,
    /// Photos the capture-time backfill tried since the last online check.
    capture_tried: HashSet<i64>,
    /// A photo to keep in place in the grid after a re-sort, with its
    /// position before it.
    keep_in_place: Option<(i64, usize)>,
    /// The grid's scroll offset last frame.
    grid_offset: f32,
    /// Positions in `visible` the grid drew last frame; all until it is drawn.
    grid_shown: std::ops::Range<usize>,
    /// External volumes attached at the last check, to notice one returning;
    /// None before the first check.
    attached: Option<HashSet<std::path::PathBuf>>,
    pub message: String,
}
impl Library {
    pub fn load(path: &std::path::Path, ctx: egui::Context) -> Result<Self> {
        crate::platform::network::prepare_filesystem_bridge();
        let mut catalog = Catalog::open(path)?;
        // Catalogs imported before history was kept: recover it from the
        // stored Lightroom catalog. Best effort; a failure only hides history.
        let _ = catalog.backfill_lightroom_history();
        let _ = catalog.backfill_lightroom_info();
        let loupe = loupe::Loupe::new(&ctx);
        let mut s = Self {
            catalog,
            photos: Vec::new(),
            selection: Default::default(),
            grid_columns: 1,
            scroll_to_active: false,
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
            done: Vec::new(),
            loupe,
            loupe_direction: 1,
            capture: None,
            info_reader: None,
            info: None,
            loupe_info: Default::default(),
            capture_tried: HashSet::new(),
            keep_in_place: None,
            grid_offset: 0.,
            grid_shown: 0..usize::MAX,
            attached: None,
            message: String::new(),
        };
        s.refresh()?;
        // Start with a selection, as Lightroom does, so the side panels are filled.
        s.select(s.visible.first().map(|i| s.photos[*i].id));
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
    /// Checks again which photos are online when an external volume comes
    /// back, so they show and their capture times are read.
    fn volumes_checked(&mut self, online: &HashMap<std::path::PathBuf, volumes::VolumeState>) {
        // Nothing to compare with until the first check has answered.
        if online.is_empty() {
            return;
        }
        let attached: HashSet<_> = online
            .iter()
            .filter(|(_, (on, _))| *on)
            .map(|(mount, _)| mount.clone())
            .collect();
        // The first answer may come during or after the online check (the
        // sidebar starts the volume check), so that check is made again
        // unless it finished with every photo online.
        let returned = match &self.attached {
            Some(before) => attached.iter().any(|mount| !before.contains(mount)),
            None => {
                self.availability.checking()
                    || self.photos.iter().any(|p| !self.is_available(&p.path))
            }
        };
        self.attached = Some(attached);
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
        self.keep_shown_selected();
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
    #[cfg(test)]
    pub(in crate::app) fn show_unflagged(&mut self) {
        self.filters.flag = 0;
        self.filter();
    }
    #[cfg(test)]
    pub(in crate::app) fn select_range_to(&mut self, id: i64) {
        self.click(id, egui::Modifiers::SHIFT);
    }
    #[cfg(test)]
    pub(in crate::app) fn shown(&self) -> Vec<i64> {
        self.visible.iter().map(|i| self.photos[*i].id).collect()
    }
    #[cfg(test)]
    pub(in crate::app) fn selected_photos(&self) -> Vec<i64> {
        self.selected_ids()
    }
    pub(in crate::app) fn place(&self) -> Place {
        Place {
            filters: self.filters.clone(),
            folder: self.selected_folder.clone(),
            selection: self.selection.clone(),
        }
    }
    /// Returns to `place`, with the photos it had selected that are still
    /// there, and scrolls to its active photo.
    pub(in crate::app) fn go_to_place(&mut self, place: &Place) {
        self.filters = place.filters.clone();
        self.selected_folder = place.folder.clone();
        // The source as the catalog has it now, e.g. after a folder gained
        // subfolders since.
        if let Some(scope) = self.folder_scope(&place.folder) {
            self.filters.folder_scope = Some(scope);
        }
        if let Some(id) = self.filters.collection {
            self.filters.members = self.collection_photos.get(&id).cloned().unwrap_or_default();
        }
        self.selection = place.selection.clone();
        self.filter();
        self.scroll_to_active = true;
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
        // Outside the folder shown, or no longer offline: All Photographs.
        if !self.visible.iter().any(|i| self.photos[*i].id == id) {
            self.filters.folder_scope = None;
            self.filters.collection = None;
            self.filters.only_missing = false;
            self.selected_folder.clear();
            self.filter();
        }
        self.select(Some(id));
    }
    /// Drain in every workspace so the bounded worker never waits for the grid.
    pub(super) fn poll_previews(&mut self, ctx: &egui::Context) {
        if self.availability.poll(false, &self.photos) {
            self.availability_known();
        }
        self.poll_capture_times();
        self.poll_photo_info();
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
    /// The selected photo, or else the first one shown in the current
    /// folder or filter (which then becomes selected), as Lightroom does
    /// when switching to Develop.
    pub(super) fn selected_or_first(&mut self) -> Option<i64> {
        if self.selection.active.is_none() {
            self.select(self.visible.first().map(|i| self.photos[*i].id));
        }
        self.selection.active
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
/// Lightroom-style grid cells: the label tints the cell, while selection uses
/// a lighter surround instead of the app's blue button fill.
mod availability;
mod background;
mod capture;
mod cell;
mod collections;
mod copy_name;
mod filmstrip;
mod filter;
mod grid;
mod info;
mod loupe;
mod metadata;
mod photo_info;
mod previews;
mod selection;
mod sidebar;
mod textures;
mod thumbnails;
mod tree;
mod volumes;
mod zoom;
use thumbnails::thumbnail;
#[cfg(test)]
mod tests;
