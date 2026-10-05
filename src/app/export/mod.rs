//! Export commands. The dialog lives in `dialog`; the export itself (planning,
//! rendering, file writing) in `crate::export`. Every export, of one photo or the
//! whole selection, goes through the app's one export queue and runs while you keep
//! editing; the top bar shows its progress, and what could not be exported stays
//! there until it is read.
mod dialog;
mod watermark_editor;

use super::{Editor, worker::Event};
use crate::app::theme;
use crate::app::widgets::plural;
use crate::export::{
    Existing, ExportSettings,
    assemble::Values,
    batch::{self, BatchPhoto, Edit, Outcome, Unplanned},
    job::Photo,
    queue::Queue,
};
use eframe::egui::{self, Color32, Sense, Vec2};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicBool},
};

/// One photo an Export acts on.
#[derive(Clone, Debug)]
struct Chosen {
    /// Its catalog id; `None` for a photo opened without a catalog.
    id: Option<i64>,
    source: PathBuf,
    name: String,
    /// The photo open in Develop: exported with its edit as shown.
    open: bool,
}

/// The photos an Export acts on, fixed when it is chosen.
#[derive(Clone, Debug, Default)]
struct Scope {
    photos: Vec<Chosen>,
    /// Photos chosen that can't be exported: their names and why.
    left_out: Vec<(String, String)>,
}

/// A batch whose files already exist, waiting for the answer to Ask.
struct Pending {
    batch: Vec<BatchPhoto>,
    settings: ExportSettings,
    watermark: Option<crate::watermark::Watermark>,
    left_out: Vec<(String, String)>,
    conflicts: Vec<PathBuf>,
}

/// What a queued batch's outcomes are reported against.
struct Queued {
    names: Vec<String>,
    left_out: Vec<(String, String)>,
}

/// What finished exports could not do, kept until it is dismissed.
#[derive(Clone, Debug, Default)]
pub(super) struct Summary {
    exported: usize,
    total: usize,
    /// Photos exported with something to say.
    noted: usize,
    /// Photos not exported, with why.
    problems: Vec<(String, String)>,
    /// What exported photos had to say, each note on its own.
    notes: Vec<(String, String)>,
}
impl Summary {
    /// "Exported 148 of 150 · 2 not exported · 1 with notes".
    fn line(&self) -> String {
        let mut line = format!("Exported {} of {}", self.exported, self.total);
        if !self.problems.is_empty() {
            line.push_str(&format!(" · {} not exported", self.problems.len()));
        }
        if self.noted > 0 {
            line.push_str(&format!(" · {} with notes", self.noted));
        }
        line
    }
    fn add(&mut self, other: Summary) {
        self.exported += other.exported;
        self.total += other.total;
        self.noted += other.noted;
        self.problems.extend(other.problems);
        self.notes.extend(other.notes);
    }
}

#[derive(Default)]
pub(super) struct Exports {
    /// The dialog is open, editing `draft` for `scope`.
    dialog: bool,
    draft: ExportSettings,
    scope: Scope,
    /// The folder chooser's answer, from its own thread.
    picked: Arc<Mutex<Option<PathBuf>>>,
    picking: Arc<AtomicBool>,
    /// The Ask question for a batch's existing files.
    pending: Option<Pending>,
    queue: Option<Queue>,
    queued: HashMap<u64, Queued>,
    /// The last export's report, while it has something to say.
    summary: Option<Summary>,
    /// The Problem Exporting Files window is open.
    report: bool,
    /// The Watermark Editor, over the dialog.
    watermark_editor: Option<watermark_editor::WatermarkEditor>,
    /// Saved watermarks, read when the dialog opens and after the editor
    /// saves one.
    watermarks: Vec<crate::watermark::Watermark>,
}

