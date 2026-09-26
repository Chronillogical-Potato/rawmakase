use crate::catalog::Folder;
use eframe::egui::{self, Color32, Vec2};
use std::{collections::HashSet, path::PathBuf};
#[derive(Default)]
pub(super) struct FolderNode {
    pub(super) key: String,
    pub(super) name: String,
    pub(super) path: PathBuf,
    pub(super) root: Option<i64>,
    pub(super) folder: Option<i64>,
    pub(super) own_count: usize,
    pub(super) count: usize,
    pub(super) ids: HashSet<i64>,
    pub(super) children: std::collections::BTreeMap<String, FolderNode>,
}
impl FolderNode {
    pub(super) fn root(id: i64, name: String, path: PathBuf) -> Self {
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
    Select(String, HashSet<i64>),
    Relink(bool, i64),
}
pub(super) fn folder_tree_row(
    ui: &mut egui::Ui,
    node: &FolderNode,
    depth: usize,
    expanded: &mut HashSet<String>,
    selected: &str,
) -> Option<TreeAction> {
    use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke};
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
                Color32::from_rgb(47, 58, 66)
            } else {
                Color32::from_gray(43)
            },
        );
    }
    if active {
        painter.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(2., rect.height())),
            0.,
            Color32::from_rgb(135, 160, 176),
        );
    }
    let indent = depth.min(12) as f32 * 14.;
    let x = rect.left() + indent + 10.;
    let y = rect.center().y;
    if !node.children.is_empty() {
        let points = if open {
            vec![
                Pos2::new(x - 3., y - 2.),
                Pos2::new(x, y + 1.),
                Pos2::new(x + 3., y - 2.),
            ]
        } else {
            vec![
                Pos2::new(x - 2., y - 3.),
                Pos2::new(x + 1., y),
                Pos2::new(x - 2., y + 3.),
            ]
        };
        painter.add(egui::Shape::line(
            points,
            Stroke::new(1.2, Color32::from_gray(150)),
        ));
    }
    let icon = Rect::from_min_size(Pos2::new(x + 10., y - 4.), Vec2::new(12., 9.));
    painter.rect_stroke(
        icon,
        1.,
        Stroke::new(1., Color32::from_gray(145)),
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [
            Pos2::new(icon.left(), icon.top()),
            Pos2::new(icon.left(), icon.top() - 2.),
        ],
        Stroke::new(1., Color32::from_gray(145)),
    );
    painter.line_segment(
        [
            Pos2::new(icon.left(), icon.top() - 2.),
            Pos2::new(icon.left() + 5., icon.top() - 2.),
        ],
        Stroke::new(1., Color32::from_gray(145)),
    );
    let can_relink = node.root.is_some() || node.folder.is_some();
    let label_rect = Rect::from_min_max(
        Pos2::new(x + 29., rect.top()),
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
        Color32::from_gray(if active { 235 } else { 190 }),
    );
    painter.text(
        Pos2::new(rect.right() - 27., y),
        Align2::RIGHT_CENTER,
        node.count.to_string(),
        FontId::proportional(10.),
        Color32::from_gray(125),
    );
    if can_relink && (response.hovered() || node.root.is_some()) {
        for dx in [-3., 0., 3.] {
            painter.circle_filled(
                Pos2::new(rect.right() - 12. + dx, y),
                1.,
                Color32::from_gray(160),
            );
        }
    }
    let relink = || {
        node.root
            .map(|id| TreeAction::Relink(true, id))
            .or_else(|| node.folder.map(|id| TreeAction::Relink(false, id)))
    };
    let mut action = None;
    if response.clicked() && !crate::app::widgets::context_clicked(&response) {
        let pointer = response.interact_pointer_pos().unwrap_or(rect.center());
        if can_relink && pointer.x > rect.right() - 24. {
            action = relink()
        } else if !node.children.is_empty() && pointer.x < x + 8. {
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
