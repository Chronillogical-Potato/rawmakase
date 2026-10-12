//! Synchronize Folder (issue #432) in the app: the scan, on a thread of its
//! own while the Library and Develop stay usable, shown in the top bar with
//! a stop button; the review dialog; and applying what the user accepted as
//! a folder change. The photo open in Develop keeps its pending edit: a
//! verified move of its file relinks it in place (see `catalog_ready`).
use super::Editor;
use super::theme;
use super::worker::Event;
use crate::catalog::folder_sync::{Accepted, Applied, SyncPlan, SyncScope, scan};
use crate::catalog::{CatalogLocation, PhotoId};
use eframe::egui;
use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The synchronization under way or waiting for review.
#[derive(Default)]
pub(super) struct FolderSync {
    scan: Option<Scan>,
    review: Option<Review>,
    /// The review dialog is showing.
    showing: bool,
    /// Counts scans, so a result of one stopped or superseded is ignored.
    started: u64,
}

impl FolderSync {
    pub(super) fn is_scanning(&self) -> bool {
        self.scan.is_some()
    }
    /// Whether the review dialog or its confirmation is showing: keys and
    /// commands go to it, never to the photos behind it.
    pub(super) fn is_reviewing(&self) -> bool {
        self.showing || self.review.as_ref().is_some_and(|r| r.confirming)
    }
}