impl Editor {
    /// Export…: the dialog for the photos chosen now, starting from the last
    /// export's choices.
    pub(super) fn open_export_dialog(&mut self) {
        let Some(scope) = self.export_scope() else {
            return;
        };
        self.exports.scope = scope;
        self.exports.draft = ExportSettings::load().unwrap_or_default();
        self.exports.watermarks = crate::watermark::presets();
        self.exports.dialog = true;
    }
    /// Export with Previous: the last export's choices, without the dialog.
    pub(super) fn export_with_previous(&mut self) {
        match ExportSettings::load() {
            Some(settings) => {
                if let Some(scope) = self.export_scope() {
                    self.exports.scope = scope;
                    self.export(settings);
                }
            }
            None => self.open_export_dialog(),
        }
    }
    /// The Export dialog or its existing-file question is showing.
    pub(super) fn export_modal(&self) -> bool {
        self.exports.dialog
            || self.exports.pending.is_some()
            || self.exports.watermark_editor.is_some()
            || self.exports.report
    }
    /// An export is running or waiting.
    pub(super) fn exporting(&self) -> bool {
        self.exports.queue.as_ref().is_some_and(Queue::busy)
    }

    /// The photos Export acts on: the Library's selection, or in Develop the
    /// Filmstrip's when the open photo is part of it, else the open photo. Says why
    /// and returns `None` when there is nothing to export, or the open photo is
    /// among them and its edit is not final yet.
    fn export_scope(&mut self) -> Option<Scope> {
        let open = self.document.catalog_photo;
        let ids = match &self.library {
            Some(l) if self.library_mode => l.selected_photos(),
            Some(l) => match open {
                Some(open) if l.is_selected(open) => l.selected_photos(),
                Some(open) => vec![open],
                None => Vec::new(),
            },
            None => Vec::new(),
        };
        let shown = self.document.path.is_some() && self.document.full().is_some();
        let mut scope = Scope::default();
        match &self.library {
            Some(l) if !ids.is_empty() => {
                for id in ids {
                    let Some(photo) = l.photo(id) else {
                        continue;
                    };
                    let name = format!(
                        "{}{}",
                        photo.filename,
                        crate::app::library::copy_suffix(photo)
                    );
                    let open = open == Some(id) && shown;
                    // The open photo too: it is decoded again from its file.
                    match l.export_refusal(id) {
                        Some(refusal) => scope.left_out.push((name, refusal.label())),
                        None => scope.photos.push(Chosen {
                            id: Some(id),
                            source: photo.path.clone(),
                            name,
                            open,
                        }),
                    }
                }
            }
            // A photo opened without a catalog, still where it was opened from.
            _ if shown && open.is_none() => {
                let source = self.document.path.clone().unwrap_or_default();
                if !source.is_file() {
                    self.status = format!("{} can't be exported: Offline", source.display());
                    return None;
                }
                scope.photos.push(Chosen {
                    id: None,
                    name: source
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into(),
                    source,
                    open: true,
                });
            }
            _ => {}
        }
        if scope.photos.is_empty() {
            self.status = match scope.left_out.as_slice() {
                [] => "Select a photo to export".into(),
                [(name, why)] => format!("{name} can't be exported: {why}"),
                left_out => format!("None of the {} photos can be exported", left_out.len()),
            };
            return None;
        }
        if scope.photos.iter().any(|p| p.open)
            && let Some(why) = self.open_photo_unready()
        {
            self.status = format!("Export waits for the open photo: {why}");
            return None;
        }
        Some(scope)
    }

