//! Moving a folder (planned in #556): dragged onto another folder of its
//! root in the Folders panel, or Move Folder… in its menu, which lists
//! where it can go. Either is confirmed as moving photos is, Cancel
//! focused; the catalog's folder stays the same folder, its photos follow.
use super::super::{Editor, widgets};
use super::{State, Verb};
use crate::catalog::file_ops::FileOp;
use crate::catalog::{CatalogLocation, RootId};
use eframe::egui::{self, Vec2};

/// Move Folder…, while where it goes is chosen. Holds nothing.
pub(super) struct ChoosingParent {
    catalog: CatalogLocation,
    root: RootId,
    /// Its logical path.
    from: String,
    filter: String,
    /// The logical path chosen ("" for the root).
    chosen: Option<String>,
}

/// The last name of logical path `path`.
fn last(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or_default()
}

/// Root `root`'s name, as the Folders panel shows it: its folder's.
fn root_name(roots: &[(RootId, String, Option<String>)], root: RootId) -> String {
    roots
        .iter()
        .find(|r| r.0 == root)
        .and_then(|r| std::path::Path::new(&r.1).file_name())
        .map_or_else(
            || "Folder".to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
}

/// Where Move Folder… may put `c`'s folder, as logical paths ("" for the
/// root) and labels: its root, and every folder of the root the Folders
/// panel shows (those with no catalog folder of their own too, such as
/// `2025` above `2025/day2`), but where it is, itself and the folders in it.
fn parent_rows(
    library: &super::super::library::Library,
    c: &ChoosingParent,
    root_name: &str,
) -> Vec<(String, String)> {
    let parent = c.from.rsplit_once('/').map_or("", |(p, _)| p);
    let inside = format!("{}/", c.from);
    let mut logical: Vec<String> = vec![String::new()];
    for f in library.session.folders.iter().filter(|f| f.root == c.root) {
        let mut at = String::new();
        for name in f.relative.split('/').filter(|n| !n.is_empty()) {
            at = join(&at, name);
            logical.push(at.clone());
        }
    }
    logical.sort();
    logical.dedup();
    let mut rows: Vec<(String, String)> = logical
        .into_iter()
        .filter(|l| l != parent && *l != c.from && !l.starts_with(&inside))
        .map(|l| {
            let label = if l.is_empty() {
                root_name.to_string()
            } else {
                format!("{root_name} / {}", l.replace('/', " / "))
            };
            (l, label)
        })
        .collect();
    rows.sort_by_key(|(_, label)| label.to_lowercase());
    rows
}

/// Logical path `parent`, then `name`.
fn join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}

impl Editor {
    /// What Move Folder… offers, as logical paths.
    #[cfg(test)]
    pub(super) fn parent_choices(&self) -> Vec<String> {
        match (&self.file_ops.state, &self.library) {
            (State::ChoosingParent(c), Some(library)) => {
                let name = root_name(&library.session.roots, c.root);
                parent_rows(library, c, &name)
                    .into_iter()
                    .map(|(l, _)| l)
                    .collect()
            }
            _ => Vec::new(),
        }
    }
    /// Moves the folder of `root` at logical path `from` into logical
    /// folder `to` of the same root ("" for the root), once confirmed.
    pub(in crate::app) fn request_folder_move(&mut self, root: RootId, from: String, to: String) {
        let into = match last(&to) {
            "" => self
                .library
                .as_ref()
                .map_or_else(String::new, |l| root_name(&l.session.roots, root)),
            name => name.to_string(),
        };
        let name = last(&from).to_string();
        let op = FileOp::MoveFolder {
            root,
            folder: from.clone(),
            parent: to.clone(),
        };
        let verb = Verb::MoveFolder {
            root,
            to: join(&to, &name),
            from,
            name,
            into,
        };
        self.request_file_op(op, None, verb);
    }

    /// Move Folder… for the folder of `root` at logical path `from`: asks
    /// where it goes.
    pub(in crate::app) fn choose_parent_for(&mut self, root: RootId, from: String) {
        if self.activity.is_busy() || !matches!(self.file_ops.state, State::Idle) {
            self.status = "Finish what is running first".into();
            return;
        }
        let Some(catalog) = self
            .library
            .as_ref()
            .map(|l| l.session.catalog.location().clone())
        else {
            return;
        };
        self.file_ops.state = State::ChoosingParent(Box::new(ChoosingParent {
            catalog,
            root,
            from,
            filter: String::new(),
            chosen: None,
        }));
    }

    /// Move Folder…'s window: its root and the root's folders it can go
    /// into, filtered by name.
    pub(super) fn choose_parent_window(&mut self, ctx: &egui::Context) {
        let (State::ChoosingParent(c), Some(library)) = (&mut self.file_ops.state, &self.library)
        else {
            self.file_ops.state = State::Idle;
            return;
        };
        if library.session.catalog.location() != &c.catalog {
            self.file_ops.state = State::Idle;
            return;
        }
        let palette = super::super::theme::palette(ctx);
        let root_name = root_name(&library.session.roots, c.root);
        let mut rows = parent_rows(library, c, &root_name);
        let filter = c.filter.to_lowercase();
        rows.retain(|(_, label)| label.to_lowercase().contains(&filter));
        let mut go = None;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("move-folder"))
            .frame(widgets::modal_frame(&palette).inner_margin(24))
            .show(ctx, |ui| {
                ui.set_width(460.);
                ui.label(
                    egui::RichText::new(format!("Move Folder “{}”", last(&c.from)))
                        .size(15.)
                        .color(palette.gray(236)),
                );
                ui.add_space(8.);
                ui.add(
                    egui::TextEdit::singleline(&mut c.filter)
                        .hint_text("Filter folders")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(6.);
                egui::ScrollArea::vertical()
                    .max_height(320.)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (logical, label) in &rows {
                            let row =
                                ui.selectable_label(c.chosen.as_ref() == Some(logical), label);
                            if row.clicked() {
                                c.chosen = Some(logical.clone());
                            }
                            if row.double_clicked() {
                                go = Some(logical.clone());
                            }
                        }
                        if rows.is_empty() {
                            ui.label(
                                egui::RichText::new("No folder of its root folder is named so.")
                                    .size(12.)
                                    .color(palette.gray(140)),
                            );
                        }
                    });
                ui.add_space(4.);
                ui.label(
                    egui::RichText::new("A folder moves within its root folder, on its drive.")
                        .size(12.)
                        .color(palette.gray(140)),
                );
                ui.add_space(16.);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = Vec2::new(14., 6.);
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    let chosen = c
                        .chosen
                        .clone()
                        .filter(|chosen| rows.iter().any(|(r, _)| r == chosen));
                    let clicked = ui
                        .add_enabled_ui(chosen.is_some(), |ui| widgets::primary_button(ui, "Move…"))
                        .inner
                        .clicked();
                    if clicked {
                        go = chosen;
                    }
                });
            });
        if cancel || response.should_close() {
            self.file_ops.state = State::Idle;
            return;
        }
        if let Some(to) = go {
            let State::ChoosingParent(c) = std::mem::take(&mut self.file_ops.state) else {
                return;
            };
            self.request_folder_move(c.root, c.from, to);
        }
    }
}
