//! File operations in the app (#370, planned in #556): the request, keeping
//! the catalog quiet while the operation is planned, confirmed and run, the
//! confirmation, and applying the outcome to the Library, the open photo,
//! the undo log and the caches. `catalog::file_ops` owns everything else
//! (what an operation takes, its refusals, the journal, the disk); nothing
//! here reads files or writes SQL.
//!
//! Remove from Catalog goes, in one uninterrupted step on the UI thread:
//! nothing else running (`Activity` idle), the Library's drafts and the open
//! edit saved (a failed save stops here, holding nothing), then
//! `Activity::ChangingFiles`, which autosave, undo and navigation wait for,
//! held until the outcome is applied. Background reads stop, and Read
//! Metadata from Files is cancelled, so no read lands on a photo whose id a
//! removal frees. The plan is made on a worker, confirmed in a dialog where
//! Cancel has the focus, run on a worker with its own connection, and what
//! it did is read back from the journal: the same after success, an error,
//! a panic or a lost thread.
use super::{Editor, widgets};
#[cfg(test)]
use crate::catalog::file_ops::Summary;
use crate::catalog::file_ops::{
    self, BatchId, FileOp, ItemResult, Outcome, Plan, Trash, TrashError, TrashReceipt,
};
use crate::catalog::{CatalogLocation, PhotoId};
use eframe::egui;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The Trash file operations get before the app has the system's: Remove
/// from Catalog never asks it for anything.
pub(super) struct Unavailable;

impl Trash for Unavailable {
    fn trash(&self, _: &Path) -> Result<TrashReceipt, TrashError> {
        Err(TrashError::Unsupported)
    }
}

/// Where the file operation is.
#[derive(Default)]
pub(super) struct FileOps {
    state: State,
    /// Tests stop a run before its worker looks, to see it obeys Stop.
    #[cfg(test)]
    stop_first: bool,
}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    /// Planned on a worker, for this catalog.
    Planning { catalog: CatalogLocation },
    /// Waiting for the user. `generation` is the session's when it was
    /// planned: a catalog read again since may have given its photos' ids
    /// to others, and the plan is not run.
    Confirming {
        plan: Box<Plan>,
        catalog: CatalogLocation,
        generation: u64,
    },
    /// Run on a worker, for `photos`, which may all be gone when what it
    /// did can't be read back; `cancel` stops it between photos.
    Running {
        batch: BatchId,
        catalog: CatalogLocation,
        photos: Vec<PhotoId>,
        cancel: Arc<AtomicBool>,
    },
}