    /// Why the open photo's edit is not final yet, if it is not: what it shows
    /// would not be what is exported.
    fn open_photo_unready(&self) -> Option<&'static str> {
        let d = &self.document;
        if d.metadata.is_none() || d.full().is_none() {
            Some("it is still loading")
        } else if d.pending_lightroom.is_some() {
            Some("its Lightroom edit is still being applied")
        } else if d.auto.is_running() {
            Some("Auto is still working on it")
        } else if d.save.is_protected() {
            Some("its edit is protected, as the file changed since it was saved")
        } else if self.importing.is_some() {
            Some("camera profiles are still being imported")
        } else if d.upright.is_running() || d.recipe.upright.needs_analysis() {
            Some("Upright is still analysing it")
        } else {
            None
        }
    }

    /// The open photo as it is now, with its catalog rating, label and keywords.
    pub(super) fn export_photo(&mut self) -> Option<Photo> {
        let catalog = self
            .document
            .catalog_photo
            .and_then(|id| self.library.as_ref()?.photo(id));
        let values = match (catalog, self.library.as_ref()) {
            (Some(p), Some(library)) => {
                let read = library.catalog.descriptive(p.id).and_then(|descriptive| {
                    let keywords = library.catalog.keywords(p.id)?;
                    Ok(Values {
                        descriptive,
                        keywords: keywords
                            .into_iter()
                            .map(|k| crate::xmp::write::KeywordPath {
                                path: k.path,
                                exported: k.exported,
                            })
                            .collect(),
                        rating: p.rating,
                        label: p.label.clone(),
                    })
                });
                read.map_err(|e| format!("Metadata could not be read for export: {e}"))
            }
            _ => Ok(Values::default()),
        };
        let values = match values {
            Ok(values) => values,
            Err(e) => {
                self.status = e;
                return None;
            }
        };
        Some(Photo {
            image: self.document.full()?.clone(),
            source: self.document.path.clone()?,
            recipe: self.document.recipe.clone(),
            values,
            watermark: None,
        })
    }

    /// Exports the scope chosen with `settings`: each photo as the catalog has it
    /// now, the open one as shown. Asks once about files that already exist.
    fn export(&mut self, settings: ExportSettings) {
        if let Err(e) = settings.save() {
            self.status = format!("Export settings not saved: {e:#}");
        }
        let scope = std::mem::take(&mut self.exports.scope);
        let watermark = if settings.watermark {
            match watermark_for(&settings.watermark_name) {
                Some(w) => Some(w),
                None => {
                    self.status = format!("Watermark not found: {}", settings.watermark_name);
                    return;
                }
            }
        } else {
            None
        };
        // Metadata typed in the Library goes in the files: not exported without it.
        if !self.commit_library_drafts() {
            self.status = format!("Export stopped: {}", self.status);
            return;
        }
        // The open photo is exported as shown; if its edit can't be saved, it is
        // still exported, and the report says so.
        let unsaved = scope.photos.iter().any(|p| p.open) && !self.flush();
        let batch = match self.batch_photos(&scope, unsaved) {
            Ok(batch) => batch,
            Err(e) => {
                self.status = format!("Export stopped: the catalog could not be read: {e:#}");
                return;
            }
        };
        self.plan_export(
            Pending {
                batch,
                settings,
                watermark,
                left_out: scope.left_out,
                conflicts: Vec::new(),
            },
            None,
        );
    }

    /// The scope's photos as the catalog has them now, read in one transaction.
    fn batch_photos(&self, scope: &Scope, unsaved: bool) -> anyhow::Result<Vec<BatchPhoto>> {
        let ids: Vec<i64> = scope.photos.iter().filter_map(|p| p.id).collect();
        let mut records = match &self.library {
            Some(l) if !ids.is_empty() => l.catalog.photo_records(&ids)?.into_iter(),
            _ => Vec::new().into_iter(),
        };
        Ok(scope
            .photos
            .iter()
            .map(|chosen| {
                let mut photo = match chosen.id {
                    Some(_) => BatchPhoto::from_record(
                        records.next().expect("a record for each photo"),
                        chosen.source.clone(),
                        chosen.name.clone(),
                    ),
                    None => BatchPhoto {
                        id: 0,
                        source: chosen.source.clone(),
                        name: chosen.name.clone(),
                        edit: Edit::Catalog(Default::default()),
                        values: Values::default(),
                    },
                };
                if chosen.open {
                    photo.edit = Edit::Shown {
                        recipe: Box::new(self.document.recipe.clone()),
                        unsaved,
                        file: self.document.file.clone(),
                    };
                }
                photo
            })
            .collect())
    }

    /// Plans where each file goes and queues the batch, or asks about the files
    /// that exist first.
    fn plan_export(&mut self, mut pending: Pending, answer: Option<Existing>) {
        match batch::plan(&pending.batch, &pending.settings, answer) {
            Ok(plan) => {
                let names = pending.batch.iter().map(|p| p.name.clone()).collect();
                let total = pending.batch.len();
                let batch = batch::Batch {
                    photos: pending.batch,
                    plan,
                    settings: pending.settings,
                    defaults: self.raw_defaults.clone(),
                    watermark: pending.watermark,
                };
                let ticket = self.export_queue().submit(batch);
                self.exports.queued.insert(
                    ticket,
                    Queued {
                        names,
                        left_out: pending.left_out,
                    },
                );
                self.status = format!("Exporting {}…", plural(total, "photo", "photos"));
            }
            Err(Unplanned::NoFolder) => self.status = "Choose a folder to export to".into(),
            Err(Unplanned::Conflicts(conflicts)) => {
                pending.conflicts = conflicts;
                self.exports.pending = Some(pending);
            }
        }
    }

    /// The export queue, started with the first export.
    fn export_queue(&mut self) -> &Queue {
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        let repaint = self.context.clone();
        self.exports.queue.get_or_insert_with(|| {
            Queue::new(
                move |ticket, outcomes| {
                    let _ = tx.send(Event::BatchExported { ticket, outcomes });
                    ctx.request_repaint();
                },
                move || repaint.request_repaint(),
            )
        })
    }

    /// A finished batch: the status line says how it went, and what could not be
    /// exported stays in the top bar until it is read.
    pub(super) fn batch_exported(&mut self, ticket: u64, outcomes: Vec<Outcome>) {
        let Some(queued) = self.exports.queued.remove(&ticket) else {
            return;
        };
        let (summary, single) = summarize(&queued, &outcomes);
        self.status = match single {
            Some(path) => format!("Exported {}", path.display()),
            None => summary.line(),
        };
        // Unread reports add up until they are dismissed: a later export that went
        // well never hides an earlier one that did not.
        if !summary.problems.is_empty() || !summary.notes.is_empty() {
            match &mut self.exports.summary {
                Some(unread) => unread.add(summary),
                None => self.exports.summary = Some(summary),
            }
        }
    }

    /// Lightroom's activity indicator: while exports run, "Exporting 24 / 150"
    /// over a bar, with a button that cancels the running one and a menu of those
    /// waiting; after one that could not export everything, what it could not do,
    /// until it is read.
    pub(super) fn export_progress(&mut self, ui: &mut egui::Ui) {
        let status = self.exports.queue.as_ref().map(Queue::status);
        let Some((ticket, progress)) = status.as_ref().and_then(|s| s.running) else {
            self.export_summary(ui);
            return;
        };
        let waiting = status.map(|s| s.waiting).unwrap_or_default();
        let (rect, response) = ui.allocate_exact_size(Vec2::new(210., 28.), Sense::click());
        let painter = ui.painter();
        let current = (progress.done + 1).min(progress.total);
        let mut label = format!("Exporting {current} / {}", progress.total);
        if !waiting.is_empty() {
            label.push_str(&format!(" · {} waiting", waiting.len()));
        }
        painter.text(
            egui::pos2(rect.left(), rect.top() + 7.),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(11.),
            theme::gray(190),
        );
        let fraction = (progress.done as f32 + progress.fraction) / progress.total.max(1) as f32;
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.top() + 17.),
            Vec2::new(rect.width() - 26., 4.),
        );
        painter.rect_filled(bar, 2., theme::gray(50));
        painter.rect_filled(
            egui::Rect::from_min_size(bar.min, Vec2::new(bar.width() * fraction.min(1.), 4.)),
            2.,
            Color32::from_rgb(110, 150, 190),
        );
        // The exports waiting, each of which can be removed before it starts.
        if !waiting.is_empty() {
            egui::Popup::menu(&response).show(|ui| {
                for (queued, photos) in &waiting {
                    let text = format!("Waiting: {}", plural(*photos, "photo", "photos"));
                    if ui.button(format!("Remove · {text}")).clicked()
                        && let Some(queue) = &self.exports.queue
                        && queue.remove(*queued)
                    {
                        self.exports.queued.remove(queued);
                        ui.close();
                    }
                }
            });
        }
        let close = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 9., rect.center().y),
            Vec2::splat(18.),
        );
        let cancel = ui
            .interact(close, ui.id().with("cancel-export"), Sense::click())
            .on_hover_text("Cancel export");
        let color = theme::gray(if cancel.hovered() { 235 } else { 150 });
        crate::app::icons::paint_at(
            ui.painter(),
            crate::app::icons::Icon::Close,
            close.center(),
            13.,
            color,
        );
        if cancel.clicked()
            && let Some(queue) = &self.exports.queue
        {
            queue.cancel(ticket);
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
    }

    /// The last export's report line, which opens Problem Exporting Files.
    fn export_summary(&mut self, ui: &mut egui::Ui) {
        let Some(summary) = &self.exports.summary else {
            return;
        };
        let line = egui::RichText::new(summary.line())
            .size(11.)
            .color(Color32::from_rgb(230, 170, 100));
        if ui
            .add(egui::Button::new(line).frame(false))
            .on_hover_text("Show what was not exported")
            .clicked()
        {
            self.exports.report = true;
        }
    }

    /// Lightroom's Problem Exporting Files: the photos not exported, by reason, and
    /// the notes on those that were.
    pub(super) fn export_report(&mut self, ctx: &egui::Context) {
        if !self.exports.report {
            return;
        }
        let Some(summary) = self.exports.summary.clone() else {
            self.exports.report = false;
            return;
        };
        let mut close = false;
        let mut dismiss = false;
        let response = egui::Modal::new(egui::Id::new("export-report"))
            .frame(super::widgets::modal_frame().inner_margin(24))
            .show(ctx, |ui| {
                ui.set_width(520.);
                ui.label(
                    egui::RichText::new("Problem Exporting Files")
                        .size(15.)
                        .color(theme::gray(236)),
                );
                ui.add_space(6.);
                if !summary.problems.is_empty() {
                    ui.label(
                        egui::RichText::new("Some export operations were not performed.")
                            .size(12.)
                            .color(theme::gray(170)),
                    );
                }
                ui.add_space(8.);
                egui::ScrollArea::vertical()
                    .max_height(320.)
                    .show(ui, |ui| {
                        for (title, rows) in [
                            ("Not exported", &summary.problems),
                            ("Exported, with notes", &summary.notes),
                        ] {
                            if rows.is_empty() {
                                continue;
                            }
                            ui.label(egui::RichText::new(title).size(12.).color(theme::gray(220)));
                            for (name, reason) in rows {
                                ui.label(
                                    egui::RichText::new(format!("{name}: {reason}"))
                                        .size(12.)
                                        .color(theme::gray(160)),
                                );
                            }
                            ui.add_space(8.);
                        }
                    });
                ui.add_space(12.);
                ui.horizontal(|ui| {
                    dismiss = ui.button("Dismiss").clicked();
                    close = super::widgets::primary_button(ui, "OK").clicked();
                });
            });
        if dismiss {
            self.exports.summary = None;
        }
        if close || dismiss || response.should_close() {
            self.exports.report = false;
        }
    }
}

