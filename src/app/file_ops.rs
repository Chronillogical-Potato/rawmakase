//! File operations in the app (#370, planned in #556): the request, keeping
//! the catalog quiet while the operation is planned, confirmed and run, the
//! confirmation, and applying the outcome to the Library, the open photo,
//! the undo log and the caches. `catalog::file_ops` owns everything else
//! (what an operation takes, its refusals, the journal, the disk); nothing
//! here reads files or writes SQL.
//!
//! An operation (Remove from Catalog, moving photos to a folder, making a
//! folder and moving photos into it) goes, in one uninterrupted step on the
//! UI thread: nothing else running (`Activity` idle), the Library's drafts
//! and the open edit saved (a failed save stops here, holding nothing), then
//! `Activity::ChangingFiles`, which autosave, undo and navigation wait for,
//! held until the outcome is applied. Background reads stop, and Read
//! Metadata from Files is cancelled, so no read lands on a photo whose id a
//! removal frees or whose file a move takes. The plan is made on a worker,
//! confirmed in a dialog where Cancel has the focus, run on a worker with
//! its own connection, and what it did is read back from the journal: the
//! same after success, an error, a panic or a lost thread. The open photo
//! follows its file when it moved.
//!
//! The dialogs that come before a plan (Move to Folder…, New Folder
//! Inside…) and the one for what a crash left unfinished, recovered when a
//! catalog opens, are in `file_ops_dialogs.rs`.
use super::{Editor, widgets};
#[cfg(test)]
use crate::catalog::file_ops::Summary;
use crate::catalog::file_ops::{
    self, BatchId, FileOp, ItemResult, Outcome, Plan, Trash, TrashError, TrashReceipt, Unfinished,
};
use crate::catalog::{CatalogLocation, PhotoId};
use eframe::egui;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[path = "file_ops_dialogs.rs"]
mod dialogs;
use dialogs::{Choosing, Naming};

/// The Trash file operations get before the app has the system's: no
/// operation it offers asks it for anything yet.
pub(super) struct Unavailable;

impl Trash for Unavailable {
    fn trash(&self, _: &Path) -> Result<TrashReceipt, TrashError> {
        Err(TrashError::Unsupported)
    }
}

/// What an operation does, for what is said about it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Verb {
    Remove,
    /// Into the folder named so.
    Move {
        to: String,
    },
    /// Makes folder `name` inside `parent`; `moving` when photos go into it.
    Create {
        name: String,
        parent: String,
        moving: bool,
    },
}

impl Verb {
    /// "nothing was removed", for a status line.
    fn nothing(&self) -> &'static str {
        match self {
            Self::Remove => "nothing was removed",
            Self::Move { .. } => "nothing was moved",
            Self::Create { .. } => "no folder was made",
        }
    }
    /// "Nothing to remove", for a plan with nothing to do.
    fn nothing_to(&self) -> &'static str {
        match self {
            Self::Remove => "Nothing to remove",
            Self::Move { .. } | Self::Create { moving: true, .. } => "Nothing to move",
            Self::Create { .. } => "No folder to make",
        }
    }
    /// "Photos not removed", for a plan that failed.
    fn failed(&self) -> &'static str {
        match self {
            Self::Remove => "Photos not removed",
            Self::Move { .. } => "Photos not moved",
            Self::Create { .. } => "Folder not made",
        }
    }
}

/// Where the file operation is.
#[derive(Default)]
pub(super) struct FileOps {
    state: State,
    /// A catalog just opened, whose unfinished file operations are to be
    /// recovered once nothing else runs.
    pending_recovery: Option<CatalogLocation>,
    /// The open photo, moved before it was fully opened: opened again from
    /// its new file once the catalog is let go.
    reopen: Option<(PhotoId, std::path::PathBuf)>,
    /// Tests stop a run before its worker looks, to see it obeys Stop.
    #[cfg(test)]
    stop_first: bool,
}