/// A scan running on its own thread.
struct Scan {
    id: u64,
    name: String,
    catalog: CatalogLocation,
    /// The session's structural generation it started under.
    generation: u64,
    files: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    /// Set once the thread has sent its result, so a thread that ended
    /// without one (a panic, a lost channel) is noticed.
    sent: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// A finished scan waiting for the user, and what they ticked.
pub(super) struct Review {
    name: String,
    catalog: CatalogLocation,
    generation: u64,
    plan: SyncPlan,
    /// Per move of the plan: accepted. Verified ones start ticked.
    moves: Vec<bool>,
    /// Per choice: the option picked, if any.
    picks: Vec<Option<usize>>,
    /// Per addition: accepted. New files start ticked, copies don't.
    additions: Vec<bool>,
    /// Per missing photo: to remove. None start ticked.
    removals: Vec<bool>,
    /// The sections whose list is shown.
    expanded: BTreeSet<&'static str>,
    /// What each photo holds that a removal loses: rating, edit, copies.
    badges: HashMap<PhotoId, String>,
    /// Asking to confirm removing photos that hold edits or metadata.
    confirming: bool,
}

/// What a scan thread sends back.
pub(crate) struct Scanned {
    id: u64,
    result: Result<SyncPlan, String>,
}
/// What applying sends back: the catalog opened again, and what changed.
pub(crate) struct Synced {
    /// The catalog opened again, or why it could not be.
    library: Result<Box<super::library::Library>, String>,
    applied: Applied,
    /// The photo open in Develop, still in the catalog, and its new file
    /// when it was found moved and verified: the open document stays,
    /// following it.
    relink: Option<(PhotoId, Option<PathBuf>)>,
}

impl Editor {
    /// Starts synchronizing `scope`, named `name`, in the background.
    pub(super) fn start_folder_sync(&mut self, scope: SyncScope, name: String) {
        if self.folder_sync.scan.is_some() {
            self.status = "A synchronization is already running.".into();
            return;
        }
        let Some(library) = &self.library else {
            return;
        };
        let inputs = match library.session.catalog.sync_inputs(scope) {
            Ok(inputs) => inputs,
            Err(e) => {
                self.status = format!("Synchronize Folder failed: {e:#}");
                return;
            }
        };
        self.folder_sync.review = None;
        self.folder_sync.showing = false;
        self.folder_sync.started += 1;
        let id = self.folder_sync.started;
        let files = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let sent = Arc::new(AtomicBool::new(false));
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        let (counted, stopped, sending) = (files.clone(), cancel.clone(), sent.clone());
        let thread = std::thread::Builder::new()
            .name("folder-sync-scan".into())
            .spawn(move || {
                let progress = |n: usize| {
                    counted.store(n, Ordering::Relaxed);
                    ctx.request_repaint();
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    scan(&inputs, &crate::volume_mounted, &stopped, &progress)
                }));
                let result = match result {
                    Ok(Ok(plan)) => Ok(plan),
                    Ok(Err(e)) => Err(format!("{e:#}")),
                    Err(_) => Err("Stopped unexpectedly".into()),
                };
                if tx
                    .send(Event::FolderSyncScanned(Box::new(Scanned { id, result })))
                    .is_ok()
                {
                    sending.store(true, Ordering::Release);
                }
                ctx.request_repaint();
            })
            .ok();
        if thread.is_none() {
            self.status = "Synchronize Folder could not start".into();
            return;
        }
        self.folder_sync.scan = Some(Scan {
            id,
            name,
            catalog: library.session.catalog.location().clone(),
            generation: library.session.generation(),
            files,
            cancel,
            sent,
            thread,
        });
    }
    /// A scan finished: its plan waits for review.
    pub(super) fn folder_sync_scanned(&mut self, scanned: Scanned) {
        let Some(scan) = self.folder_sync.scan.take_if(|s| s.id == scanned.id) else {
            return;
        };
        let current = self.library.as_ref().is_some_and(|l| {
            *l.session.catalog.location() == scan.catalog
                && l.session.generation() == scan.generation
        });
        match scanned.result {
            Err(e) if scan.cancel.load(Ordering::Relaxed) => {
                let _ = e;
                self.status = "Synchronization stopped".into();
            }
            Err(e) => self.status = format!("Synchronize Folder failed: {e}"),
            Ok(_) if !current => {
                self.status = "The catalog changed during the scan; synchronize again".into();
            }
            Ok(plan) => {
                if nothing_to_do(&plan) {
                    self.status = format!("\"{}\" is in sync", scan.name);
                    if !plan.not_checked.is_empty() {
                        self.status
                            .push_str(&format!(" · {} not checked", plan.not_checked.len()));
                    }
                    return;
                }
                let badges = self.badges(&plan);
                self.folder_sync.review = Some(Review::new(scan, plan, badges));
                self.folder_sync.showing = false;
            }
        }
    }
    /// Notices a scan thread that ended without sending its result.
    pub(super) fn poll_folder_sync(&mut self) {
        let lost = self.folder_sync.scan.as_ref().is_some_and(|s| {
            s.thread.as_ref().is_some_and(|t| t.is_finished()) && !s.sent.load(Ordering::Acquire)
        });
        if lost {
            self.folder_sync.scan = None;
            self.status = "Synchronize Folder stopped unexpectedly".into();
        }
        // The review opens on its own once the Library is in front and
        // nothing else is asked.
        if self.folder_sync.review.is_some()
            && !self.folder_sync.showing
            && !self
                .folder_sync
                .review
                .as_ref()
                .is_some_and(|r| r.confirming)
            && self.module == super::library::Module::Library
            && self.modal.is_none()
            && !self.command_modal()
            && self.folder_sync.review.as_ref().is_some_and(|r| !r.seen())
        {
            self.folder_sync.showing = true;
            if let Some(review) = &mut self.folder_sync.review {
                review.mark_seen();
            }
        }
    }
    /// Asks the scan to stop at its next check, for quitting.
    pub(super) fn stop_folder_sync(&mut self) -> Option<super::task::Stopping> {
        let scan = self.folder_sync.scan.as_mut()?;
        scan.cancel.store(true, Ordering::Relaxed);
        Some(super::task::Stopping::new(scan.thread.take()))
    }
    /// The top bar's part: the scan's progress and stop button, or the
    /// button that opens a review waiting.
    pub(super) fn folder_sync_progress(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        if let Some(scan) = &self.folder_sync.scan {
            let files = scan.files.load(Ordering::Relaxed);
            let text = format!(
                "Synchronizing \"{}\" · {} files",
                scan.name,
                thousands(files)
            );
            ui.label(egui::RichText::new(text).size(11.).color(palette.gray(170)));
            let stop = ui
                .add(egui::Button::new(egui::RichText::new("✕").size(11.)).frame(false))
                .on_hover_text("Stop synchronizing");
            if stop.clicked() {
                scan.cancel.store(true, Ordering::Relaxed);
            }
            return;
        }
        if self.folder_sync.review.is_some()
            && !self.folder_sync.showing
            && ui
                .add(egui::Button::new(
                    egui::RichText::new("Review synchronization")
                        .size(11.)
                        .color(palette.accent()),
                ))
                .clicked()
        {
            self.folder_sync.showing = true;
        }
    }
    /// What each photo of `plan` holds, for the review to show.
    fn badges(&self, plan: &SyncPlan) -> HashMap<PhotoId, String> {
        let Some(library) = &self.library else {
            return HashMap::new();
        };
        let session = &library.session;
        let edited = session.catalog.edit_times().unwrap_or_default();
        let photos = plan
            .moves
            .iter()
            .map(|m| m.photo)
            .chain(plan.missing.iter().map(|m| m.photo))
            .chain(
                plan.choices
                    .iter()
                    .flat_map(|c| c.options.iter().map(|m| m.photo)),
            );
        photos
            .map(|id| {
                let copies = session
                    .photos
                    .iter()
                    .filter(|p| p.master == Some(id))
                    .count();
                let photo = session.photos.iter().find(|p| p.id == id);
                let rating = photo.map_or(0, |p| p.rating);
                let keywords = photo.is_some_and(|p| !p.keywords.is_empty());
                let collections = session
                    .collection_photos
                    .values()
                    .filter(|members| members.contains(&id))
                    .count();
                let mut parts = Vec::new();
                if rating > 0 {
                    parts.push("★".repeat(rating as usize));
                }
                if edited.contains_key(&id) {
                    parts.push("edited".into());
                }
                if keywords {
                    parts.push("keywords".into());
                }
                if collections > 0 {
                    parts.push(format!("in {collections} collections"));
                }
                if copies > 0 {
                    parts.push(super::widgets::plural(
                        copies,
                        "virtual copy",
                        "virtual copies",
                    ));
                }
                (id, parts.join(" · "))
            })
            .collect()
    }
    /// The review dialog.
    pub(super) fn folder_sync_window(&mut self, ctx: &egui::Context) {
        if !self.folder_sync.showing {
            return;
        }
        let open = self.document.catalog_photo;
        let open_master = open.and_then(|id| {
            let library = self.library.as_ref()?;
            let photo = library.session.photos.iter().find(|p| p.id == id)?;
            Some(photo.master.unwrap_or(id))
        });
        let Some(review) = &mut self.folder_sync.review else {
            self.folder_sync.showing = false;
            return;
        };
        let mut answer: Option<bool> = None;
        egui::Modal::new(egui::Id::new("folder-sync-review")).show(ctx, |ui| {
            ui.set_width(560.);
            ui.heading(format!("Synchronize Folder \"{}\"", review.name));
            ui.add_space(6.);
            egui::ScrollArea::vertical()
                .max_height(420.)
                .show(ui, |ui| review.sections(ui, open_master));
            ui.add_space(6.);
            ui.label(
                egui::RichText::new("Nothing is moved, renamed or deleted on disk.")
                    .size(11.)
                    .weak(),
            );
            ui.add_space(8.);
            ui.horizontal(|ui| {
                let removing = review.removals.iter().filter(|r| **r).count();
                let label = if removing > 0 {
                    format!("Synchronize and Remove {removing}")
                } else {
                    "Synchronize".into()
                };
                if ui.button(label).clicked() {
                    answer = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(false);
                }
            });
        });
        match answer {
            Some(false) => {
                self.folder_sync.showing = false;
                self.folder_sync.review = None;
            }
            Some(true) => {
                let review = self.folder_sync.review.as_mut().expect("shown");
                let losing = review.removal_losses();
                if !losing.is_empty() && !review.confirming {
                    review.confirming = true;
                } else {
                    self.apply_folder_sync(ctx);
                }
            }
            None => {}
        }
        self.removal_confirm_window(ctx);
    }
    /// Confirms removing photos that hold edits, ratings or copies.
    fn removal_confirm_window(&mut self, ctx: &egui::Context) {
        let Some(review) = &mut self.folder_sync.review else {
            return;
        };
        if !review.confirming {
            return;
        }
        let losing = review.removal_losses();
        let detail = format!(
            "These photos are removed from the catalog with what they hold:\n{}",
            losing.join("\n")
        );
        match super::widgets::confirm_modal(
            ctx,
            "folder-sync-remove",
            "Remove missing photos?",
            &detail,
            false,
            &[("Cancel", false), ("Remove", true)],
            false,
        ) {
            Some(true) => {
                review.confirming = false;
                self.apply_folder_sync(ctx);
            }
            Some(false) => review.confirming = false,
            None => {}
        }
    }
    /// Applies the review's choices as a folder change, then opens the
    /// catalog again.
    fn apply_folder_sync(&mut self, ctx: &egui::Context) {
        let Some(review) = self.folder_sync.review.take() else {
            return;
        };
        self.folder_sync.showing = false;
        let current = self.library.as_ref().is_some_and(|l| {
            *l.session.catalog.location() == review.catalog
                && l.session.generation() == review.generation
        });
        if !current {
            self.status = "The catalog changed since the scan; synchronize again".into();
            return;
        }
        let accepted = review.accepted();
        // The open photo, and its file found moved, as accepted.
        let open = self.document.catalog_photo;
        let open_master = open.and_then(|id| {
            let photo = self.library.as_ref()?.photo(id)?;
            Some(photo.master.unwrap_or(id))
        });
        let open_move = open_master.and_then(|master| {
            let to = accepted
                .moves
                .iter()
                .find(|(id, _)| *id == master)?
                .1
                .clone();
            let verified = review
                .plan
                .moves
                .iter()
                .chain(review.plan.choices.iter().flat_map(|c| c.options.iter()))
                .any(|m| m.photo == master && m.to == to && m.verified);
            verified.then_some(to)
        });
        // The open edit is saved first. Saving fails when its own file was
        // moved: then only a verified move of it lets synchronizing go on,
        // keeping the edit pending, saved on the new path afterwards.
        if self.activity.is_busy() {
            self.status = "Finish what is running first, then synchronize.".into();
            self.folder_sync.review = Some(review);
            return;
        }
        // Edits made in the Library are saved first, whatever happens.
        if !self.commit_library_drafts() {
            self.folder_sync.review = Some(review);
            return;
        }
        let file_gone = self.document.path.as_ref().is_some_and(|p| !p.exists());
        if !self.flush() && !(file_gone && open_move.is_some()) {
            self.folder_sync.review = Some(review);
            return;
        }
        if !self.activity.begin_folder_change() {
            self.folder_sync.review = Some(review);
            return;
        }
        // What the readers bring back could land on photos given removed
        // photos' ids.
        if let Some(library) = &mut self.library {
            library.session.stop_background_reads();
        }
        let location = review.catalog.clone();
        let plan = review.plan;
        let relink = open.map(|id| (id, open_move));
        let ctx = ctx.clone();
        super::task::spawn(
            self.tx.clone(),
            ctx.clone(),
            move |tx| {
                let result = (|| -> anyhow::Result<Synced> {
                    let mut catalog = crate::catalog::Catalog::open(&location)?;
                    let applied = catalog.apply_folder_sync(
                        &plan,
                        &accepted,
                        &crate::volume_mounted,
                        open,
                        &crate::catalog::MetadataDefaults::load(),
                    )?;
                    drop(catalog);
                    // Applied: what follows must not lose it, nor where the
                    // open photo's file went.
                    let opened = crate::catalog_session::CatalogSession::open(&location)
                        .map(|opened| super::library::Library::new(opened, ctx.clone()));
                    let library = match opened {
                        Ok(mut library) => {
                            synced_message(&mut library, &plan, &applied);
                            Ok(Box::new(library))
                        }
                        Err(e) => Err(format!("{e:#}")),
                    };
                    // The open photo stays open while it is in the catalog;
                    // it follows only a move that went through.
                    let relink = relink.and_then(|(id, to)| {
                        let master = match &library {
                            Ok(l) => l.photo(id)?.master.unwrap_or(id),
                            Err(_) => id,
                        };
                        let moved = applied.moved.contains(&master) || applied.moved.contains(&id);
                        Some((id, to.filter(|_| moved)))
                    });
                    Ok(Synced {
                        library,
                        applied,
                        relink,
                    })
                })();
                let _ = tx.send(Event::FolderSyncApplied(
                    result.map(Box::new).map_err(|e| format!("{e:#}")),
                ));
            },
            |tx, message| {
                let _ = tx.send(Event::FolderSyncApplied(Err(message)));
            },
        );
    }
    /// Applying finished: forgets removed photos, moves preview requests
    /// with moved ones, and shows the catalog again, the open photo
    /// following its file.
    pub(super) fn folder_synced(&mut self, result: Result<Box<Synced>, String>) {
        let synced = match result {
            Ok(synced) => synced,
            Err(e) => {
                self.activity.finish_dialog();
                self.status = format!("Synchronize Folder failed: {e}");
                return;
            }
        };
        let Synced {
            library,
            applied,
            relink,
        } = *synced;
        let library = match library {
            Ok(library) => library,
            Err(e) => {
                // Applied, but not shown again: the open photo still
                // follows its file, so its edit saves there.
                self.activity.finish_dialog();
                if let Some((id, Some(path))) = relink
                    && self.document.catalog_photo == Some(id)
                {
                    self.document.relink(path);
                }
                for &id in &applied.removed {
                    self.undo_log.forget_photo(id);
                }
                self.status = format!(
                    "Synchronized, but the catalog could not be opened again: {e}. Open it again."
                );
                return;
            }
        };
        for &id in &applied.removed {
            self.forget_preview_intent(id);
            self.undo_log.forget_photo(id);
        }
        // Each moved photo and its copies, at their new file.
        let moved: Vec<(PhotoId, PathBuf)> = library
            .session
            .photos
            .iter()
            .filter(|p| applied.moved.contains(&p.master.unwrap_or(p.id)))
            .map(|p| (p.id, p.path.clone()))
            .collect();
        self.move_preview_intents(library.session.catalog.location(), &moved);
        self.relink_open = relink;
        self.catalog_ready(Ok(library));
    }
}

