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
pub use descriptive::{DescriptiveCommand, DescriptiveEdit};
pub use filmstrip::Pick;
pub use metadata::{Metadata, MetadataCommand};
pub use quick::CollectionCommand;
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
    /// How grid cells show their photos (J).
    cell_style: cell::Style,
    /// The sort order's keys from the catalog (see `Sort::keys`), kept until
    /// edits or sizes change them.
    sort_keys: Option<(sort::Sort, sort::Keys)>,
    /// The frame the Library was last drawn in, to notice it showing again.
    drawn_pass: u64,
    /// The style the grid was last drawn with, to keep its rows in place
    /// when it changes.
    drawn_style: cell::Style,
    /// Grid cells' photo info, read once per photo while expanded cells
    /// show it.
    cell_info: HashMap<i64, Option<crate::catalog::PhotoInfo>>,
    strip_current: Option<i64>,
    /// Indices into `photos` of the ones shown, in display order.
    visible: Vec<usize>,
    availability: availability::Availability,
    ctx: egui::Context,
    cache: textures::PreviewTextures,
    /// A virtual copy command from a thumbnail menu, for the editor.
    copy_request: Option<CopyAction>,
    copy_names: copy_name::CopyNames,
    /// Title, caption and the other descriptive fields being shown or typed.
    fields: metadata_fields::Fields,
    loupe: loupe::Loupe,
    compare: compare::Compare,
    survey: survey::Survey,
    /// Photos rendered at the size Compare and Survey show them.
    screen: screen::ScreenPreviews,
    stamps: stage::Stamps,
    /// Which way the Loupe last moved, so the photo after is prepared ahead.
    loupe_direction: i32,
    /// Metadata changes not yet handed to the shared undo log.
    done: Vec<MetadataCommand>,
    /// Collection changes not yet handed to the shared undo log.
    collection_done: Vec<quick::CollectionCommand>,
    /// Descriptive metadata changes not yet handed to the shared undo log.
    descriptive_done: Vec<DescriptiveCommand>,
    /// Reads capture times for photos added from folders.
    capture: Option<background::Reader<capture::Read>>,
    /// Reads camera settings and sizes for photos added from folders.
    info_reader: Option<background::Reader<Option<Option<crate::catalog::PhotoInfo>>>>,
    /// The Loupe's Info overlay.
    loupe_info: photo_info::Overlay,
    /// Photo info was asked for while it was being read.
    info_again: bool,
    /// The hovered grid photo's info, for its tooltip.
    hover_info: Option<(i64, Option<crate::catalog::PhotoInfo>)>,
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
    /// What a message sums up, one item per line, shown on hover while that
    /// message is: (message, detail).
    message_detail: (String, String),
    /// Photos to Read Metadata from Files for, once confirmed.
    read_request: Option<Vec<i64>>,
    /// Read Metadata from Files while it reads.
    reread: Option<descriptive::Reread>,
    /// Read Metadata from Files finished since the editor last asked.
    reread_finished: bool,
}
impl Library {
    pub fn load(path: &std::path::Path, ctx: egui::Context) -> Result<Self> {
        crate::platform::network::prepare_filesystem_bridge();
        let mut catalog = Catalog::open(path)?;
        // Catalogs imported before history was kept: recover it from the
        // stored Lightroom catalog. Best effort; a failure only hides history.
        let _ = catalog.backfill_lightroom_history();
        let _ = catalog.backfill_lightroom_info();
        let _ = catalog.backfill_lightroom_metadata();
        // Unlike those, a failure here could export keywords Lightroom keeps
        // out, so it is said; it is tried again on the next open.
        let keyword_options = catalog.backfill_keyword_export().err();
        let loupe = loupe::Loupe::new(&ctx);
        let screen = screen::ScreenPreviews::new(&ctx);
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
            cell_style: Default::default(),
            drawn_style: Default::default(),
            drawn_pass: 0,
            sort_keys: None,
            cell_info: HashMap::new(),
            strip_current: None,
            visible: Vec::new(),
            availability: Default::default(),
            cache: textures::PreviewTextures::new(&ctx),
            ctx,
            copy_request: None,
            copy_names: Default::default(),
            fields: Default::default(),
            done: Vec::new(),
            collection_done: Vec::new(),
            descriptive_done: Vec::new(),
            loupe,
            compare: Default::default(),
            survey: Default::default(),
            screen,
            stamps: Default::default(),
            loupe_direction: 1,
            capture: None,
            info_reader: None,
            info: None,
            info_again: false,
            hover_info: None,
            loupe_info: Default::default(),
            capture_tried: HashSet::new(),
            keep_in_place: None,
            grid_offset: 0.,
            grid_shown: 0..usize::MAX,
            attached: None,
            message: String::new(),
            message_detail: Default::default(),
            read_request: None,
            reread: None,
            reread_finished: false,
        };
        s.refresh()?;
        if let Some(e) = keyword_options {
            s.message = format!(
                "Lightroom's keyword export options could not be read: {e:#}. \
                 Exports include every keyword's parents until they are."
            );
        }
        // Start with a selection, as Lightroom does, so the side panels are filled.
        s.select(s.visible.first().map(|i| s.photos[*i].id));
        Ok(s)
    }
    pub fn refresh(&mut self) -> Result<()> {
        self.reload()?;
        self.availability.start(&self.photos, &self.ctx);
        self.cache.failed.clear();
        self.screen.retry_failed();
        self.filter();
        Ok(())
    }
    /// Reads the catalog again without checking which files are online,
    /// for changes that add or remove no file, such as virtual copies.
    fn reload(&mut self) -> Result<()> {
        self.cell_info.clear();
        self.sort_keys = None;
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
        // Copy commands save a name being typed before they run, and so
        // does anything else that reads the catalog again.
        self.copy_names.clear();
        self.fields.clear();
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
    /// Checks again, in the background, which photos are online when one
    /// counted offline turns out to be there, e.g. restored in place on a
    /// drive that stayed attached.
    pub(in crate::app) fn found(&mut self, path: &std::path::Path) {
        if !self.is_available(path) {
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
        // What the order needs from the catalog, read once until it changes.
        let sort = self.filters.sort;
        if self.sort_keys.as_ref().is_none_or(|(of, _)| *of != sort) {
            self.sort_keys = Some((sort, sort.keys(&self.catalog)));
        }
        let keys = &self.sort_keys.as_ref().unwrap().1;
        self.visible = self.filters.visible(
            &self.photos,
            |path| self.availability.is_available(path),
            keys,
        );
        self.keep_shown_selected();
    }
    /// Applies `change` and filters again, keeping the selected photo where
    /// it was on screen: for a re-sort the user did not ask for, such as
    /// capture times or sizes read in the background.
    fn resort_in_place(&mut self, change: impl FnOnce(&mut Self)) {
        let anchor = self.selected().and_then(|id| {
            self.visible
                .iter()
                .position(|i| self.photos[*i].id == id)
                .map(|at| (id, at))
        });
        // A selected photo scrolled out of view is no anchor: the view stays.
        let anchor = anchor.filter(|(_, at)| self.grid_shown.contains(at));
        change(self);
        self.filter();
        // Several batches before the grid is drawn again: the first position counts.
        if self.keep_in_place.is_none() {
            self.keep_in_place = anchor;
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
    #[cfg(test)]
    pub(in crate::app) fn show_unflagged(&mut self) {
        self.filters.flags = [0].into();
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
        self.compare.restored = true;
        self.scroll_to_active = true;
    }
    /// Whether Read Metadata from Files finished since the last call, for
    /// the status line to show its outcome.
    pub(in crate::app) fn take_reread_finished(&mut self) -> bool {
        std::mem::take(&mut self.reread_finished)
    }
    /// Photos Read Metadata from Files was chosen for, for the editor to
    /// confirm.
    pub(in crate::app) fn take_read_request(&mut self) -> Option<Vec<i64>> {
        self.read_request.take()
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
        self.screen.forget(id);
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
            // Filters turned off with Cmd+L hide nothing, and are kept.
            if self.filters.enabled {
                self.filters.clear_bar();
            }
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
        self.poll_reread();
        self.cache.poll(ctx);
        self.screen.poll(ctx);
    }
    /// Hands the worker the photos shown last frame. Call once per frame.
    pub(super) fn publish_shown(&mut self) {
        self.cache.publish_shown();
        self.screen.publish_shown();
    }
    /// The photo's preview: its edit once rendered, else the embedded one.
    fn texture(&self, photo: &Photo) -> Option<&egui::TextureHandle> {
        self.cache.texture(photo)
    }
    /// Queues the previews a shown photo needs; its edit comes from the catalog.
    fn request_previews(&mut self, photo: &Photo, ctx: &egui::Context) {
        let catalog = &self.catalog;
        self.cache
            .request(photo, ctx, || edit_source(catalog, photo.id));
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
    /// Saves a Copy Name or metadata field still being typed, e.g. when the
    /// Library panel goes away before the field loses focus. On failure it
    /// stays pending, to be saved again or discarded.
    pub(super) fn commit_drafts(&mut self) -> Result<()> {
        if self.copy_names.commit(&self.catalog, &mut self.photos)? {
            self.filter();
        }
        self.commit_fields()
    }
    #[cfg(test)]
    pub(super) fn set_copy_name_draft(&mut self, id: i64, name: &str) {
        self.copy_names.draft = Some((id, name.into()));
    }
    /// Drops a Copy Name or metadata field that could not be saved, e.g.
    /// closing without saving.
    pub(super) fn discard_drafts(&mut self) {
        self.copy_names.clear();
        self.fields.clear();
    }
}
/// Why Develop cannot open a photo: it edits camera RAW files that are
/// online.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Refusal {
    Offline,
    /// Not a camera RAW: the file's format, e.g. "JPEG".
    NotRaw(String),
}
impl Refusal {
    /// A word or two, shown beside a greyed-out Open in Develop.
    pub(in crate::app) fn label(&self) -> String {
        match self {
            Self::Offline => "Offline".into(),
            Self::NotRaw(format) => format!("{format} file"),
        }
    }
    /// The reason in full, with what to do about it.
    pub(in crate::app) fn detail(&self) -> String {
        match self {
            Self::Offline => {
                "The photo is offline. Use Locate root folder or right-click its folder to relink it."
                    .into()
            }
            Self::NotRaw(format) => format!(
                "{format} files can be browsed in Library; Develop edits camera RAW files."
            ),
        }
    }
}
/// Why Develop cannot open `photo`, if it cannot, given whether its file is
/// `available`.
pub(in crate::app) fn develop_refusal(photo: &Photo, available: bool) -> Option<Refusal> {
    if !available {
        Some(Refusal::Offline)
    } else if !crate::storage::is_raw(&photo.path) {
        Some(Refusal::NotRaw(photo.format.clone()))
    } else {
        None
    }
}
/// The edit a photo's previews are rendered with: its RAWmakase recipe, or
/// else its Lightroom settings.
fn edit_source(catalog: &Catalog, id: i64) -> Option<previews::EditSource> {
    let (recipe, lightroom) = catalog.edit_texts(id).ok()?;
    recipe
        .map(previews::EditSource::Recipe)
        .or(lightroom.map(previews::EditSource::Lightroom))
}

mod availability;
mod background;
mod capture;
/// Lightroom-style grid cells: the label tints the cell, while selection uses
/// a lighter surround instead of the app's blue button fill.
mod cell;
mod collections;
mod compare;
mod copy_name;
mod descriptive;
mod filmstrip;
mod filter;
mod filter_bar;
mod grid;
mod info;
mod layout;
mod loupe;
mod metadata;
mod metadata_fields;
mod photo_info;
mod previews;
mod quick;
mod rows;
mod screen;
mod selection;
mod sidebar;
mod sort;
mod stage;
mod survey;
mod textures;
mod thumbnails;
mod tree;
mod views;
mod volumes;
mod zoom;
use thumbnails::thumbnail;
#[cfg(test)]
mod tests;

impl Library {
    /// Shows `message`, with `detail` on hover while it is shown.
    pub(in crate::app) fn set_message_with_detail(&mut self, message: String, detail: String) {
        self.message = message.clone();
        self.message_detail = (message, detail);
    }
    /// What the message shown sums up, if it does.
    pub(in crate::app) fn message_detail(&self) -> Option<&str> {
        let (message, detail) = &self.message_detail;
        (*message == self.message && !detail.is_empty()).then_some(detail.as_str())
    }
}