#[derive(Default)]
enum State {
    #[default]
    Idle,
    /// Move to Folder…: the folder being chosen. Holds nothing.
    Choosing(Box<Choosing>),
    /// New Folder Inside…: the name being given. Holds nothing.
    Naming(Box<Naming>),
    /// Planned on a worker, for this catalog.
    Planning {
        catalog: CatalogLocation,
        verb: Verb,
    },
    /// Waiting for the user. `generation` is the session's when it was
    /// planned: a catalog read again since may have given its photos' ids
    /// to others, and the plan is not run.
    Confirming {
        plan: Box<Plan>,
        catalog: CatalogLocation,
        generation: u64,
        verb: Verb,
    },
    /// Run on a worker, for `photos`, which may all be gone or moved when
    /// what it did can't be read back; `cancel` stops it between photos.
    Running {
        batch: BatchId,
        catalog: CatalogLocation,
        photos: Vec<PhotoId>,
        cancel: Arc<AtomicBool>,
        verb: Verb,
        progress: Progress,
    },
    /// What a crash left unfinished, decided on a worker as the catalog
    /// opens.
    Recovering { catalog: CatalogLocation },
    /// What recovery couldn't decide, for the user. Holds nothing.
    Unfinished {
        catalog: CatalogLocation,
        items: Vec<Unfinished>,
    },
    /// The user's choice for `operation`, carried out on a worker; `items`
    /// are the rest, shown again after.
    Resolving {
        catalog: CatalogLocation,
        operation: file_ops::OperationId,
        items: Vec<Unfinished>,
    },
}

/// How far a run is, read from its journal now and then.
pub(super) struct Progress {
    /// Its items, the folder it makes included.
    steps: usize,
    done: usize,
    read: Option<Instant>,
}

