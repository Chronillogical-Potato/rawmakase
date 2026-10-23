//! The Library's left panel: Catalog, Folders and Collections, and
//! the source they choose.
use super::folder_selection::{Click, FolderSelection};
use super::tree::{FolderNode, TreeAction, folder_tree_row};
use super::{Action, Library, collections, volumes};
use crate::app::theme;
use crate::app::widgets::section;
use crate::catalog::{CollectionKind, FolderId, PhotoId, RootId};
use eframe::egui::{self, Vec2};
use std::collections::HashSet;

/// The sidebar's trees, as the catalog's lists were when they were built.
pub(super) struct Trees {
    /// What the lists were: see [`Library::lists_key`].
    key: ListsKey,
    /// The drive each root was on, in the catalog's order of roots.
    drives: Vec<crate::platform::volume::Volume>,
    /// The startup disk first, then other drives by name.
    volumes: Vec<Drive>,
    collections: Vec<collections::CollectionNode>,
}
/// A drive in the Folders panel: its roots' folder trees and photo count.
struct Drive {
    volume: crate::platform::volume::Volume,
    trees: Vec<FolderNode>,
    photos: usize,
}
/// What the sidebar's trees are built from: the session's structural
/// generation, which every full read changes, and the sizes of the lists the
/// edits made without one change (collection members, a removal whose read
/// failed, the Quick Collection made).
type ListsKey = (u64, usize, usize, usize, usize);