/// Whether `plan` offers nothing to do.
fn nothing_to_do(plan: &SyncPlan) -> bool {
    plan.moves.is_empty()
        && plan.choices.is_empty()
        && plan.additions.is_empty()
        && plan.missing.is_empty()
        && plan.conflicts.is_empty()
        && plan.ambiguous.is_empty()
}

/// The status line after applying: "Synchronized · 14 updated · 32 added ·
/// 5 still missing", with every detail on hover.
fn synced_message(library: &mut super::library::Library, plan: &SyncPlan, applied: &Applied) {
    let still_missing = plan
        .missing
        .iter()
        .filter(|m| !applied.removed.contains(&m.photo))
        .count();
    let mut summary = vec![
        format!("{} updated", applied.moved.len()),
        format!("{} added", applied.added),
    ];
    if !applied.removed.is_empty() {
        summary.push(format!("{} removed", applied.removed.len()));
    }
    if still_missing > 0 {
        summary.push(format!("{still_missing} still missing"));
    }
    if !applied.skipped.is_empty() {
        summary.push(format!("{} skipped", applied.skipped.len()));
    }
    let mut detail: Vec<String> = Vec::new();
    for s in &applied.skipped {
        detail.push(format!("Skipped {}: {}", s.path.display(), s.reason));
    }
    if !applied.protected.is_empty() {
        detail.push(format!(
            "{} kept their edit protected: it was saved for another version of the file",
            super::widgets::plural(applied.protected.len(), "photo", "photos")
        ));
    }
    for n in &plan.not_checked {
        detail.push(format!("Not checked {}: {}", n.path.display(), n.reason));
    }
    if let Some(e) = &applied.add_error {
        summary.push("adding new files failed".into());
        detail.push(format!("Adding new files failed: {e:#}"));
    }
    let report = applied.report.details();
    if !report.is_empty() {
        detail.push(report);
        detail.push(
            "Read Metadata from Files reads their metadata again once the files can be read."
                .into(),
        );
    }
    library.set_message_with_detail(
        format!("Synchronized · {}", summary.join(" · ")),
        detail.join("\n"),
    );
    library.offer_previous_import = applied.added > 0;
}