impl FileOps {
    /// Asks a running operation to stop after the photo under way (the
    /// top bar's Stop, closing the window). What it did by then stands.
    pub(super) fn stop(&self) {
        if let State::Running { cancel, .. } = &self.state {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    /// Whether a dialog of a file operation that holds nothing is open:
    /// keys go to it, not to the photos behind.
    pub(super) fn dialog_open(&self) -> bool {
        matches!(
            self.state,
            State::Choosing(_) | State::Naming(_) | State::Unfinished { .. }
        )
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
    /// The open confirmation's question, what it says and its action.
    #[cfg(test)]
    pub(super) fn question(&self) -> Option<(String, Vec<String>, &'static str)> {
        match &self.state {
            State::Confirming { plan, verb, .. } => Some(question(plan, verb)),
            _ => None,
        }
    }
    /// The confirmed plan, as the dialog's action hands it to the worker,
    /// without starting one.
    #[cfg(test)]
    pub(super) fn take_confirmed_for_tests(&mut self) -> (BatchId, file_ops::Confirmed) {
        let State::Confirming {
            plan,
            catalog,
            verb,
            ..
        } = std::mem::take(&mut self.state)
        else {
            panic!("nothing to confirm");
        };
        let batch = plan.batch().clone();
        self.state = State::Running {
            batch: batch.clone(),
            catalog,
            photos: plan.photos(),
            cancel: Default::default(),
            verb,
            progress: Progress {
                steps: plan.steps(),
                done: 0,
                read: None,
            },
        };
        (batch, plan.confirm())
    }
}

/// The names of `s`'s photos, the first 8 and how many more.
fn names_line(names: &[String]) -> String {
    let shown: Vec<&str> = names.iter().take(8).map(String::as_str).collect();
    let more = names.len().saturating_sub(shown.len());
    if more > 0 {
        format!("{} and {more} more", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

/// The confirmation's question, what it says below it and its action.
fn question(plan: &Plan, verb: &Verb) -> (String, Vec<String>, &'static str) {
    let s = plan.summary();
    let mut lines = Vec::new();
    if s.names.len() > 1 {
        lines.push(names_line(&s.names));
    }
    let left = |lines: &mut Vec<String>, stay: &str| {
        if !plan.skipped().is_empty() {
            lines.push(format!(
                "{} {stay}: {}",
                widgets::plural(plan.skipped().len(), "photo is", "photos are"),
                plan.skipped()
                    .first()
                    .map(|s| s.reason.to_string())
                    .unwrap_or_default()
            ));
        }
    };
    let (title, action) = match verb {
        Verb::Remove => {
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
            left(&mut lines, "left as they are");
            let title = match s.names.as_slice() {
                [one] if s.photos == 1 => format!("Remove “{one}” from the catalog?"),
                _ => format!(
                    "Remove {} from the catalog?",
                    widgets::plural(s.photos, "photo", "photos")
                ),
            };
            (title, "Remove from Catalog")
        }
        Verb::Move { .. } | Verb::Create { .. } => {
            lines.push(if s.companions > 0 {
                format!(
                    "{} (and {}) will be moved on disk.",
                    widgets::plural(s.files, "file", "files"),
                    widgets::plural(s.companions, "sidecar file", "sidecar files")
                )
            } else {
                format!(
                    "{} will be moved on disk.",
                    widgets::plural(s.files, "file", "files")
                )
            });
            if s.copies > 0 {
                lines.push(format!(
                    "{} with {}.",
                    widgets::plural(s.copies, "virtual copy goes", "virtual copies go"),
                    if s.copies == 1 {
                        "its photo"
                    } else {
                        "their photos"
                    }
                ));
            }
            if !s.shared_companions.is_empty() {
                lines.push(format!(
                    "{} where {}: other files read {} too.",
                    widgets::plural(
                        s.shared_companions.len(),
                        "sidecar file stays",
                        "sidecar files stay"
                    ),
                    if s.shared_companions.len() == 1 {
                        "it is"
                    } else {
                        "they are"
                    },
                    if s.shared_companions.len() == 1 {
                        "it"
                    } else {
                        "them"
                    }
                ));
            }
            if s.other_computers {
                lines.push(
                    "Other computers using this catalog find these photos missing until the files move there too."
                        .into(),
                );
            }
            left(&mut lines, "staying where they are");
            let photos = match s.names.as_slice() {
                [one] => format!("“{one}”"),
                _ => widgets::plural(s.files, "photo", "photos"),
            };
            match verb {
                Verb::Create { name, parent, .. } => (
                    format!("Make folder “{name}” in “{parent}” and move {photos} into it?"),
                    "Make Folder and Move",
                ),
                Verb::Move { to } => (format!("Move {photos} to “{to}”?"), "Move"),
                Verb::Remove => unreachable!(),
            }
        }
    };
    (title, lines, action)
}

/// The status line after a run: "Removed 3 photos from the catalog", with
/// what was left and why.
fn summary_of(outcome: &Outcome, verb: &Verb) -> String {
    let count = |f: fn(&ItemResult) -> bool| -> usize {
        outcome.items.iter().filter(|i| f(&i.result)).count()
    };
    let removed: usize = outcome
        .items
        .iter()
        .filter(|i| i.result == ItemResult::Removed)
        .map(|i| i.photos.len())
        .sum();
    let moved = count(|r| matches!(r, ItemResult::Moved { .. }));
    let created = count(|r| matches!(r, ItemResult::Created { .. })) > 0;
    let mut text = match verb {
        Verb::Remove => format!(
            "Removed {} from the catalog",
            widgets::plural(removed, "photo", "photos")
        ),
        Verb::Move { to } => format!(
            "Moved {} to {to}",
            widgets::plural(moved, "photo", "photos")
        ),
        Verb::Create { name, .. } if !created => format!("Folder {name} not made"),
        Verb::Create {
            name, moving: true, ..
        } => format!(
            "Made folder {name} and moved {} into it",
            widgets::plural(moved, "photo", "photos")
        ),
        Verb::Create { name, .. } => format!("Made folder {name}"),
    };
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
        let what = match verb {
            Verb::Create { .. } if !created => String::new(),
            _ => format!(" · {} left", widgets::plural(left.len(), "photo", "photos")),
        };
        text.push_str(&format!("{what}: {}", left[0]));
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
        if !photos.is_empty() {
            self.request_file_op(FileOp::RemoveFromCatalog { photos }, Verb::Remove);
        }
    }

    /// Moves `photos` to catalog folder `to`, named `name` (dropped on it,
    /// or chosen in Move to Folder…), once confirmed.
    pub(super) fn request_photo_move(
        &mut self,
        photos: Vec<PhotoId>,
        to: crate::catalog::FolderId,
        name: String,
    ) {
        if !photos.is_empty() {
            self.request_file_op(FileOp::MovePhotos { photos, to }, Verb::Move { to: name });
        }
    }

    /// Plans `op`: saves what is open, keeps the catalog quiet and plans it
    /// on a worker, then asks. Nothing is held when the save fails.
    fn request_file_op(&mut self, op: FileOp, verb: Verb) {
        // A dialog that led here holds nothing.
        if matches!(self.file_ops.state, State::Choosing(_) | State::Naming(_)) {
            self.file_ops.state = State::Idle;
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
        // Saved first, with nothing processed in between: a failed save
        // stops here.
        if !self.flush() {
            self.status = format!(
                "The open photo's edit could not be saved; {}",
                verb.nothing()
            );
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
            verb,
        };
        let ctx = self.context.clone();
        super::task::spawn(
            self.tx.clone(),
            ctx,
            move |tx| {
                let planned = crate::catalog::Catalog::open(&location)
                    .map_err(|e| format!("{e:#}"))
                    .and_then(|catalog| file_ops::plan(&catalog, op).map_err(|e| e.to_string()));
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

    /// The plan came back: it is asked about, or nothing is to be done. A
    /// folder made with nothing moving into it was asked for already, by
    /// New Folder Inside…'s Create.
    pub(super) fn file_op_planned(&mut self, planned: Result<Plan, String>) {
        let State::Planning { catalog, verb } = std::mem::take(&mut self.file_ops.state) else {
            return;
        };
        if !self.showing_catalog(&catalog) {
            self.release_files();
            return;
        }
        match planned {
            Ok(plan) if plan.is_empty() => {
                self.status = match plan.skipped().first() {
                    Some(s) => format!("{}: {}", verb.nothing_to(), s.reason),
                    None => verb.nothing_to().into(),
                };
                self.release_files();
            }
            Ok(plan) => {
                let generation = self.library.as_ref().map_or(0, |l| l.session.generation());
                let asked = matches!(verb, Verb::Create { moving: false, .. });
                self.file_ops.state = State::Confirming {
                    plan: Box::new(plan),
                    catalog,
                    generation,
                    verb,
                };
                if asked {
                    self.confirm_file_op();
                }
            }
            Err(e) => {
                self.status = format!("{}: {e}", verb.failed());
                self.release_files();
            }
        }
    }

    /// The file operations' dialogs, while one is open.
    pub(super) fn file_op_window(&mut self, ctx: &egui::Context) {
        match &self.file_ops.state {
            State::Choosing(_) => return self.choose_folder_window(ctx),
            State::Naming(_) => return self.new_folder_window(ctx),
            State::Unfinished { .. } => return self.unfinished_window(ctx),
            _ => {}
        }
        let State::Confirming { plan, verb, .. } = &self.file_ops.state else {
            return;
        };
        let (title, lines, action) = question(plan, verb);
        let id = match verb {
            Verb::Remove => "remove-photos",
            Verb::Move { .. } | Verb::Create { .. } => "move-photos",
        };
        match widgets::destructive_confirm(ctx, id, &title, &lines, "Cancel", action) {
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
            verb,
        } = std::mem::take(&mut self.file_ops.state)
        else {
            return;
        };
        // Still the catalog it was planned for, not read again since.
        let current = self.library.as_ref().map(|l| l.session.generation());
        if !self.showing_catalog(&catalog) || current != Some(generation) {
            self.status = format!("The catalog changed; {}. Try again.", verb.nothing());
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
            verb,
            progress: Progress {
                steps: plan.steps(),
                done: 0,
                read: None,
            },
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
            verb,
            progress,
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
                verb,
                progress,
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
            // the photos may be gone or moved, so all are forgotten as if
            // they were, and the catalog's own lists say where they are.
            (_, Err(e)) => {
                self.status =
                    format!("The file operation stopped ({e}); the catalog was read again");
                self.forget_removed_photos(&photos);
                self.reread_after_files(&photos, &[]);
                self.release_files();
                return;
            }
        };
        self.status = summary_of(&outcome, &verb);
        // After the summary: a catalog that can't be read again says so.
        self.apply_file_outcome(&outcome);
        self.release_files();
    }

    /// Brings the app in step with what file operations did: removed
    /// photos forgotten, moved ones followed (the open photo, preview
    /// requests), the catalog read again for those and for folders made.
    fn apply_file_outcome(&mut self, outcome: &Outcome) {
        let ids = |f: fn(&ItemResult) -> bool| -> Vec<PhotoId> {
            outcome
                .items
                .iter()
                .filter(|i| f(&i.result))
                .flat_map(|i| i.photos.iter().copied())
                .collect()
        };
        let removed = ids(|r| matches!(r, ItemResult::Removed | ItemResult::Trashed));
        let moved = ids(|r| matches!(r, ItemResult::Moved { .. }));
        let created = outcome
            .items
            .iter()
            .any(|i| matches!(i.result, ItemResult::Created { .. }));
        self.forget_removed_photos(&removed);
        if created && let Some(library) = &mut self.library {
            library.unfold_new_folder();
        }
        if !removed.is_empty() || !moved.is_empty() || created {
            self.reread_after_files(&removed, &moved);
        }
    }

    /// Forgets photos removed from the catalog, whose ids new photos may
    /// be given: their previews, undo steps and selection, the reference,
    /// and the open photo, which nothing may save into now.
    fn forget_removed_photos(&mut self, removed: &[PhotoId]) {
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
    }

    /// Reads the catalog again after photos were `removed` or `moved`
    /// (their cached thumbnails are dropped), moves preview requests with
    /// moved photos, and has the open photo follow its file.
    fn reread_after_files(&mut self, removed: &[PhotoId], moved: &[PhotoId]) {
        let Some(library) = &mut self.library else {
            return;
        };
        let changed: Vec<PhotoId> = removed.iter().chain(moved).copied().collect();
        if let Err(e) = library.reread_after(&changed) {
            self.status = format!("Done, but the catalog could not be read again: {e:#}");
            return;
        }
        // Each moved photo, its copies included, at its file now.
        let files: Vec<(PhotoId, std::path::PathBuf)> = library
            .session
            .photos
            .iter()
            .filter(|p| moved.contains(&p.id) || p.master.is_some_and(|m| moved.contains(&m)))
            .map(|p| (p.id, p.path.clone()))
            .collect();
        let catalog = library.session.catalog.location().clone();
        // The open photo's file is where the catalog says now: its edit
        // saves there.
        let open = self
            .document
            .catalog_photo
            .and_then(|id| library.photo(id))
            .map(|p| p.path.clone())
            .filter(|path| self.document.path.as_ref() != Some(path));
        self.move_preview_intents(&catalog, &files);
        if let Some(path) = open {
            // A load that may still read the old path (under way, or the
            // full-size decode after the half-size one shown) or failed
            // there, leaving no full-size image, is dropped, and the photo
            // opened again where it is now. Its edit was saved before the
            // move, so nothing is lost.
            let unfinished =
                self.load.is_running() || self.document.full().is_none_or(|image| image.fast);
            if unfinished && let Some(id) = self.document.catalog_photo {
                self.load.invalidate();
                self.file_ops.reopen = Some((id, path.clone()));
            }
            self.document.relink(path);
        }
    }

    /// The top bar's part while an operation runs: what it does, and Stop.
    pub(super) fn file_op_progress(&mut self, ui: &mut egui::Ui) {
        let text = match &mut self.file_ops.state {
            State::Recovering { .. } | State::Resolving { .. } => {
                "Checking unfinished file operations…".to_string()
            }
            State::Running {
                batch,
                photos,
                cancel,
                verb,
                progress,
                ..
            } => {
                if cancel.load(Ordering::Relaxed) {
                    "Stopping after the photo under way…".to_string()
                } else {
                    // How far it is, read from the journal now and then.
                    let due = progress
                        .read
                        .is_none_or(|at| at.elapsed() >= Duration::from_millis(250));
                    if due && let Some(library) = &self.library {
                        progress.read = Some(Instant::now());
                        if let Ok(outcome) = file_ops::outcome(&library.session.catalog, batch) {
                            progress.done = outcome
                                .items
                                .iter()
                                .filter(|i| i.result != ItemResult::Interrupted)
                                .count();
                        }
                    }
                    match verb {
                        Verb::Remove => format!(
                            "Removing {} from the catalog",
                            widgets::plural(photos.len(), "photo", "photos")
                        ),
                        Verb::Move { to } => {
                            let at = (progress.done + 1).min(progress.steps);
                            format!("Moving {at} of {} to {to}…", progress.steps)
                        }
                        Verb::Create { name, .. } if progress.done == 0 => {
                            format!("Making folder {name}…")
                        }
                        Verb::Create { name, .. } => format!(
                            "Moving {} of {} into {name}…",
                            progress.done,
                            progress.steps.saturating_sub(1)
                        ),
                    }
                }
            }
            _ => return,
        };
        let palette = super::theme::palette(ui.ctx());
        let stoppable = matches!(
            &self.file_ops.state,
            State::Running { cancel, .. } if !cancel.load(Ordering::Relaxed)
        );
        ui.label(egui::RichText::new(text).size(12.).color(palette.gray(210)));
        if stoppable
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
        if let Some((id, path)) = self.file_ops.reopen.take()
            && self.document.catalog_photo == Some(id)
        {
            // What the operation did stays on the status line.
            let status = std::mem::take(&mut self.status);
            self.load_raw(path, Some(id));
            self.status = status;
        }
    }
}

#[cfg(test)]
#[path = "file_ops_move_tests.rs"]
mod move_tests;
#[cfg(test)]
#[path = "file_ops_tests.rs"]
mod tests;
