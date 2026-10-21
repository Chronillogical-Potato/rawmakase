//! Renaming (planned in #556): Rename Photo (F2 on one photo), Rename
//! Photos (F2 on several, numbered in the order they were taken, #342) and
//! Rename Folder…
//!
//! Renaming photos holds the catalog while its dialog is open, as a
//! confirmation does: the open edit is saved and `Activity::ChangingFiles`
//! taken first, and the photos' order is read once, on a worker, then kept.
//! What the dialog would do is planned as it is typed, so a name taken, or
//! one the system can't have, is said at once; Rename runs that plan, which
//! is its confirmation (a rename keeps every byte, and is undone by another
//! rename). Rename Folder… holds nothing until Rename, which plans and runs.
use super::super::{Editor, widgets};
use super::{State, Verb};
use crate::catalog::file_ops::{self, FileOp, Plan, RenameOrder, Skip};
use crate::catalog::{CatalogLocation, PhotoId, RootId};
use crate::export_settings::{NameTemplate, Naming};
use eframe::egui::{self, Vec2};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Rename Photo or Rename Photos, open.
pub(super) struct Renaming {
    catalog: CatalogLocation,
    /// The session's when it opened: a catalog read again since may have
    /// given its photos' ids to others.
    generation: u64,
    order: RenameOrder,
    /// One photo: its new name before the extension, and the extension.
    stem: String,
    extension: String,
    /// Several: how they are named.
    template: NameTemplate,
    planned: Option<Planned>,
    focused: bool,
}

/// What the dialog's names would do, planned for the inputs they came from.
struct Planned {
    stem: String,
    template: NameTemplate,
    rows: Vec<Row>,
    /// The plan of exactly these names, when it renames anything and
    /// nothing blocks it.
    plan: Option<Plan>,
}

/// A photo's name now and as renamed, and why it can't be.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    old: String,
    new: String,
    problem: Option<String>,
}

/// What a rename dialog shows, for tests.
#[cfg(test)]
pub(super) struct RenameShown {
    pub title: String,
    pub rows: Vec<(String, String, Option<String>)>,
    pub can_rename: bool,
}

/// Rename Folder…, open. Holds nothing.
pub(super) struct RenamingFolder {
    catalog: CatalogLocation,
    root: RootId,
    /// Its logical path, and its name now.
    logical: String,
    old: String,
    name: String,
    focused: bool,
}

impl Renaming {
    fn one(&self) -> bool {
        self.order.photos().len() == 1
    }

    fn title(&self) -> String {
        match self.order.photos() {
            [one] => format!("Rename Photo “{}”", one.filename),
            photos => format!("Rename {} Photos", photos.len()),
        }
    }

    /// Plans what the inputs name, unless they are what was planned.
    fn replan(&mut self, catalog: &crate::catalog::Catalog) {
        if self
            .planned
            .as_ref()
            .is_some_and(|p| p.stem == self.stem && p.template == self.template)
        {
            return;
        }
        let (rows, plan) = if self.one() {
            self.plan_one(catalog)
        } else {
            self.plan_several(catalog)
        };
        self.planned = Some(Planned {
            stem: self.stem.clone(),
            template: self.template.clone(),
            rows,
            plan,
        });
    }

    fn plan_one(&self, catalog: &crate::catalog::Catalog) -> (Vec<Row>, Option<Plan>) {
        let photo = &self.order.photos()[0];
        let new = if self.extension.is_empty() {
            self.stem.clone()
        } else {
            format!("{}.{}", self.stem, self.extension)
        };
        let row = |problem: Option<String>| Row {
            old: photo.filename.clone(),
            new: new.clone(),
            problem,
        };
        if let Some(problem) = file_ops::photo_name_problem(&self.stem) {
            return (vec![row(Some(problem.to_string()))], None);
        }
        let op = FileOp::RenamePhotos {
            renames: vec![(photo.photo, self.stem.clone())],
        };
        match file_ops::plan(catalog, op) {
            Ok(plan) => match plan.skipped().first() {
                Some(s) => (vec![row(Some(s.reason.to_string()))], None),
                None if plan.is_empty() => (vec![row(None)], None),
                None => (vec![row(None)], Some(plan)),
            },
            Err(e) => (vec![row(Some(e.to_string()))], None),
        }
    }

