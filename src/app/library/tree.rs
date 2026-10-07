use crate::app::icons::{self, Icon};
use crate::app::theme;
use crate::catalog::{Folder, FolderId, RootId};
use eframe::egui::{self, Vec2};
use std::{collections::HashSet, path::PathBuf};
#[derive(Default)]
pub(super) struct FolderNode {
    pub(super) key: String,
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) root: Option<RootId>,
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
            ..Default::default()
        }
    }
    pub(super) fn insert(&mut self, f: &Folder) {
        let mut node = self;
        for component in f.relative.split('/').filter(|c| !c.is_empty()) {
            let key = format!("{}/{}", node.key, component);
            let path = node.path.join(component);
            node = node
                .children
                .entry(component.into())
                .or_insert_with(|| FolderNode {
                    key,
                    name: component.into(),
                    path,
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
}
pub(super) enum TreeAction {
    Select(String, HashSet<FolderId>),
    RelinkRoot(RootId),
    RelinkFolder(FolderId),
}
pub(super) fn folder_tree_row(
    ui: &mut egui::Ui,
    node: &FolderNode,
    depth: usize,
    expanded: &mut HashSet<String>,
    selected: &str,
) -> Option<TreeAction> {
    let palette = theme::palette(ui.ctx());
    use egui::{Align2, FontId, Pos2, Rect, Sense};
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.), Sense::click());
    let painter = ui.painter();
    let active = selected == node.key;
    let open = expanded.contains(&node.key);
    if active || response.hovered() {
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
    // Triangle in the gutter, folder icon at the same 10 px inset as the
    // Catalog rows' text.
    let x = rect.left() + indent + 3.;
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
        Pos2::new(rect.right() - 62., rect.bottom()),
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
        Pos2::new(rect.right() - 27., y),
        Align2::RIGHT_CENTER,
        node.count.to_string(),
        FontId::proportional(10.),
        palette.gray(125),
    );
    if can_relink && (response.hovered() || node.root.is_some()) {
        for dx in [-3., 0., 3.] {
            painter.circle_filled(Pos2::new(rect.right() - 12. + dx, y), 1., palette.gray(160));
        }
    }
    let relink = || {
        node.root
            .map(TreeAction::RelinkRoot)
            .or_else(|| node.folder.map(TreeAction::RelinkFolder))
    };
    let mut action = None;
    if response.clicked() && !crate::app::widgets::context_clicked(&response) {
        let pointer = response.interact_pointer_pos().unwrap_or(rect.center());
        if can_relink && pointer.x > rect.right() - 24. {
            action = relink()
        } else if !node.children.is_empty() && pointer.x < x + 6. {
            if open {
                expanded.remove(&node.key);
            } else {
                expanded.insert(node.key.clone());
            }
        } else {
            action = Some(TreeAction::Select(node.key.clone(), node.ids.clone()));
        }
    }
    response.clone().on_hover_text(format!(
        "{}\n{} photographs{}",
        node.path.display(),
        node.count,
        if can_relink {
            " including subfolders\nClick … or right-click to locate"
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
