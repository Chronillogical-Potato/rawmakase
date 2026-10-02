//! The Collections panel: imported Lightroom collection sets and collections,
//! read-only. Smart collections stay hidden until their rules are evaluated,
//! and Lightroom's own (the Quick Collection, unsaved creations) never show.
use crate::app::icons::{self, Icon};
use crate::app::theme;
use crate::catalog::{Collection, CollectionKind};
use eframe::egui::{self, Vec2};
use std::collections::{HashMap, HashSet};

pub(super) struct CollectionNode {
    pub id: i64,
    pub name: String,
    pub set: bool,
    pub count: usize,
    pub children: Vec<CollectionNode>,
}

/// The panel's tree, ordered as Lightroom orders it: sets first, then
/// collections, each by name. A set is left out when everything in it is
/// hidden, such as Lightroom's default "Smart Collections" set.
pub(super) fn tree(
    collections: &[Collection],
    counts: &HashMap<i64, HashSet<i64>>,
) -> Vec<CollectionNode> {
    let mut children: HashMap<Option<i64>, Vec<&Collection>> = HashMap::new();
    let ids: HashSet<i64> = collections.iter().map(|c| c.id).collect();
    for c in collections {
        // A parent that isn't in the catalog puts the collection at the top.
        let parent = c.parent.filter(|p| ids.contains(p) && *p != c.id);
        children.entry(parent).or_default().push(c);
    }
    let mut seen = HashSet::new();
    nodes(None, &children, counts, &mut seen)
}

fn nodes(
    parent: Option<i64>,
    children: &HashMap<Option<i64>, Vec<&Collection>>,
    counts: &HashMap<i64, HashSet<i64>>,
    seen: &mut HashSet<i64>,
) -> Vec<CollectionNode> {
    let mut out = Vec::new();
    for c in children.get(&parent).into_iter().flatten() {
        // A parent cycle in a damaged catalog would otherwise never end.
        if !seen.insert(c.id) {
            continue;
        }
        match c.kind {
            CollectionKind::Set => {
                let inner = nodes(Some(c.id), children, counts, seen);
                let had_children = children.get(&Some(c.id)).is_some_and(|v| !v.is_empty());
                if inner.is_empty() && had_children {
                    continue;
                }
                out.push(CollectionNode {
                    id: c.id,
                    name: c.name.clone(),
                    set: true,
                    count: 0,
                    children: inner,
                });
            }
            CollectionKind::Collection => out.push(CollectionNode {
                id: c.id,
                name: c.name.clone(),
                set: false,
                count: counts.get(&c.id).map_or(0, HashSet::len),
                children: Vec::new(),
            }),
            CollectionKind::Smart | CollectionKind::System => {}
        }
    }
    out.sort_by_cached_key(|n| (!n.set, n.name.to_lowercase()));
    out
}

/// The key that marks a set as collapsed in the Library's expanded set.
fn collapsed_key(id: i64) -> String {
    format!("collection-set-collapsed:{id}")
}

/// Draws `node` and, unless it is a collapsed set, its children. Returns a
/// collection that was clicked.
pub(super) fn collection_row(
    ui: &mut egui::Ui,
    node: &CollectionNode,
    depth: usize,
    expanded: &mut HashSet<String>,
    selected: Option<i64>,
) -> Option<i64> {
    use egui::{Align2, FontId, Pos2, Rect, Sense};
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.), Sense::click());
    let painter = ui.painter();
    let active = !node.set && selected == Some(node.id);
    let collapsed = expanded.contains(&collapsed_key(node.id));
    if active || response.hovered() {
        painter.rect_filled(
            rect,
            3.,
            if active {
                theme::selected_row()
            } else {
                theme::gray(43)
            },
        );
    }
    if active {
        painter.rect_filled(
            Rect::from_min_size(rect.min, Vec2::new(2., rect.height())),
            0.,
            theme::selected_marker(),
        );
    }
    // The same columns as the folder tree.
    let indent = depth.min(12) as f32 * 14.;
    let x = rect.left() + indent + 3.;
    let y = rect.center().y;
    if node.set && !node.children.is_empty() {
        let chevron = if collapsed {
            Icon::ChevronRight
        } else {
            Icon::ChevronDown
        };
        icons::paint_at(painter, chevron, Pos2::new(x, y), 11., theme::gray(150));
    }
    icons::paint_at(
        painter,
        if node.set {
            Icon::CollectionSet
        } else {
            Icon::Collection
        },
        Pos2::new(x + 13., y),
        13.,
        theme::gray(145),
    );
    let label_rect = Rect::from_min_max(
        Pos2::new(x + 26., rect.top()),
        Pos2::new(rect.right() - 40., rect.bottom()),
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
        theme::gray(if active { 235 } else { 190 }),
    );
    // Lightroom counts collections, not sets.
    if !node.set {
        painter.text(
            Pos2::new(rect.right() - 10., y),
            Align2::RIGHT_CENTER,
            node.count.to_string(),
            FontId::proportional(10.),
            theme::gray(125),
        );
    }
    let mut clicked = None;
    if response.clicked() {
        if node.set {
            let key = collapsed_key(node.id);
            if collapsed {
                expanded.remove(&key);
            } else {
                expanded.insert(key);
            }
        } else {
            clicked = Some(node.id);
        }
    }
    if node.set && !expanded.contains(&collapsed_key(node.id)) {
        for child in &node.children {
            if let Some(id) = collection_row(ui, child, depth + 1, expanded, selected) {
                clicked = Some(id);
            }
        }
    }
    clicked
}