/// How a batch went: the report, and the file when it was one photo exported
/// with nothing to say.
fn summarize(queued: &Queued, outcomes: &[Outcome]) -> (Summary, Option<PathBuf>) {
    let mut summary = Summary::default();
    let mut exported = Vec::new();
    for (name, outcome) in queued.names.iter().zip(outcomes) {
        let problem = match outcome {
            Outcome::Exported { path, notes } => {
                exported.push(path.clone());
                summary.noted += usize::from(!notes.is_empty());
                for note in notes {
                    summary.notes.push((name.clone(), note.clone()));
                }
                continue;
            }
            Outcome::Skipped(reason) => format!("skipped, {reason}"),
            Outcome::Failed(reason) => reason.clone(),
            Outcome::Cancelled => "export cancelled".into(),
            Outcome::NotStarted => "not started, export cancelled".into(),
        };
        summary.problems.push((name.clone(), problem));
    }
    for (name, why) in &queued.left_out {
        summary
            .problems
            .push((name.clone(), format!("can't be exported: {why}")));
    }
    summary.exported = exported.len();
    summary.total = queued.names.len() + queued.left_out.len();
    let single = (summary.total == 1 && summary.exported == 1 && summary.noted == 0)
        .then(|| exported.remove(0));
    (summary, single)
}

/// The watermark named in the Export dialog: a saved preset, or the Simple
/// Copyright Watermark, whose text comes from each photo at export.
fn watermark_for(name: &str) -> Option<crate::watermark::Watermark> {
    use crate::watermark::{SIMPLE_COPYRIGHT, Watermark, presets};
    if name == SIMPLE_COPYRIGHT {
        return Some(Watermark {
            name: SIMPLE_COPYRIGHT.into(),
            ..Default::default()
        });
    }
    presets().into_iter().find(|w| w.name == name)
}

#[cfg(test)]
mod tests;