impl Trees {
    /// The drives the Folders panel shows, in order.
    #[cfg(test)]
    pub(super) fn drives(&self) -> Vec<crate::platform::volume::Volume> {
        self.volumes.iter().map(|d| d.volume.clone()).collect()
    }
}
impl Library {
    fn lists_key(&self) -> ListsKey {
        let members = self
            .session
            .collection_photos
            .values()
            .map(|m| m.len())
            .sum();
        (
            self.session.generation(),
            self.session.folders.len(),
            self.session.roots.len(),
            self.session.collections.len(),
            members,
        )
    }
    /// Where each root folder is on this computer.
    pub(super) fn root_paths(&self) -> Vec<std::path::PathBuf> {
        let roots = self.session.roots.iter();
        roots
            .map(|(_, original, mapped)| mapped.as_deref().unwrap_or(original).into())
            .collect()
    }
    /// The sidebar's trees, built again only when the lists, or the drives
    /// their roots are on, changed.
    fn sidebar_trees(&mut self) -> std::sync::Arc<Trees> {
        let key = self.lists_key();
        let paths = self.root_paths();
        let drives = self.volumes.drives(paths.iter().map(|p| p.as_path()));
        if let Some(trees) = self
            .sidebar_trees
            .as_ref()
            .filter(|t| t.key == key && t.drives == drives)
        {
            return trees.clone();
        }
        let mut volumes: std::collections::BTreeMap<
            crate::platform::volume::Volume,
            Vec<(RootId, String, Option<String>)>,
        > = Default::default();
        for (root, drive) in self.session.roots.iter().zip(&drives) {
            volumes.entry(drive.clone()).or_default().push(root.clone());
        }
        let mut volumes: Vec<_> = volumes
            .into_iter()
            .map(|(volume, roots)| {
                let photos = roots
                    .iter()
                    .map(|(id, ..)| {
                        self.session
                            .folders
                            .iter()
                            .filter(|f| f.root == *id)
                            .map(|f| f.count)
                            .sum::<usize>()
                    })
                    .sum();
                let trees = roots
                    .into_iter()
                    .map(|(root, original, mapped)| {
                        let root_path = mapped.as_deref().unwrap_or(&original);
                        let name = std::path::Path::new(&original)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        let mut tree = FolderNode::root(root, name, root_path.into());
                        for f in self.session.folders.iter().filter(|f| f.root == root) {
                            tree.insert(f);
                        }
                        tree.finish();
                        tree
                    })
                    .collect();
                Drive {
                    volume,
                    trees,
                    photos,
                }
            })
            .collect();
        volumes.sort_by_key(|d: &Drive| (d.volume.mount.is_some(), d.volume.name.to_lowercase()));
        let trees = std::sync::Arc::new(Trees {
            key,
            drives,
            volumes,
            collections: collections::tree(
                &self.session.collections,
                &self.session.collection_photos,
            ),
        });
        self.sidebar_trees = Some(trees.clone());
        trees
    }
    /// The left panel's catalog, folders and collections.
    pub(crate) fn sidebar(&mut self, ui: &mut egui::Ui) -> Action {
        let mut action = Action::None;
        // The folder and collection trees built when the lists last changed.
        let trees = self.sidebar_trees();
        ui.spacing_mut().item_spacing.y = 0.;
        egui::ScrollArea::vertical()
            .id_salt("library-sources")
            .show(ui, |ui| {
                section(ui, "Catalog", false, |ui| {
                    let offline = self.session.photos.len() - self.available_count();
                    let all = self.filters.folder_scope.is_none()
                        && self.filters.collection.is_none()
                        && !self.filters.only_missing
                        && !self.filters.previous_import;
                    if source_row(ui, "All Photographs", self.session.photos.len(), all).clicked() {
                        self.filters.folder_scope = None;
                        self.selected_folders.clear();
                        self.filters.collection = None;
                        self.filters.only_missing = false;
                        self.filters.previous_import = false;
                        self.filter()
                    }
                    if source_row(
                        ui,
                        "Quick Collection",
                        self.quick_count(),
                        self.showing_quick(),
                    )
                    .on_hover_text("B adds the selected photos · ⌘B shows them")
                    .clicked()
                        && let Err(e) = self.show_quick()
                    {
                        self.message = format!("Quick Collection unavailable: {e}");
                    }
                    let imported = self.session.previous_import.len();
                    if imported > 0
                        && source_row(
                            ui,
                            "Previous Import",
                            imported,
                            self.filters.previous_import,
                        )
                        .on_hover_text("The photos the last Add Photo Folder or Synchronize added")
                        .clicked()
                    {
                        self.show_previous_import();
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
                    self.volumes.check(ui.ctx(), self.root_paths());
                    let online = self.volumes.snapshot().online;
                    self.volumes_checked(&online);
                    // Applied once every row is drawn, for a Shift-click range.
                    let mut clicked = None;
                    for (k, drive) in trees.volumes.iter().enumerate() {
                        let volume = &drive.volume;
                        // Room between drives, so each reads as its own group.
                        if k > 0 {
                            ui.add_space(10.);
                        }
                        let state = online
                            .get(volume.mount.as_deref().unwrap_or(std::path::Path::new("/")))
                            .copied();
                        let attached = match &volume.mount {
                            None => Some(true),
                            Some(_) => state.map(|s| s.0),
                        };
                        let space = state.and_then(|s| s.1);
                        let photos = drive.photos;
                        let key = format!("volume-collapsed:{}", volume.name);
                        let collapsed = self.expanded.contains(&key);
                        if volumes::volume_row(ui, volume, attached, space, photos, !collapsed)
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
                        for tree in &drive.trees {
                            match folder_tree_row(
                                ui,
                                tree,
                                0,
                                &mut self.expanded,
                                &self.selected_folders,
                            ) {
                                Some(TreeAction::Select(key, click)) => {
                                    clicked = Some((key, click));
                                }
                                Some(TreeAction::RelinkRoot(id)) => action = Action::RelinkRoot(id),
                                Some(TreeAction::RelinkFolder(id)) => {
                                    action = Action::RelinkFolder(id)
                                }
                                Some(TreeAction::Synchronize(scope, name)) => {
                                    self.sync_request = Some((scope, name));
                                }
                                Some(TreeAction::NewFolder {
                                    root,
                                    parent,
                                    name,
                                    folder,
                                }) => self.request_new_folder(root, parent, name, folder),
                                Some(TreeAction::RenameFolder { root, logical }) => {
                                    self.file_request =
                                        Some(super::FileRequest::RenameFolder { root, logical })
                                }
                                Some(TreeAction::MoveFolder { root, logical }) => {
                                    self.file_request = Some(super::FileRequest::ChooseParent {
                                        root,
                                        from: logical,
                                    })
                                }
                                Some(TreeAction::TrashFolder { root, logical }) => {
                                    self.file_request =
                                        Some(super::FileRequest::TrashFolder { root, logical })
                                }
                                Some(TreeAction::RemoveRoot(root, name)) => {
                                    self.removal_request = Some(super::FolderRemoval {
                                        catalog: self.session.catalog.location().clone(),
                                        name,
                                        folders: HashSet::new(),
                                        photos: 0,
                                        empty_root: Some(root),
                                    });
                                }
                                Some(TreeAction::DropFolder {
                                    root,
                                    from,
                                    to_root,
                                    to,
                                }) => {
                                    self.file_request = Some(super::FileRequest::MoveFolder {
                                        root,
                                        from,
                                        to_root,
                                        to,
                                    })
                                }
                                Some(TreeAction::Reveal(path)) => {
                                    if let Err(e) = crate::platform::reveal::reveal(&path) {
                                        self.message =
                                            format!("Could not show {}: {e}", path.display());
                                    }
                                }
                                Some(TreeAction::Drop {
                                    root,
                                    to,
                                    name,
                                    photos,
                                }) => self.request_move(photos, root, to, name),
                                Some(TreeAction::Remove(name, folders, photos)) => {
                                    self.removal_request = Some(super::FolderRemoval {
                                        catalog: self.session.catalog.location().clone(),
                                        name,
                                        folders,
                                        photos,
                                        empty_root: None,
                                    })
                                }
                                None => {}
                            }
                        }
                    }
                    if let Some((key, click)) = clicked {
                        let mut rows = Vec::new();
                        for drive in &trees.volumes {
                            let collapsed = format!("volume-collapsed:{}", drive.volume.name);
                            if !self.expanded.contains(&collapsed) {
                                for tree in &drive.trees {
                                    tree.shown_rows(&self.expanded, &mut rows);
                                }
                            }
                        }
                        self.click_folder(&key, click, &rows);
                    }
                    if self.session.roots.is_empty() {
                        empty_note(ui, "No folders yet");
                    }
                    ui.add_space(16.);
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
                    let tree = &trees.collections;
                    for node in tree {
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
                        empty_note(ui, "No collections");
                    }
                });
            });
        action
    }
    /// What the Navigator shows outside the Loupe: the selected photo's
    /// preview.
    pub(in crate::app) fn selected_preview(&self) -> Option<crate::app::navigator::Photo> {
        self.selected()
            .and_then(|id| self.photo(id))
            .and_then(|p| self.texture(p))
            .map(|t| (t.id(), t.size_vec2()))
    }
    /// Shows collection `id`'s photos; the filter bar still applies.
    /// Shows Previous Import: the photos the last import added.
    pub(in crate::app) fn show_previous_import(&mut self) {
        self.filters.previous_import = true;
        self.filters.members = self.session.previous_import.clone();
        self.filters.collection = None;
        self.filters.folder_scope = None;
        self.filters.only_missing = false;
        self.selected_folders.clear();
        self.offer_previous_import = false;
        self.filter();
    }
    pub(in crate::app) fn select_collection(&mut self, id: crate::catalog::CollectionId) {
        self.filters.previous_import = false;
        self.filters.collection = Some(id);
        self.filters.members = self
            .session
            .collection_photos
            .get(&id)
            .cloned()
            .unwrap_or_default();
        self.filters.folder_scope = None;
        self.filters.only_missing = false;
        self.selected_folders.clear();
        self.filter();
    }
    /// A plain click on the folder with tree key `key`, as the Folders
    /// panel gives it.
    #[cfg(test)]
    pub(in crate::app) fn click_folder_for_tests(&mut self, key: &str) {
        self.click_folder(key, Click::Replace, &[key]);
    }
    /// Shows the folders chosen by clicking `key` in the Folders panel;
    /// `rows` are the folder rows shown, in order.
    pub(super) fn click_folder(&mut self, key: &str, click: Click, rows: &[&str]) {
        self.selected_folders.click(key, click, rows);
        self.filters.folder_scope = self.folders_scope(&self.selected_folders);
        self.filters.collection = None;
        self.filters.previous_import = false;
        self.filter();
    }
    /// The source shown: a folder tree key ("" is All Photographs) or
    /// `collection:<id>`. Of several folders, the one the others were chosen
    /// from: the session keeps one.
    pub(in crate::app) fn source_key(&self) -> String {
        match self.filters.collection {
            Some(id) => format!("collection:{id}"),
            None => self.selected_folders.anchor().to_string(),
        }
    }
    /// Shows a folder saved with `source_key` again, including its subfolders,
    /// and selects `photo` if it is in it.
    /// The folders a folder key covers now: the folder and its subfolders.
    /// None for a key that names no folder in the catalog.
    pub(super) fn folder_scope(&self, key: &str) -> Option<HashSet<FolderId>> {
        let rest = key.strip_prefix("root:")?;
        let (root, relative) = rest.split_once('/').unwrap_or((rest, ""));
        let root = root.parse::<i64>().ok().map(RootId)?;
        let below = format!("{relative}/");
        let ids: HashSet<FolderId> = self
            .session
            .folders
            .iter()
            .filter(|f| {
                let path = f.relative.trim_end_matches('/');
                f.root == root
                    && (relative.is_empty() || path == relative || path.starts_with(&below))
            })
            .map(|f| f.id)
            .collect();
        (!ids.is_empty()).then_some(ids)
    }
    /// The folders several folder keys cover now; none for none chosen (All
    /// Photographs). A chosen folder no longer in the catalog adds none.
    pub(super) fn folders_scope(&self, folders: &FolderSelection) -> Option<HashSet<FolderId>> {
        (!folders.is_empty()).then(|| {
            folders
                .keys()
                .iter()
                .filter_map(|key| self.folder_scope(key))
                .flatten()
                .collect()
        })
    }
    pub(in crate::app) fn restore_source(&mut self, key: &str, photo: Option<PhotoId>) {
        if let Some(id) = key
            .strip_prefix("collection:")
            .and_then(|id| id.parse::<i64>().ok())
            .map(crate::catalog::CollectionId)
            && self.session.collections.iter().any(|c| {
                c.id == id && (c.kind == CollectionKind::Collection || self.quick() == Some(id))
            })
        {
            self.select_collection(id);
        }
        if let Some(rest) = key.strip_prefix("root:") {
            let (root, relative) = rest.split_once('/').unwrap_or((rest, ""));
            if let Some(root) = root.parse::<i64>().ok().map(RootId)
                && let Some(ids) = self.folder_scope(key)
            {
                self.selected_folders = FolderSelection::one(key);
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
        self.filter();
        if let Some(id) = photo
            && self
                .visible
                .iter()
                .any(|i| self.session.photos[*i].id == id)
        {
            self.select(Some(id));
        }
    }
    /// The selected source's name, as Lightroom shows it above the filmstrip.
    pub(in crate::app) fn source_name(&self) -> String {
        if self.filters.previous_import {
            return "Previous Import".into();
        }
        if let Some(id) = self.filters.collection {
            return self
                .session
                .collections
                .iter()
                .find(|c| c.id == id)
                .map_or_else(
                    || "Collection".into(),
                    |c| {
                        if c.name == crate::catalog::QUICK_COLLECTION {
                            "Quick Collection".into()
                        } else {
                            c.name.clone()
                        }
                    },
                );
        }
        let selected = match self.selected_folders.keys() {
            [] => return "All Photographs".into(),
            [key] => key,
            keys => return format!("{} folders", keys.len()),
        };
        match selected.rsplit_once('/') {
            Some((_, name)) => name.into(),
            None => self
                .session
                .roots
                .iter()
                .find(|(id, _, _)| format!("root:{id}") == *selected)
                .and_then(|(_, original, _)| {
                    std::path::Path::new(original)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                })
                .unwrap_or_default(),
        }
    }
}
/// A full-width "+ label" button under a panel's list: outlined, so it reads
/// as an action rather than another row.
/// A section's note that it is empty, inset as its rows' names are.
fn empty_note(ui: &mut egui::Ui, text: &str) {
    let palette = theme::palette(ui.ctx());
    ui.add_space(4.);
    ui.horizontal(|ui| {
        ui.add_space(10.);
        ui.label(egui::RichText::new(text).size(11.).color(palette.gray(120)));
    });
}
fn add_row(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), egui::Sense::click());
    let hovered = response.hovered();
    ui.painter().rect(
        rect,
        6.,
        palette.gray(if hovered { 40 } else { 28 }),
        egui::Stroke::new(1., palette.gray(if hovered { 70 } else { 52 })),
        egui::StrokeKind::Inside,
    );
    let color = palette.gray(if hovered { 240 } else { 200 });
    let text =
        ui.painter()
            .layout_no_wrap(label.to_string(), egui::FontId::proportional(12.), color);
    // The icon and label centred together.
    let width = 13. + 6. + text.size().x;
    let left = rect.center().x - width / 2.;
    crate::app::icons::paint_at(
        ui.painter(),
        crate::app::icons::Icon::Add,
        egui::pos2(left + 6.5, rect.center().y),
        13.,
        color,
    );
    ui.painter().galley(
        egui::pos2(left + 19., rect.center().y - text.size().y / 2.),
        text,
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
/// A Catalog panel row: name on the left, photo count right-aligned.
fn source_row(ui: &mut egui::Ui, name: &str, count: usize, active: bool) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.), egui::Sense::click());
    if active || response.hovered() {
        ui.painter().rect_filled(
            rect,
            2.,
            if active {
                palette.selected_row()
            } else {
                palette.gray(43)
            },
        );
    }
    let y = rect.center().y;
    ui.painter().text(
        egui::pos2(rect.left() + 10., y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.),
        palette.gray(if active { 235 } else { 190 }),
    );
    ui.painter().text(
        egui::pos2(rect.right() - 10., y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::proportional(11.),
        palette.gray(125),
    );
    response
}