impl Review {
    fn new(scan: Scan, plan: SyncPlan, badges: HashMap<PhotoId, String>) -> Self {
        Self {
            moves: plan.moves.iter().map(|m| m.verified).collect(),
            picks: vec![None; plan.choices.len()],
            additions: plan
                .additions
                .iter()
                .map(|a| a.copy_of.is_empty())
                .collect(),
            removals: vec![false; plan.missing.len()],
            expanded: BTreeSet::new(),
            badges,
            confirming: false,
            name: scan.name,
            catalog: scan.catalog,
            generation: scan.generation,
            plan,
        }
    }
    /// Whether the review was opened once already.
    fn seen(&self) -> bool {
        self.expanded.contains("seen")
    }
    fn mark_seen(&mut self) {
        self.expanded.insert("seen");
    }
    /// What the user accepted.
    fn accepted(&self) -> Accepted {
        let mut moves: Vec<(PhotoId, PathBuf)> = self
            .plan
            .moves
            .iter()
            .zip(&self.moves)
            .filter(|(_, on)| **on)
            .map(|(m, _)| (m.photo, m.to.clone()))
            .collect();
        for (choice, pick) in self.plan.choices.iter().zip(&self.picks) {
            if let Some(m) = pick.and_then(|i| choice.options.get(i)) {
                moves.push((m.photo, m.to.clone()));
            }
        }
        Accepted {
            moves,
            additions: self
                .plan
                .additions
                .iter()
                .zip(&self.additions)
                .filter(|(_, on)| **on)
                .map(|(a, _)| a.file.clone())
                .collect(),
            removals: self
                .plan
                .missing
                .iter()
                .zip(&self.removals)
                .filter(|(_, on)| **on)
                .map(|(m, _)| m.photo)
                .collect(),
        }
    }
    /// The photos to remove that hold something, each with what.
    fn removal_losses(&self) -> Vec<String> {
        self.plan
            .missing
            .iter()
            .zip(&self.removals)
            .filter(|(_, on)| **on)
            .filter_map(|(m, _)| {
                let badge = self.badges.get(&m.photo).filter(|b| !b.is_empty())?;
                Some(format!(
                    "{}: {badge}",
                    m.path.file_name().unwrap_or_default().to_string_lossy()
                ))
            })
            .collect()
    }
    /// Each section: a box for all of it, its count, and its list.
    fn sections(&mut self, ui: &mut egui::Ui, open_master: Option<PhotoId>) {
        let short = |p: &std::path::Path| {
            p.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        };
        let shown = |p: &std::path::Path| p.display().to_string();
        let verified: Vec<usize> = (0..self.plan.moves.len())
            .filter(|i| self.plan.moves[*i].verified)
            .collect();
        let probable: Vec<usize> = (0..self.plan.moves.len())
            .filter(|i| !self.plan.moves[*i].verified)
            .collect();
        for (key, title, indices) in [
            ("verified", "Update moved or renamed photos", verified),
            ("probable", "Probably moved, not verified", probable),
        ] {
            if indices.is_empty() {
                continue;
            }
            let mut all = indices.iter().all(|i| self.moves[*i]);
            let n = indices.len();
            if self.header(ui, key, &mut all, &format!("{title} ({n})")) {
                for &i in &indices {
                    let open = open_master == Some(self.plan.moves[i].photo);
                    // A probable match can't take the open photo along.
                    self.moves[i] = all && !(open && key == "probable");
                }
            }
            if self.expanded.contains(key) {
                for &i in &indices {
                    let m = &self.plan.moves[i];
                    let open = open_master == Some(m.photo);
                    ui.horizontal(|ui| {
                        ui.add_space(18.);
                        let enabled = !(open && !m.verified);
                        ui.add_enabled(enabled, egui::Checkbox::new(&mut self.moves[i], ""));
                        ui.label(format!("{} → {}", short(&m.from), shown(&m.to)))
                            .on_hover_text(shown(&m.from));
                        badge(ui, self.badges.get(&m.photo));
                        if open && !m.verified {
                            ui.label(egui::RichText::new("open in Develop, not verified").weak());
                        }
                    });
                }
            }
        }
        let copies: Vec<usize> = (0..self.plan.additions.len())
            .filter(|i| !self.plan.additions[*i].copy_of.is_empty())
            .collect();
        let new: Vec<usize> = (0..self.plan.additions.len())
            .filter(|i| self.plan.additions[*i].copy_of.is_empty())
            .collect();
        for (key, title, indices) in [
            ("new", "Add new photos", new),
            (
                "copies",
                "Add copies of photos already in the catalog",
                copies,
            ),
        ] {
            if indices.is_empty() {
                continue;
            }
            let mut all = indices.iter().all(|i| self.additions[*i]);
            if self.header(ui, key, &mut all, &format!("{title} ({})", indices.len())) {
                for &i in &indices {
                    self.additions[i] = all;
                }
            }
            if self.expanded.contains(key) {
                for &i in &indices {
                    ui.horizontal(|ui| {
                        ui.add_space(18.);
                        ui.checkbox(&mut self.additions[i], "");
                        ui.label(shown(&self.plan.additions[i].file));
                    });
                }
            }
        }
        if !self.plan.missing.is_empty() {
            let mut all = self.removals.iter().all(|r| *r);
            let n = self.plan.missing.len();
            if self.header(
                ui,
                "missing",
                &mut all,
                &format!("Remove missing photos from the catalog ({n})"),
            ) {
                for (i, m) in self.plan.missing.iter().enumerate() {
                    self.removals[i] = all && open_master != Some(m.photo);
                }
            }
            let holding = self
                .plan
                .missing
                .iter()
                .filter(|m| self.badges.get(&m.photo).is_some_and(|b| !b.is_empty()))
                .count();
            if holding > 0 {
                ui.horizontal(|ui| {
                    ui.add_space(24.);
                    ui.label(
                        egui::RichText::new(format!(
                            "{} edits, ratings or copies",
                            super::widgets::plural(holding, "has", "have")
                        ))
                        .size(11.)
                        .weak(),
                    );
                });
            }
            if self.expanded.contains("missing") {
                for (i, m) in self.plan.missing.iter().enumerate() {
                    let open = open_master == Some(m.photo);
                    ui.horizontal(|ui| {
                        ui.add_space(18.);
                        ui.add_enabled(!open, egui::Checkbox::new(&mut self.removals[i], ""));
                        ui.label(shown(&m.path));
                        badge(ui, self.badges.get(&m.photo));
                        if open {
                            ui.label(egui::RichText::new("open in Develop").weak());
                        }
                    });
                }
            }
        }
        if !self.plan.choices.is_empty() {
            ui.add_space(4.);
            ui.label(format!("⚠ Needs a choice ({})", self.plan.choices.len()));
            for (c, choice) in self.plan.choices.iter().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(18.);
                    let selected = self.picks[c]
                        .and_then(|i| choice.options.get(i))
                        .map_or("Skip".to_string(), |m| {
                            format!("{} → {}", short(&m.from), shown(&m.to))
                        });
                    egui::ComboBox::from_id_salt(("folder-sync-choice", c))
                        .selected_text(selected)
                        .width(480.)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.picks[c], None, "Skip");
                            for (i, m) in choice.options.iter().enumerate() {
                                let open = open_master == Some(m.photo) && !m.verified;
                                ui.add_enabled_ui(!open, |ui| {
                                    ui.selectable_value(
                                        &mut self.picks[c],
                                        Some(i),
                                        format!("{} → {}", short(&m.from), shown(&m.to)),
                                    );
                                });
                            }
                        });
                });
            }
        }
        let not_checked =
            self.plan.not_checked.len() + self.plan.conflicts.len() + self.plan.ambiguous.len();
        if not_checked > 0 {
            ui.add_space(4.);
            let mut lines: Vec<String> = self
                .plan
                .not_checked
                .iter()
                .map(|n| format!("{}: {}", shown(&n.path), n.reason))
                .collect();
            lines.extend(self.plan.conflicts.iter().map(|c| c.message()));
            lines.extend(
                self.plan
                    .ambiguous
                    .iter()
                    .map(|d| format!("{}: more than one catalog folder could be it", shown(d))),
            );
            ui.label(format!("ⓘ Not checked ({not_checked})"))
                .on_hover_text(lines.join("\n"));
        }
    }
    /// A section's header: its box and title, and a toggle for its list.
    /// Returns whether the box changed.
    fn header(&mut self, ui: &mut egui::Ui, key: &'static str, on: &mut bool, title: &str) -> bool {
        let mut changed = false;
        ui.horizontal(|ui| {
            changed = ui.checkbox(on, title).changed();
            let open = self.expanded.contains(key);
            if ui
                .small_button(if open { "▾ list" } else { "▸ list" })
                .clicked()
            {
                if open {
                    self.expanded.remove(key);
                } else {
                    self.expanded.insert(key);
                }
            }
        });
        changed
    }
}

