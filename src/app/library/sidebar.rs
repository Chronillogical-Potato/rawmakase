//! The Library's left panel: Navigator, Catalog, Folders and Collections, and
//! the source they choose.
use super::tree::{FolderNode, TreeAction, folder_tree_row};
use super::{Action, Library, collections, volumes};
use crate::app::theme;
use crate::app::widgets::section;
use crate::catalog::CollectionKind;
use eframe::egui::{self, Vec2};
use std::collections::HashSet;

impl Library {
    /// The left panel; without `navigator` when the editor shows its own,
    /// as for a RAW in the Loupe.
    pub fn sidebar(&mut self, ui: &mut egui::Ui, navigator: bool) -> Action {
        let mut action = Action::None;
        ui.spacing_mut().item_spacing.y = 0.;
        egui::ScrollArea::vertical()
            .id_salt("library-sources")
            .show(ui, |ui| {
                if navigator {
                    let photo = self
                        .selected()
                        .and_then(|id| self.photo(id))
                        .and_then(|p| self.texture(p))
                        .map(|t| (t.id(), t.size_vec2()));
                    crate::app::navigator::navigator(ui, photo, None, None);
                }
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
    /// Shows collection `id`'s photos; the filter bar still applies.
    pub(super) fn select_collection(&mut self, id: i64) {
        self.filters.collection = Some(id);
        self.filters.members = self.collection_photos.get(&id).cloned().unwrap_or_default();
        self.filters.folder_scope = None;
        self.filters.only_missing = false;
        self.selected_folder.clear();
        self.filter();
    }
    /// The source shown: a folder tree key ("" is All Photographs) or
    /// `collection:<id>`.
    pub(in crate::app) fn source_key(&self) -> String {
        match self.filters.collection {
            Some(id) => format!("collection:{id}"),
            None => self.selected_folder.clone(),
        }
    }
    /// Shows a folder saved with `source_key` again, including its subfolders,
    /// and selects `photo` if it is in it.
    /// The folders a folder key covers now: the folder and its subfolders.
    /// None for a key that names no folder in the catalog.
    pub(super) fn folder_scope(&self, key: &str) -> Option<HashSet<i64>> {
        let rest = key.strip_prefix("root:")?;
        let (root, relative) = rest.split_once('/').unwrap_or((rest, ""));
        let root = root.parse::<i64>().ok()?;
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
        (!ids.is_empty()).then_some(ids)
    }
    pub(in crate::app) fn restore_source(&mut self, key: &str, photo: Option<i64>) {
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
            if let Ok(root) = root.parse::<i64>()
                && let Some(ids) = self.folder_scope(key)
            {
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
        self.filter();
        if let Some(id) = photo
            && self.visible.iter().any(|i| self.photos[*i].id == id)
        {
            self.select(Some(id));
        }
    }
    /// The selected source's name, as Lightroom shows it above the filmstrip.
    pub(in crate::app) fn source_name(&self) -> String {
        if let Some(id) = self.filters.collection {
            return self.collections.iter().find(|c| c.id == id).map_or_else(
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