impl FileOps {
    /// Asks a running operation to stop after the photo under way (the
    /// top bar's Stop, closing the window). What it did by then stands.
    pub(super) fn stop(&self) {
        if let State::Running { cancel, .. } = &self.state {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    /// Whether a running operation was asked to stop.
    #[cfg(test)]
    pub(super) fn stopping(&self) -> bool {
        matches!(&self.state, State::Running { cancel, .. } if cancel.load(Ordering::Relaxed))
    }
    /// Whether the confirmation is open.
    #[cfg(test)]
    pub(super) fn confirming(&self) -> bool {
        matches!(self.state, State::Confirming { .. })
    }
    /// What the open confirmation counts.
    #[cfg(test)]
    pub(super) fn summary(&self) -> Option<&Summary> {
        match &self.state {
            State::Confirming { plan, .. } => Some(plan.summary()),
            _ => None,
        }
    }
    /// The confirmed plan, as the dialog's action hands it to the worker,
    /// without starting one.
    #[cfg(test)]
    pub(super) fn take_confirmed_for_tests(&mut self) -> (BatchId, file_ops::Confirmed) {
        let State::Confirming { plan, catalog, .. } = std::mem::take(&mut self.state) else {
            panic!("nothing to confirm");
        };
        let batch = plan.batch().clone();
        self.state = State::Running {
            batch: batch.clone(),
            catalog,
            photos: plan.photos(),
            cancel: Default::default(),
        };
        (batch, plan.confirm())
    }
}

/// The confirmation's question and what it says below it.
fn question(plan: &Plan) -> (String, Vec<String>) {
    let s = plan.summary();
    let title = match s.names.as_slice() {
        [one] if s.photos == 1 => format!("Remove “{one}” from the catalog?"),
        _ => format!(
            "Remove {} from the catalog?",
            widgets::plural(s.photos, "photo", "photos")
        ),
    };
    let mut lines = Vec::new();
    if s.names.len() > 1 {
        let shown: Vec<&str> = s.names.iter().take(8).map(String::as_str).collect();
        let more = s.names.len().saturating_sub(shown.len());
        lines.push(if more > 0 {
            format!("{} and {more} more", shown.join(", "))
        } else {
            shown.join(", ")
        });
    }
    lines.push("The files stay on disk.".into());
    if s.copies > 0 {
        lines.push(format!(
            "{} removed with {}.",
            widgets::plural(s.copies, "virtual copy is", "virtual copies are"),
            if s.copies == 1 {
                "its photo"
            } else {
                "their photos"
            }
        ));
    }
    if s.edited > 0 {
        lines.push(format!(
            "{} edits in RAWmakase; they are removed too. Adding the file again doesn't bring them back.",
            widgets::plural(s.edited, "photo has", "photos have")
        ));
    }
    if !plan.skipped().is_empty() {
        lines.push(format!(
            "{} left as they are: {}",
            widgets::plural(plan.skipped().len(), "photo is", "photos are"),
            plan.skipped()
                .first()
                .map(|s| s.reason.to_string())
                .unwrap_or_default()
        ));
    }
    (title, lines)
}

/// "Removed 3 photos from the catalog", with what was left and why.
fn summary_of(outcome: &Outcome) -> String {
    let removed: usize = outcome
        .items
        .iter()
        .filter(|i| i.result == ItemResult::Removed)
        .map(|i| i.photos.len())
        .sum();
    let mut text = format!(
        "Removed {} from the catalog",
        widgets::plural(removed, "photo", "photos")
    );
    let left: Vec<String> = outcome
        .items
        .iter()
        .filter_map(|i| match &i.result {
            ItemResult::Removed
            | ItemResult::Moved { .. }
            | ItemResult::Trashed
            | ItemResult::Created { .. } => None,
            ItemResult::Unchanged(why) => Some(why.clone().unwrap_or_else(|| "unchanged".into())),
            ItemResult::NeedsRecovery(why) => Some(why.clone()),
            ItemResult::Kept => Some("kept as it is".into()),
            ItemResult::Interrupted => Some("interrupted".into()),
        })
        .collect();
    if !left.is_empty() {
        text.push_str(&format!(
            " · {} left: {}",
            widgets::plural(left.len(), "photo", "photos"),
            left[0]
        ));
    }
    if let Some(stopped) = &outcome.stopped {
        text.push_str(&format!(" · stopped: {stopped}"));
    }
    text
}

impl Editor {
    /// Remove from Catalog for `photos` (Delete, ⌥Delete, the thumbnail
    /// menu): saves what is open, keeps the catalog quiet and plans it, then
    /// asks. Nothing is held when the save fails.
    pub(super) fn request_photo_removal(&mut self, photos: Vec<PhotoId>) {
        if photos.is_empty() {
            return;
        }
        if self.activity.is_busy() {
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
        // Saved first, with nothing processed in between: a failed save
        // stops here.
        if !self.flush() {
            self.status = "The open photo's edit could not be saved; nothing was removed".into();
            return;
        }
        if !self.activity.begin_file_change() {
            return;
        }
        if let Some(library) = &mut self.library {
            library.suspend_background_reads();
        }
        self.file_ops.state = State::Planning {
            catalog: location.clone(),
        };
        let ctx = self.context.clone();
        super::task::spawn(
            self.tx.clone(),
            ctx,
            move |tx| {
                let planned = crate::catalog::Catalog::open(&location)
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|catalog| {
                        file_ops::plan(&catalog, FileOp::RemoveFromCatalog { photos })
                            .map_err(|e| e.to_string())
                    });
                let _ = tx.send(super::worker::Event::FileOpPlanned(Box::new(planned)));
            },
            |tx, message| {
                let _ = tx.send(super::worker::Event::FileOpPlanned(Box::new(Err(message))));
            },
        );
    }

    /// Delete and Backspace in Develop, as in the Library: for the
    /// filmstrip's selection when the open photo is in it, else the open
    /// photo. Not while a tool is open, as tools take Delete for their own
    /// spots, masks and guides, nor behind a menu or a dialog.
    pub(super) fn develop_delete_key(&mut self, ctx: &egui::Context) {
        if !self.view.is(super::state::Tool::None)
            || egui::Popup::is_any_open(ctx)
            || self.command_modal()
        {
            return;
        }
        // Whether it was pressed, and with Option (Remove from Catalog, even
        // in a collection).
        let pressed = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key: egui::Key::Delete | egui::Key::Backspace,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if !modifiers.shift && !modifiers.ctrl && !modifiers.command => {
                    Some(modifiers.alt)
                }
                _ => None,
            })
        });
        let (Some(alt), Some(open), Some(library)) =
            (pressed, self.document.catalog_photo, self.library.as_mut())
        else {
            return;
        };
        let selected = library.selected_photos();
        let ids = if selected.contains(&open) {
            selected
        } else {
            vec![open]
        };
        if alt {
            library.request_removal(ids);
        } else {
            library.delete_key(ids);
        }
    }

    /// The plan came back: it is asked about, or nothing is to be done.
    pub(super) fn file_op_planned(&mut self, planned: Result<Plan, String>) {
        let State::Planning { catalog } = std::mem::take(&mut self.file_ops.state) else {
            return;
        };
        if !self.showing_catalog(&catalog) {
            self.release_files();
            return;
        }
        match planned {
            Ok(plan) if plan.is_empty() => {
                self.status = match plan.skipped().first() {
                    Some(s) => format!("Nothing to remove: {}", s.reason),
                    None => "Nothing to remove".into(),
                };
                self.release_files();
            }
            Ok(plan) => {
                let generation = self.library.as_ref().map_or(0, |l| l.session.generation());
                self.file_ops.state = State::Confirming {
                    plan: Box::new(plan),
                    catalog,
                    generation,
                }
            }
            Err(e) => {
                self.status = format!("Photos not removed: {e}");
                self.release_files();
            }
        }
    }

    /// The confirmation, while it is open.
    pub(super) fn file_op_window(&mut self, ctx: &egui::Context) {
        let State::Confirming { plan, .. } = &self.file_ops.state else {
            return;
        };
        let (title, lines) = question(plan);
        match widgets::destructive_confirm(
            ctx,
            "remove-photos",
            &title,
            &lines,
            "Cancel",
            "Remove from Catalog",
        ) {
            None => {}
            Some(true) => self.confirm_file_op(),
            Some(false) => {
                self.file_ops.state = State::Idle;
                self.release_files();
            }
        }
    }

    /// The confirmation's action: runs the plan on a worker.
    pub(super) fn confirm_file_op(&mut self) {
        let State::Confirming {
            plan,
            catalog,
            generation,
        } = std::mem::take(&mut self.file_ops.state)
        else {
            return;
        };
        // Still the catalog it was planned for, not read again since.
        let current = self.library.as_ref().map(|l| l.session.generation());
        if !self.showing_catalog(&catalog) || current != Some(generation) {
            self.status = "The catalog changed; nothing was removed. Try again.".into();
            self.release_files();
            return;
        }
        let batch = plan.batch().clone();
        #[cfg(test)]
        let cancel = Arc::new(AtomicBool::new(self.file_ops.stop_first));
        #[cfg(not(test))]
        let cancel: Arc<AtomicBool> = Default::default();
        self.file_ops.state = State::Running {
            batch: batch.clone(),
            catalog: catalog.clone(),
            photos: plan.photos(),
            cancel: cancel.clone(),
        };
        let confirmed = plan.confirm();
        let failed_batch = batch.clone();
        super::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let done = crate::catalog::Catalog::open(&catalog)
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|mut catalog| {
                        file_ops::execute(&mut catalog, confirmed, &Unavailable, &cancel)
                            .map_err(|e| e.to_string())
                    });
                let _ = tx.send(super::worker::Event::FileOpDone(batch, Box::new(done)));
            },
            move |tx, message| {
                let _ = tx.send(super::worker::Event::FileOpDone(
                    failed_batch,
                    Box::new(Err(message)),
                ));
            },
        );
    }

    /// The run finished, failed or its thread stopped: what it did is read
    /// from the journal and applied, and the catalog is let go.
    pub(super) fn file_op_done(&mut self, batch: BatchId, done: Result<Outcome, String>) {
        let State::Running {
            batch: running,
            catalog,
            photos,
            cancel,
        } = std::mem::take(&mut self.file_ops.state)
        else {
            return;
        };
        if running != batch {
            self.file_ops.state = State::Running {
                batch: running,
                catalog,
                photos,
                cancel,
            };
            return;
        }
        if !self.showing_catalog(&catalog) {
            self.release_files();
            return;
        }
        let read = self
            .library
            .as_ref()
            .map(|l| file_ops::outcome(&l.session.catalog, &batch));
        let outcome = match (read, done) {
            (Some(Ok(mut outcome)), done) => {
                outcome.stopped = match done {
                    Ok(done) => done.stopped,
                    Err(e) => Some(e),
                };
                outcome
            }
            (_, Ok(outcome)) => outcome,
            // Neither the journal nor the worker says what was done: any of
            // the photos may be gone, so all are forgotten as if they were,
            // and the catalog's own lists show which are left.
            (_, Err(e)) => {
                self.status =
                    format!("Remove from Catalog stopped ({e}); the catalog was read again");
                self.forget_removed_photos(&photos);
                self.release_files();
                return;
            }
        };
        let removed: Vec<PhotoId> = outcome
            .items
            .iter()
            .filter(|i| i.result == ItemResult::Removed)
            .flat_map(|i| i.photos.iter().copied())
            .collect();
        self.status = summary_of(&outcome);
        // After the summary: a catalog that can't be read again says so.
        self.forget_removed_photos(&removed);
        self.release_files();
    }

    /// Forgets photos removed from the catalog, whose ids new photos may
    /// be given: their previews, undo steps and selection, the reference,
    /// and the open photo, which nothing may save into now.
    fn forget_removed_photos(&mut self, removed: &[PhotoId]) {
        if removed.is_empty() {
            return;
        }
        for &id in removed {
            self.forget_preview_intent(id);
            self.undo_log.forget_photo(id);
        }
        if self.reference.photo.is_some_and(|id| removed.contains(&id)) {
            self.clear_reference();
        }
        if self
            .document
            .catalog_photo
            .is_some_and(|id| removed.contains(&id))
        {
            self.close_removed_document();
        }
        if let Some(library) = &mut self.library
            && let Err(e) = library.reread_after(removed)
        {
            self.status = format!("Removed, but the catalog could not be read again: {e:#}");
        }
    }

    /// The top bar's part while an operation runs: what it does, and Stop.
    pub(super) fn file_op_progress(&mut self, ui: &mut egui::Ui) {
        let State::Running { photos, cancel, .. } = &self.file_ops.state else {
            return;
        };
        let palette = super::theme::palette(ui.ctx());
        let stopping = cancel.load(Ordering::Relaxed);
        let text = if stopping {
            "Stopping after the photo under way…".to_string()
        } else {
            format!(
                "Removing {} from the catalog",
                widgets::plural(photos.len(), "photo", "photos")
            )
        };
        ui.label(egui::RichText::new(text).size(12.).color(palette.gray(210)));
        if !stopping
            && ui
                .small_button("Stop")
                .on_hover_text("Stop after the photo under way")
                .clicked()
        {
            self.file_ops.stop();
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }

    /// Whether the Library still shows catalog `location`.
    fn showing_catalog(&self, location: &CatalogLocation) -> bool {
        self.library
            .as_ref()
            .is_some_and(|l| l.session.catalog.location() == location)
    }

    /// Lets the catalog go: what waited may write again, and the
    /// background reads go on.
    fn release_files(&mut self) {
        self.activity.finish_file_change();
        if let Some(library) = &mut self.library {
            library.resume_background_reads();
        }
    }
}

#[cfg(test)]
#[path = "file_ops_tests.rs"]
mod tests;