/// What a photo holds, after its row.
fn badge(ui: &mut egui::Ui, badge: Option<&String>) {
    if let Some(b) = badge.filter(|b| !b.is_empty()) {
        ui.label(egui::RichText::new(b).size(11.).weak());
    }
}

/// `n` with thousands separated: 1,240.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::library::Module;
    use crate::catalog::ReadChecksum;
    use crate::storage::{ChecksumRead, FileChecksum};
    use std::time::{Duration, Instant};

    /// Distinct synthetic bytes, larger than the quick hash's blocks.
    fn bytes(seed: u8) -> Vec<u8> {
        (0..300_000u32)
            .map(|i| (i as u8).wrapping_mul(seed) ^ seed)
            .collect()
    }
    /// An editor on a catalog of a folder "trip" with two photos, their
    /// checksums on record.
    fn editor() -> anyhow::Result<(tempfile::TempDir, Editor, PathBuf)> {
        let dir = tempfile::tempdir()?;
        let trip = dir.path().canonicalize()?.join("trip");
        std::fs::create_dir_all(&trip)?;
        std::fs::write(trip.join("a.dng"), bytes(1))?;
        std::fs::write(trip.join("b.dng"), bytes(2))?;
        let path = dir.path().join("photos.rawmakase");
        let mut catalog = crate::catalog::Catalog::create(&path)?;
        catalog.add_folder(&trip)?;
        let read: Vec<ReadChecksum> = catalog
            .photos()?
            .into_iter()
            .map(|p| {
                let ChecksumRead::Done(checksum) = FileChecksum::read_full(&p.path, |_| true)?
                else {
                    anyhow::bail!("not read");
                };
                Ok(ReadChecksum {
                    photo: p.id,
                    folder: p.folder,
                    filename: p.filename,
                    path: p.path,
                    checksum,
                })
            })
            .collect::<anyhow::Result<_>>()?;
        catalog.save_checksums(&read)?;
        drop(catalog);
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
        e.library = Some(Box::new(crate::app::library::Library::load(&path, ctx)?));
        e.module = Module::Library;
        Ok((dir, e, trip))
    }
    fn id(e: &Editor, name: &str) -> PhotoId {
        let library = e.library.as_ref().unwrap();
        library
            .session
            .photos
            .iter()
            .find(|p| p.filename == name)
            .unwrap()
            .id
    }
    /// Delivers events until the scan is over.
    fn scanned(e: &mut Editor) {
        let ctx = e.context.clone();
        let until = Instant::now() + Duration::from_secs(10);
        while e.folder_sync.scan.is_some() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
            e.events(&ctx);
            e.poll_folder_sync();
        }
        assert!(e.folder_sync.scan.is_none(), "the scan never finished");
    }
    /// Delivers events until applying is over.
    fn applied(e: &mut Editor) {
        let ctx = e.context.clone();
        let until = Instant::now() + Duration::from_secs(10);
        while e.activity.is_busy() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
            e.events(&ctx);
        }
        assert!(!e.activity.is_busy(), "applying never finished");
    }

    #[test]
    fn a_renamed_photo_is_reviewed_and_relinked_with_its_rating() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let a = id(&e, "a.dng");
        e.library
            .as_mut()
            .unwrap()
            .session
            .set_ratings(&[(a, 4, 0, String::new())])?;
        std::fs::rename(trip.join("a.dng"), trip.join("renamed.dng"))?;
        std::fs::write(trip.join("new.dng"), bytes(3))?;
        e.start_folder_sync(SyncScope::All, "trip".into());
        scanned(&mut e);
        // The review opens on its own in the Library.
        e.poll_folder_sync();
        assert!(e.folder_sync.showing);
        let ctx = e.context.clone();
        e.apply_folder_sync(&ctx);
        applied(&mut e);
        let library = e.library.as_ref().unwrap();
        let photo = library.photo(a).unwrap();
        assert_eq!(
            (photo.path.clone(), photo.rating),
            (trip.join("renamed.dng"), 4)
        );
        assert!(
            e.status.starts_with("Synchronized · 1 updated · 1 added"),
            "{}",
            e.status
        );
        // The photo added is Previous Import.
        assert_eq!(library.session.previous_import.len(), 1);
        assert!(library.offer_previous_import);
        Ok(())
    }

    #[test]
    fn a_scan_from_before_a_structural_change_is_not_offered() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        std::fs::rename(trip.join("a.dng"), trip.join("renamed.dng"))?;
        e.start_folder_sync(SyncScope::All, "trip".into());
        // A folder change while it scans.
        e.library.as_mut().unwrap().session.reload()?;
        scanned(&mut e);
        assert!(e.folder_sync.review.is_none());
        assert!(e.status.contains("changed during the scan"), "{}", e.status);
        Ok(())
    }

    #[test]
    fn a_scan_thread_lost_without_a_result_clears_the_top_bar() -> anyhow::Result<()> {
        let (_dir, mut e, _) = editor()?;
        let thread = std::thread::spawn(|| {});
        while !thread.is_finished() {
            std::thread::yield_now();
        }
        e.folder_sync.scan = Some(Scan {
            id: 99,
            name: "trip".into(),
            catalog: e
                .library
                .as_ref()
                .unwrap()
                .session
                .catalog
                .location()
                .clone(),
            generation: 0,
            files: Default::default(),
            cancel: Default::default(),
            sent: Default::default(),
            thread: Some(thread),
        });
        e.poll_folder_sync();
        assert!(!e.folder_sync.is_scanning());
        assert!(e.status.contains("stopped unexpectedly"));
        Ok(())
    }

    #[test]
    fn the_open_photo_moved_outside_keeps_its_pending_edit_and_saves_it_there() -> anyhow::Result<()>
    {
        let (_dir, mut e, trip) = editor()?;
        let a = id(&e, "a.dng");
        // Open in Develop with an edit not yet saved.
        e.module = Module::Develop;
        e.document.catalog_photo = Some(a);
        e.document.path = Some(trip.join("a.dng"));
        e.document.edit.change(None, |r| r.exposure = 0.8);
        let steps = e.document.edit.history().steps().0.len();
        std::fs::rename(trip.join("a.dng"), trip.join("moved.dng"))?;
        // Saving to the old path fails now.
        assert!(!e.flush());
        e.start_folder_sync(SyncScope::All, "trip".into());
        scanned(&mut e);
        let ctx = e.context.clone();
        e.apply_folder_sync(&ctx);
        applied(&mut e);
        assert_eq!(
            e.document.path.as_deref(),
            Some(trip.join("moved.dng").as_path())
        );
        assert_eq!(e.document.catalog_photo, Some(a));
        assert_eq!(e.document.edit.history().steps().0.len(), steps);
        assert!(e.module == Module::Develop);
        // The save goes through on the new path.
        assert!(e.flush());
        let library = e.library.as_ref().unwrap();
        let saved = library
            .session
            .catalog
            .load_edit(a, &trip.join("moved.dng"))?
            .expect("saved");
        assert_eq!(saved.recipe.exposure, 0.8);
        Ok(())
    }

    #[test]
    fn the_open_photo_whose_move_is_skipped_stays_open_with_its_edit() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let a = id(&e, "a.dng");
        e.module = Module::Develop;
        e.document.catalog_photo = Some(a);
        e.document.path = Some(trip.join("a.dng"));
        e.document.edit.change(None, |r| r.exposure = 0.6);
        std::fs::rename(trip.join("a.dng"), trip.join("moved.dng"))?;
        e.start_folder_sync(SyncScope::All, "trip".into());
        scanned(&mut e);
        // The file found changes before applying: its move is skipped.
        let later = std::time::SystemTime::now() + Duration::from_secs(30);
        std::fs::File::options()
            .write(true)
            .open(trip.join("moved.dng"))?
            .set_modified(later)?;
        let ctx = e.context.clone();
        e.apply_folder_sync(&ctx);
        applied(&mut e);
        assert_eq!(e.document.catalog_photo, Some(a));
        assert_eq!(
            e.document.path.as_deref(),
            Some(trip.join("a.dng").as_path())
        );
        assert_eq!(e.document.edit.recipe().exposure, 0.6);
        assert!(e.document.edit.save_state().needs_save());
        assert!(e.module == Module::Develop);
        Ok(())
    }

    #[test]
    fn removing_a_photo_with_keywords_asks_first() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let a = id(&e, "a.dng");
        let library = e.library.as_mut().unwrap();
        library
            .session
            .catalog
            .add_keywords(&[a], &[vec!["Trip".into()]])?;
        library.session.reload()?;
        std::fs::remove_file(trip.join("a.dng"))?;
        e.start_folder_sync(SyncScope::All, "trip".into());
        scanned(&mut e);
        let review = e.folder_sync.review.as_mut().expect("a review");
        review.removals.iter_mut().for_each(|r| *r = true);
        let losses = review.removal_losses();
        assert_eq!(losses.len(), 1);
        assert!(losses[0].contains("keywords"), "{losses:?}");
        Ok(())
    }

    #[test]
    fn the_review_is_modal() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        std::fs::remove_file(trip.join("a.dng"))?;
        e.start_folder_sync(SyncScope::All, "trip".into());
        scanned(&mut e);
        e.poll_folder_sync();
        assert!(e.folder_sync.showing);
        // Keys and commands go to the dialog, not the photos behind it.
        assert!(e.command_modal());
        Ok(())
    }
}