    fn plan_several(&self, catalog: &crate::catalog::Catalog) -> (Vec<Row>, Option<Plan>) {
        match file_ops::rename_preview(catalog, &self.order, &self.template) {
            Ok(preview) => {
                let rows: Vec<Row> = preview
                    .rows
                    .iter()
                    .map(|r| Row {
                        old: r.old.clone(),
                        new: r.new.clone(),
                        problem: r
                            .problem
                            .as_ref()
                            .filter(|p| **p != Skip::SameName)
                            .map(ToString::to_string)
                            .or_else(|| r.note.clone()),
                    })
                    .collect();
                let blocked = preview
                    .rows
                    .iter()
                    .any(|r| r.problem.as_ref().is_some_and(|p| *p != Skip::SameName));
                let plan = (!blocked && !preview.plan.is_empty()).then_some(preview.plan);
                (rows, plan)
            }
            Err(e) => {
                let rows = self
                    .order
                    .photos()
                    .iter()
                    .map(|o| Row {
                        old: o.filename.clone(),
                        new: o.filename.clone(),
                        problem: Some(e.to_string()),
                    })
                    .collect();
                (rows, None)
            }
        }
    }
}

/// How many digits numbering `count` photos from `start` takes.
fn digits(start: u32, count: usize) -> usize {
    (start as usize + count.saturating_sub(1))
        .max(1)
        .to_string()
        .len()
}

