use super::folder_selection::{Click, FolderSelection};
use crate::app::icons::{self, Icon};
use crate::app::theme;
use crate::catalog::{Folder, FolderId, PhotoId, RootId};
use eframe::egui::{self, Vec2};
use std::{collections::HashSet, path::PathBuf};
#[derive(Default)]
pub(super) struct FolderNode {
    pub(super) key: String,
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) root: Option<RootId>,
    /// The root it is in, and its logical path there ("" for the root).
    pub(super) in_root: RootId,
    pub(super) logical: String,
    pub(super) folder: Option<FolderId>,
    pub(super) own_count: usize,
    pub(super) count: usize,
    pub(super) ids: HashSet<FolderId>,
    pub(super) children: std::collections::BTreeMap<String, FolderNode>,
}
impl FolderNode {
    pub(super) fn root(id: RootId, name: String, path: PathBuf) -> Self {
        Self {
            key: format!("root:{id}"),
            name,
            path,
            root: Some(id),
            in_root: id,
            ..Default::default()
        }
    }
    pub(super) fn insert(&mut self, f: &Folder) {
        let mut node = self;
        for component in f.relative.split('/').filter(|c| !c.is_empty()) {
            let key = format!("{}/{}", node.key, component);
            let path = node.path.join(component);
            let in_root = node.in_root;
            let logical = match node.logical.as_str() {
                "" => component.to_string(),
                parent => format!("{parent}/{component}"),
            };
            node = node
                .children
                .entry(component.into())
                .or_insert_with(|| FolderNode {
                    key,
                    name: component.into(),
                    path,
                    in_root,
                    logical,
                    ..Default::default()
                });
        }
        node.folder = Some(f.id);
        node.own_count += f.count;
        node.path = f.path.clone();
    }
    pub(super) fn finish(&mut self) {
        self.count = self.own_count;
        if let Some(id) = self.folder {
            self.ids.insert(id);
        }
        for child in self.children.values_mut() {
            child.finish();
            self.count += child.count;
            self.ids.extend(&child.ids);
        }
    }
    /// The keys of this row and the rows unfolded below it, in the order
    /// they are drawn.
    pub(super) fn shown_rows<'a>(&'a self, expanded: &HashSet<String>, rows: &mut Vec<&'a str>) {
        rows.push(&self.key);
        if expanded.contains(&self.key) {
            for child in self.children.values() {
                child.shown_rows(expanded, rows);
            }
        }
    }
}
pub(super) enum TreeAction {
    /// The folder's key, and how it was clicked.
    Select(String, Click),
    RelinkRoot(RootId),
    RelinkFolder(FolderId),
    /// Remove from Catalog: the folder's name, it and its subfolders, and
    /// their photos' count.
    Remove(String, HashSet<FolderId>, usize),
    /// Synchronize Folder: what to synchronize, and its name.
    Synchronize(crate::catalog::folder_sync::SyncScope, String),
    /// New Folder Inside…: the folder's root, logical path, name and
    /// catalog folder, if it has one.
    NewFolder {
        root: RootId,
        parent: String,
        name: String,
        folder: Option<FolderId>,
    },
    /// Photos dropped on catalog folder `folder`, named `name`.
    Drop {
        folder: FolderId,
        name: String,
        photos: Vec<PhotoId>,
    },
}
pub(super) fn folder_tree_row(
    ui: &mut egui::Ui,
    node: &FolderNode,
    depth: usize,
    expanded: &mut HashSet<String>,
    selected: &FolderSelection,
) -> Option<TreeAction> {
    let palette = theme::palette(ui.ctx());
    use egui::{Align2, FontId, Pos2, Rect, Sense};
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.), Sense::click());
    let painter = ui.painter();
    let active = selected.contains(&node.key);
    let open = expanded.contains(&node.key);
    // Photos dragged over a catalog folder can be dropped on it.
    let dropping = node.folder.is_some()
        && response
            .dnd_hover_payload::<super::DraggedPhotos>()
            .is_some();
    if dropping {
        painter.rect_filled(rect, 3., palette.selected_row());
        painter.rect_stroke(
            rect.shrink(0.5),
            3.,
            egui::Stroke::new(1., palette.selected_marker()),
            egui::StrokeKind::Inside,
        );
    } else if active || response.hovered() {
        painter.rect_filled(
            rect,
            3.,
            if active {
                palette.selected_row()
            } else {
                palette.gray(43)
            },
        );
    }
    if active {
        painter.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(2., rect.height())),
            0.,
            palette.selected_marker(),
        );
    }
    let indent = depth.min(12) as f32 * 14.;
    // The chevron in the gutter, starting at the Catalog rows' 10 px inset
    // under a drive's dot; the folder icon under the drive's name.
    let x = rect.left() + indent + 12.;
    let y = rect.center().y;
    if !node.children.is_empty() {
        let chevron = if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        icons::paint_at(painter, chevron, Pos2::new(x, y), 11., palette.gray(150));
    }
    icons::paint_at(
        painter,
        Icon::Folder,
        Pos2::new(x + 13., y),
        13.,
        palette.gray(145),
    );
    let can_relink = node.root.is_some() || node.folder.is_some();
    let label_rect = Rect::from_min_max(
        Pos2::new(x + 26., rect.top()),
        Pos2::new(rect.right() - 44., rect.bottom()),
    );
    let text = egui::WidgetText::from(node.name.clone()).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        label_rect.width().max(1.),
        FontId::proportional(12.),
    );
    painter.galley(
        Pos2::new(label_rect.left(), y - text.size().y / 2.),
        text,
        palette.gray(if active { 235 } else { 190 }),
    );
    painter.text(
        Pos2::new(rect.right() - 10., y),
        Align2::RIGHT_CENTER,
        node.count.to_string(),
        FontId::proportional(10.),
        palette.gray(125),
    );
    let relink = || {
        node.root
            .map(TreeAction::RelinkRoot)
            .or_else(|| node.folder.map(TreeAction::RelinkFolder))
    };
    let mut action = None;
    // Photos are dragged only once the pointer went far enough (see
    // `grid::drag_payload`).
    if let (Some(folder), Some(dragged)) = (
        node.folder,
        response.dnd_release_payload::<super::DraggedPhotos>(),
    ) {
        action = Some(TreeAction::Drop {
            folder,
            name: node.name.clone(),
            photos: dragged.0.clone(),
        });
    }
    if response.clicked() && !crate::app::widgets::context_clicked(&response) {
        let pointer = response.interact_pointer_pos().unwrap_or(rect.center());
        if !node.children.is_empty() && pointer.x < x + 6. {
            if open {
                expanded.remove(&node.key);
            } else {
                expanded.insert(node.key.clone());
            }
        } else {
            let click = Click::of(ui.input(|i| i.modifiers));
            action = Some(TreeAction::Select(node.key.clone(), click));
        }
    }
    response.clone().on_hover_text(format!(
        "{}\n{} photographs{}",
        node.path.display(),
        node.count,
        if can_relink {
            " including subfolders\nRight-click to locate, synchronize, make a folder inside or remove"
        } else {
            ""
        }
    ));
    crate::app::widgets::context_menu(&response, |ui| {
        if can_relink
            && ui
                .button(if node.root.is_some() {
                    "Locate root folder…"
                } else {
                    "Locate this folder…"
                })
                .clicked()
        {
            action = relink();
            ui.close();
        }
        let scope = match (node.root, node.folder) {
            (Some(root), _) => Some(crate::catalog::folder_sync::SyncScope::Root(root)),
            (None, Some(folder)) => Some(crate::catalog::folder_sync::SyncScope::Folder(folder)),
            _ => None,
        };
        if let Some(scope) = scope
            && ui.button("Synchronize Folder…").clicked()
        {
            action = Some(TreeAction::Synchronize(scope, node.name.clone()));
            ui.close();
        }
        if ui.button("New Folder Inside…").clicked() {
            action = Some(TreeAction::NewFolder {
                root: node.in_root,
                parent: node.logical.clone(),
                name: node.name.clone(),
                folder: node.folder,
            });
            ui.close();
        }
        if !node.ids.is_empty() && ui.button("Remove from Catalog…").clicked() {
            action = Some(TreeAction::Remove(
                node.name.clone(),
                node.ids.clone(),
                node.count,
            ));
            ui.close();
        }
    });
    if expanded.contains(&node.key) {
        for child in node.children.values() {
            if let Some(a) = folder_tree_row(ui, child, depth + 1, expanded, selected) {
                action = Some(a)
            }
        }
    }
    action
}