impl Editor {
    /// F2 and Rename Photo…: renames `photos`, one by its name, several by a
    /// template in the order they were taken. Saves what is open and holds
    /// the catalog while their order is read and the dialog is open.
    pub(in crate::app) fn request_rename(&mut self, photos: Vec<PhotoId>) {
        if photos.is_empty() {
            return;
        }
        if self.activity.is_busy() || !matches!(self.file_ops.state, State::Idle) {
            self.status = "Finish what is running first".into();
            return;
        }
        let Some(location) = self
            .library
            .as_ref()
            .map(|l| l.session.catalog.location().clone())
        else {
            return;
        };
        if !self.flush() {
            self.status = "The open photo's edit could not be saved; nothing was renamed".into();
            return;
        }
        if !self.activity.begin_file_change() {
            return;
        }
        if let Some(library) = &mut self.library {
            library.suspend_background_reads();
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.file_ops.state = State::Ordering {
            catalog: location.clone(),
            cancel: cancel.clone(),
        };
        let failed = (location.clone(), cancel.clone());
        super::super::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let ordered = crate::catalog::Catalog::open(&location)
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|catalog| {
                        file_ops::rename_order(&catalog, &photos, &cancel)
                            .map_err(|e| e.to_string())
                    });
                let _ = tx.send(super::super::worker::Event::RenameOrdered(
                    location,
                    cancel,
                    Box::new(ordered),
                ));
            },
            move |tx, message| {
                let _ = tx.send(super::super::worker::Event::RenameOrdered(
                    failed.0,
                    failed.1,
                    Box::new(Err(message)),
                ));
            },
        );
    }

    /// The photos' order came back: the dialog opens, or nothing can be
    /// renamed.
    pub(in crate::app) fn rename_ordered(
        &mut self,
        location: CatalogLocation,
        read_with: Arc<AtomicBool>,
        ordered: Result<RenameOrder, String>,
    ) {
        // Only the reading under way, not one cancelled before it.
        let ours = matches!(
            &self.file_ops.state,
            State::Ordering { cancel, .. } if Arc::ptr_eq(cancel, &read_with)
        );
        if !ours {
            return;
        }
        let State::Ordering { catalog, .. } = std::mem::take(&mut self.file_ops.state) else {
            return;
        };
        if location != catalog || !self.showing_catalog(&catalog) {
            self.release_files();
            return;
        }
        let order = match ordered {
            Ok(order) if !order.photos().is_empty() => order,
            Ok(_) => {
                self.status = "Nothing to rename".into();
                self.release_files();
                return;
            }
            Err(e) => {
                self.status = format!("Photos not renamed: {e}");
                self.release_files();
                return;
            }
        };
        let generation = self.library.as_ref().map_or(0, |l| l.session.generation());
        let first = std::path::Path::new(&order.photos()[0].filename);
        let stem = first
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let extension = first
            .extension()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let count = order.photos().len();
        self.file_ops.state = State::Renaming(Box::new(Renaming {
            catalog,
            generation,
            order,
            stem,
            extension,
            template: NameTemplate {
                naming: Naming::CustomNameSequence,
                text: String::new(),
                start: 1,
                padding: digits(1, count),
            },
            planned: None,
            focused: false,
        }));
    }

    /// While the photos' order is read: Cancel (or Escape) stops it and
    /// lets the catalog go.
    pub(super) fn ordering_window(&mut self, ctx: &egui::Context) {
        let palette = super::super::theme::palette(ctx);
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("rename-ordering"))
            .frame(widgets::modal_frame(&palette).inner_margin(24))
            .show(ctx, |ui| {
                ui.set_width(360.);
                ui.label(
                    egui::RichText::new("Reading when the photos were taken…")
                        .size(14.)
                        .color(palette.gray(230)),
                );
                ui.add_space(16.);
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        if cancel || response.should_close() {
            self.cancel_ordering();
        }
    }

    /// Stops reading the photos' order, and lets the catalog go.
    pub(in crate::app) fn cancel_ordering(&mut self) {
        if let State::Ordering { cancel, .. } = &self.file_ops.state {
            cancel.store(true, Ordering::Relaxed);
            self.file_ops.state = State::Idle;
            self.release_files();
        }
    }

    /// Rename Photo / Rename Photos: the name or the template, each photo's
    /// new name with what is wrong with it, and Rename.
    pub(super) fn rename_window(&mut self, ctx: &egui::Context) {
        let (State::Renaming(r), Some(library)) = (&mut self.file_ops.state, &self.library) else {
            return;
        };
        if library.session.catalog.location() != &r.catalog {
            self.file_ops.state = State::Idle;
            self.release_files();
            return;
        }
        r.replan(&library.session.catalog);
        let palette = super::super::theme::palette(ctx);
        let mut rename = false;
        let mut cancel = false;
        let can_rename = r.planned.as_ref().is_some_and(|p| p.plan.is_some());
        let title = r.title();
        let response = egui::Modal::new(egui::Id::new("rename-photos"))
            .frame(widgets::modal_frame(&palette).inner_margin(24))
            .show(ctx, |ui| {
                ui.set_width(if r.one() { 420. } else { 520. });
                ui.label(egui::RichText::new(&title).size(15.).color(palette.gray(236)));
                ui.add_space(10.);
                let field = if r.one() {
                    let field = ui.horizontal(|ui| {
                        let output = egui::TextEdit::singleline(&mut r.stem)
                            .desired_width(330.)
                            .show(ui);
                        if !r.extension.is_empty() {
                            ui.label(
                                egui::RichText::new(format!(".{}", r.extension))
                                    .color(palette.gray(150)),
                            );
                        }
                        output
                    });
                    field.inner
                } else {
                    let output = ui
                        .horizontal(|ui| {
                            ui.label("Name");
                            egui::ComboBox::from_id_salt("rename-naming")
                                .selected_text(r.template.naming.label())
                                .show_ui(ui, |ui| {
                                    for naming in Naming::ALL {
                                        ui.selectable_value(
                                            &mut r.template.naming,
                                            naming,
                                            naming.label(),
                                        );
                                    }
                                });
                            egui::TextEdit::singleline(&mut r.template.text)
                                .hint_text("Custom text")
                                .desired_width(200.)
                                .show(ui)
                        })
                        .inner;
                    ui.horizontal(|ui| {
                        ui.label("Start number");
                        ui.add(egui::DragValue::new(&mut r.template.start).range(0..=999_999));
                        ui.label("Digits");
                        ui.add(egui::DragValue::new(&mut r.template.padding).range(0..=9));
                    });
                    output
                };
                if !r.focused {
                    field.response.request_focus();
                    // The name is typed over, not added to.
                    let mut state = field.state.clone();
                    let end = egui::text::CCursor::new(field.galley.text().chars().count());
                    state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(0),
                        end,
                    )));
                    state.store(ui.ctx(), field.response.id);
                    r.focused = true;
                }
                if field.response.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    && can_rename
                {
                    rename = true;
                }
                ui.add_space(8.);
                if let Some(planned) = &r.planned {
                    let rows = |ui: &mut egui::Ui| {
                        for row in &planned.rows {
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&row.old)
                                        .size(12.)
                                        .color(palette.gray(150)),
                                );
                                ui.label(egui::RichText::new("→").size(12.).color(palette.gray(110)));
                                ui.label(
                                    egui::RichText::new(&row.new)
                                        .size(12.)
                                        .color(palette.gray(220)),
                                );
                            });
                            if let Some(problem) = &row.problem {
                                ui.label(
                                    egui::RichText::new(problem)
                                        .size(12.)
                                        .color(palette.warning()),
                                );
                            }
                        }
                    };
                    if r.order.photos().len() > 1 {
                        egui::ScrollArea::vertical().max_height(260.).show(ui, rows);
                        ui.add_space(4.);
                        ui.label(
                            egui::RichText::new(
                                "Numbered in the order the photos were taken; those without a capture time come last.",
                            )
                            .size(12.)
                            .color(palette.gray(140)),
                        );
                    } else if planned.rows.iter().any(|r| r.problem.is_some()) {
                        rows(ui);
                    }
                }
                ui.add_space(4.);
                ui.label(
                    egui::RichText::new("Their sidecar files are renamed to match.")
                        .size(12.)
                        .color(palette.gray(140)),
                );
                ui.add_space(16.);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = Vec2::new(14., 6.);
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui
                        .add_enabled_ui(can_rename, |ui| widgets::primary_button(ui, "Rename"))
                        .inner
                        .clicked()
                    {
                        rename = true;
                    }
                });
            });
        if cancel || response.should_close() {
            self.file_ops.state = State::Idle;
            self.release_files();
            return;
        }
        if rename {
            // What this frame typed is planned before it is run: Return may
            // come with the last of the text.
            if let (State::Renaming(r), Some(library)) = (&mut self.file_ops.state, &self.library) {
                r.replan(&library.session.catalog);
                // What it typed last can't be had: the dialog stays, and
                // says why.
                if r.planned.as_ref().is_none_or(|p| p.plan.is_none()) {
                    return;
                }
            }
            let State::Renaming(r) = std::mem::take(&mut self.file_ops.state) else {
                return;
            };
            let Some(Planned {
                plan: Some(plan),
                rows,
                ..
            }) = r.planned
            else {
                // Not reached (checked above); the catalog is let go anyway.
                self.release_files();
                return;
            };
            let verb = Verb::Rename {
                one: match rows.as_slice() {
                    [one] => Some((one.old.clone(), one.new.clone())),
                    _ => None,
                },
            };
            self.file_ops.state = State::Confirming {
                plan: Box::new(plan),
                trash: None,
                catalog: r.catalog,
                generation: r.generation,
                verb,
            };
            self.confirm_file_op();
        }
    }

    /// Rename Folder… for the folder of `root` at logical path `logical`:
    /// asks its new name.
    pub(in crate::app) fn rename_folder(&mut self, root: RootId, logical: String) {
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
        let old = logical.rsplit('/').next().unwrap_or_default().to_string();
        self.file_ops.state = State::RenamingFolder(Box::new(RenamingFolder {
            catalog,
            root,
            logical,
            name: old.clone(),
            old,
            focused: false,
        }));
    }

    /// Rename Folder…: its new name, checked as it is typed.
    pub(super) fn rename_folder_window(&mut self, ctx: &egui::Context) {
        let State::RenamingFolder(f) = &mut self.file_ops.state else {
            return;
        };
        let palette = super::super::theme::palette(ctx);
        let problem = file_ops::folder_name_problem(&f.name);
        let valid = problem.is_none() && f.name != f.old;
        let mut rename = false;
        let mut cancel = false;
        let response = egui::Modal::new(egui::Id::new("rename-folder"))
            .frame(widgets::modal_frame(&palette).inner_margin(24))
            .show(ctx, |ui| {
                ui.set_width(420.);
                ui.label(
                    egui::RichText::new(format!("Rename Folder “{}”", f.old))
                        .size(15.)
                        .color(palette.gray(236)),
                );
                ui.add_space(10.);
                let field = egui::TextEdit::singleline(&mut f.name)
                    .desired_width(f32::INFINITY)
                    .show(ui);
                if !f.focused {
                    field.response.request_focus();
                    let mut state = field.state.clone();
                    let end = egui::text::CCursor::new(field.galley.text().chars().count());
                    state
                        .cursor
                        .set_char_range(Some(egui::text::CCursorRange::two(
                            egui::text::CCursor::new(0),
                            end,
                        )));
                    state.store(ui.ctx(), field.response.id);
                    f.focused = true;
                }
                if field.response.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    && valid
                {
                    rename = true;
                }
                if let Some(problem) = problem.filter(|_| f.name != f.old) {
                    ui.add_space(4.);
                    ui.label(
                        egui::RichText::new(problem)
                            .size(12.)
                            .color(palette.warning()),
                    );
                }
                ui.add_space(4.);
                ui.label(
                    egui::RichText::new(
                        "The folder is renamed on disk; its photos and the folders in it follow.",
                    )
                    .size(12.)
                    .color(palette.gray(140)),
                );
                ui.add_space(16.);
                ui.horizontal(|ui| {
                    ui.spacing_mut().button_padding = Vec2::new(14., 6.);
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                    if ui
                        .add_enabled_ui(valid, |ui| widgets::primary_button(ui, "Rename"))
                        .inner
                        .clicked()
                    {
                        rename = true;
                    }
                });
            });
        if cancel || response.should_close() {
            self.file_ops.state = State::Idle;
            return;
        }
        if rename {
            let State::RenamingFolder(f) = std::mem::take(&mut self.file_ops.state) else {
                return;
            };
            if !self.showing_catalog(&f.catalog) {
                return;
            }
            let parent = f
                .logical
                .rsplit_once('/')
                .map_or(String::new(), |(p, _)| p.to_string());
            let op = FileOp::RenameFolder {
                root: f.root,
                folder: f.logical.clone(),
                name: f.name.clone(),
            };
            let verb = Verb::RenameFolder {
                root: f.root,
                from: f.logical,
                to: if parent.is_empty() {
                    f.name.clone()
                } else {
                    format!("{parent}/{}", f.name)
                },
            };
            self.request_file_op(op, None, verb);
        }
    }
}

impl super::FileOps {
    /// The cancel flag of the order being read, while it is.
    #[cfg(test)]
    pub(super) fn ordering_cancel(&self) -> Option<Arc<AtomicBool>> {
        match &self.state {
            State::Ordering { cancel, .. } => Some(cancel.clone()),
            _ => None,
        }
    }
    /// The open rename dialog, as it shows.
    #[cfg(test)]
    pub(super) fn rename_shown(&self) -> Option<RenameShown> {
        let State::Renaming(r) = &self.state else {
            return None;
        };
        let planned = r.planned.as_ref()?;
        Some(RenameShown {
            title: r.title(),
            rows: planned
                .rows
                .iter()
                .map(|row| (row.old.clone(), row.new.clone(), row.problem.clone()))
                .collect(),
            can_rename: planned.plan.is_some(),
        })
    }
}
