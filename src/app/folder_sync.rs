//! Synchronize Folder (issue #432) in the app: the scan, on a thread of its
//! own while the Library and Develop stay usable, shown in the top bar with
//! a stop button; the review dialog; and applying what the user accepted as
//! a folder change. The photo open in Develop keeps its pending edit: a
//! verified move of its file relinks it in place (see `catalog_ready`).
use super::Editor;
use super::icons::Icon;
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
    /// The folder synchronized, on this computer; none for all of them.
    root: Option<PathBuf>,
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
    badges: HashMap<PhotoId, Vec<String>>,
    /// The folder synchronized, which rows are shown from; none for all.
    root: Option<PathBuf>,
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
        let root = match scope {
            SyncScope::Folder(folder) => library
                .session
                .folders
                .iter()
                .find(|f| f.id == folder)
                .map(|f| f.path.clone()),
            SyncScope::Root(root) => library
                .session
                .roots
                .iter()
                .find(|(id, ..)| *id == root)
                .map(|(_, original, located)| {
                    PathBuf::from(located.as_deref().unwrap_or(original))
                }),
            SyncScope::All => None,
        };
        self.folder_sync.scan = Some(Scan {
            id,
            name,
            root,
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
    /// button that opens a review waiting, as the export's progress shows.
    pub(super) fn folder_sync_progress(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        if let Some(scan) = &self.folder_sync.scan {
            let files = scan.files.load(Ordering::Relaxed);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(250., 28.), egui::Sense::hover());
            let painter = ui.painter();
            painter.text(
                rect.left_center() + egui::vec2(0., -6.),
                egui::Align2::LEFT_CENTER,
                format!("Synchronizing \u{201c}{}\u{201d}", scan.name),
                egui::FontId::proportional(12.),
                palette.gray(210),
            );
            painter.text(
                rect.left_center() + egui::vec2(0., 8.),
                egui::Align2::LEFT_CENTER,
                format!("{} files looked at", thousands(files)),
                egui::FontId::proportional(10.5),
                palette.gray(135),
            );
            let close = egui::Rect::from_center_size(
                rect.right_center() - egui::vec2(10., 0.),
                egui::vec2(20., 20.),
            );
            let stop = ui
                .interact(
                    close,
                    ui.id().with("stop-folder-sync"),
                    egui::Sense::click(),
                )
                .on_hover_text("Stop synchronizing");
            let tint = palette.gray(if stop.hovered() { 235 } else { 150 });
            super::icons::paint_at(ui.painter(), Icon::Close, close.center(), 13., tint);
            if stop.clicked() {
                scan.cancel.store(true, Ordering::Relaxed);
            }
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(250));
            return;
        }
        if let Some(review) = &self.folder_sync.review
            && !self.folder_sync.showing
        {
            let label = format!("Review synchronization of \u{201c}{}\u{201d}", review.name);
            let button =
                egui::Button::new(egui::RichText::new(label).size(12.).color(palette.accent()))
                    .frame(false);
            if ui
                .add(button)
                .on_hover_text("Open the changes found")
                .clicked()
            {
                self.folder_sync.showing = true;
            }
        }
    }
    /// What each photo of `plan` holds, for the review to show.
    fn badges(&self, plan: &SyncPlan) -> HashMap<PhotoId, Vec<String>> {
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
                    parts.push("\u{2605}".repeat(rating as usize));
                }
                if edited.contains_key(&id) {
                    parts.push("Edited".into());
                }
                if keywords {
                    parts.push("Keywords".into());
                }
                if collections > 0 {
                    parts.push(super::widgets::plural(
                        collections,
                        "collection",
                        "collections",
                    ));
                }
                if copies > 0 {
                    parts.push(super::widgets::plural(
                        copies,
                        "virtual copy",
                        "virtual copies",
                    ));
                }
                (id, parts)
            })
            .collect()
    }
    /// The review dialog: one card for each kind of change, each saying
    /// what it does, with its photos below.
    pub(super) fn folder_sync_window(&mut self, ctx: &egui::Context) {
        if !self.folder_sync.showing {
            return;
        }
        let open = self.document.catalog_photo;
        let open_master = open.and_then(|id| {
            let photo = self.library.as_ref()?.photo(id)?;
            Some(photo.master.unwrap_or(id))
        });
        let thumbnails: HashMap<PhotoId, egui::TextureHandle> =
            match (&self.library, &self.folder_sync.review) {
                (Some(library), Some(review)) => review
                    .photos()
                    .filter_map(|id| Some((id, library.thumbnail(id)?.clone())))
                    .collect(),
                _ => HashMap::new(),
            };
        let Some(review) = &mut self.folder_sync.review else {
            self.folder_sync.showing = false;
            return;
        };
        let palette = theme::palette(ctx);
        let mut answer: Option<bool> = None;
        let response = egui::Modal::new(egui::Id::new("folder-sync-review"))
            .backdrop_color(egui::Color32::from_black_alpha(140))
            .frame(super::widgets::modal_frame(&palette))
            .show(ctx, |ui| {
                ui.set_width(WIDTH);
                ui.spacing_mut().item_spacing = egui::vec2(8., 8.);
                // Header: what was looked at, and what was found.
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 24,
                        right: 24,
                        top: 20,
                        bottom: 14,
                    })
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(format!(
                                "Synchronize \u{201c}{}\u{201d}",
                                review.name
                            ))
                            .size(16.)
                            .color(palette.gray(236)),
                        );
                        ui.add_space(2.);
                        ui.label(
                            egui::RichText::new(review.summary())
                                .size(12.)
                                .color(palette.gray(150)),
                        );
                    });
                separator(ui, &palette);
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 220.).max(240.))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(16, 14))
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing = egui::vec2(8., 10.);
                                review.cards(ui, open_master, &thumbnails);
                            });
                    });
                separator(ui, &palette);
                // Footer: the promise about the disk, then the buttons.
                egui::Frame::new()
                    .inner_margin(egui::Margin {
                        left: 24,
                        right: 24,
                        top: 12,
                        bottom: 16,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(
                                    "Only the catalog changes. Files on disk stay as they are.",
                                )
                                .size(12.)
                                .color(palette.gray(135)),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.spacing_mut().button_padding = egui::vec2(18., 6.);
                                    let removing = review.removals.iter().filter(|r| **r).count();
                                    let nothing = review.accepted_nothing();
                                    let label = if removing > 0 {
                                        format!("Synchronize and Remove {removing}")
                                    } else {
                                        "Synchronize".into()
                                    };
                                    let confirm = ui.add_enabled_ui(!nothing, |ui| {
                                        super::widgets::primary_button(ui, &label)
                                    });
                                    if confirm.inner.clicked() {
                                        answer = Some(true);
                                    }
                                    if nothing {
                                        confirm.response.on_hover_text("Nothing is ticked");
                                    }
                                    if ui
                                        .add(
                                            egui::Button::new("Cancel")
                                                .min_size(egui::vec2(84., 30.)),
                                        )
                                        .clicked()
                                    {
                                        answer = Some(false);
                                    }
                                },
                            );
                        });
                    });
            });
        if answer.is_none() && response.should_close() && !review.confirming {
            answer = Some(false);
        }
        match answer {
            Some(false) => {
                self.folder_sync.showing = false;
                self.folder_sync.review = None;
            }
            Some(true) => {
                let review = self.folder_sync.review.as_mut().expect("shown");
                if !review.removal_losses().is_empty() && !review.confirming {
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
        // The open photo's master, as the review saw it: what moves is the
        // master's family.
        let relink = open.map(|id| (id, open_master.unwrap_or(id), open_move));
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
                    let relink = relink.and_then(|(id, master, to)| {
                        // Gone from the catalog: nothing to keep open.
                        if let Ok(l) = &library {
                            l.photo(id)?;
                        }
                        Some((id, to.filter(|_| applied.moved.contains(&master))))
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
                    self.forget_preview_intent(id);
                    self.undo_log.forget_photo(id);
                }
                // The lists shown must not keep photos removed, whose ids
                // new photos may have: read again here, or emptied. The
                // Library stays, so an edit still unsaved saves to it.
                let reread = self
                    .library
                    .as_mut()
                    .map(|l| l.reread_after(&applied.removed));
                if let Some(Err(again)) = reread {
                    self.status = format!(
                        "Synchronized, but the catalog could not be read again: {again:#}. Open it again."
                    );
                } else {
                    self.status = format!(
                        "Synchronized, but the catalog could not be opened again: {e}. It was read again."
                    );
                }
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
    fn new(scan: Scan, plan: SyncPlan, badges: HashMap<PhotoId, Vec<String>>) -> Self {
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
            root: scan.root.or_else(|| common_folder(&plan)),
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
                let badge = self
                    .badges
                    .get(&m.photo)
                    .filter(|b| !b.is_empty())?
                    .join(", ");
                Some(format!(
                    "{}: {badge}",
                    m.path.file_name().unwrap_or_default().to_string_lossy()
                ))
            })
            .collect()
    }
    /// Every photo the review shows, for their thumbnails.
    fn photos(&self) -> impl Iterator<Item = PhotoId> + '_ {
        self.plan
            .moves
            .iter()
            .map(|m| m.photo)
            .chain(self.plan.missing.iter().map(|m| m.photo))
            .chain(
                self.plan
                    .choices
                    .iter()
                    .flat_map(|c| c.options.iter().map(|m| m.photo)),
            )
    }
    /// "4 changes found in 1,240 files" and the like, under the title.
    fn summary(&self) -> String {
        let p = &self.plan;
        let mut parts = Vec::new();
        let moved = p.moves.len();
        if moved > 0 {
            parts.push(super::widgets::plural(moved, "photo moved", "photos moved"));
        }
        let new = p.additions.len();
        if new > 0 {
            parts.push(super::widgets::plural(new, "new file", "new files"));
        }
        if !p.missing.is_empty() {
            parts.push(format!("{} missing", p.missing.len()));
        }
        if !p.choices.is_empty() {
            parts.push(super::widgets::plural(
                p.choices.len(),
                "needs a choice",
                "need a choice",
            ));
        }
        if parts.is_empty() {
            "Nothing to change".into()
        } else {
            parts.join(" \u{b7} ")
        }
    }
    /// Whether nothing at all is ticked or picked.
    fn accepted_nothing(&self) -> bool {
        !self.moves.iter().any(|m| *m)
            && !self.additions.iter().any(|a| *a)
            && !self.removals.iter().any(|r| *r)
            && self.picks.iter().all(Option::is_none)
    }
    /// Where `path` is, from the folder synchronized when it is below it.
    fn shown(&self, path: &std::path::Path) -> String {
        match self
            .root
            .as_deref()
            .and_then(|root| path.strip_prefix(root).ok())
        {
            Some(rest) => rest.display().to_string(),
            None => super::widgets::pretty_path(path),
        }
    }
    /// The folder `dir` by name: its path from the folder synchronized, or
    /// that folder's own name.
    fn place(&self, dir: Option<&std::path::Path>) -> String {
        let Some(dir) = dir else {
            return String::new();
        };
        let shown = self.shown(dir);
        if shown.is_empty() {
            dir.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(shown)
        } else {
            shown
        }
    }
    /// One card for each kind of change found.
    fn cards(
        &mut self,
        ui: &mut egui::Ui,
        open_master: Option<PhotoId>,
        thumbnails: &HashMap<PhotoId, egui::TextureHandle>,
    ) {
        let palette = theme::palette(ui.ctx());
        let verified: Vec<usize> = (0..self.plan.moves.len())
            .filter(|i| self.plan.moves[*i].verified)
            .collect();
        let probable: Vec<usize> = (0..self.plan.moves.len())
            .filter(|i| !self.plan.moves[*i].verified)
            .collect();
        for (key, kind, indices) in [
            ("verified", Kind::Moved, verified),
            ("probable", Kind::Probable, probable),
        ] {
            if indices.is_empty() {
                continue;
            }
            let mut all = indices.iter().all(|i| self.moves[*i]);
            let some = indices.iter().any(|i| self.moves[*i]);
            let open_list = self.expanded.contains(key);
            let (toggled, expand) =
                card_header(ui, &palette, kind, indices.len(), &mut all, some, open_list);
            if toggled {
                for &i in &indices {
                    let open = open_master == Some(self.plan.moves[i].photo);
                    // A probable match can't take the open photo along.
                    self.moves[i] = all && !(open && kind == Kind::Probable);
                }
            }
            self.toggle(key, expand);
            if self.expanded.contains(key) {
                list(ui, &palette, |ui| {
                    for &i in &indices {
                        let m = self.plan.moves[i].clone();
                        let open = open_master == Some(m.photo);
                        let enabled = !(open && !m.verified);
                        let mut on = self.moves[i];
                        let note = (open && !m.verified).then_some("Open in Develop, not verified");
                        row(
                            ui,
                            &palette,
                            Row {
                                on: Some((&mut on, enabled)),
                                thumbnail: thumbnails.get(&m.photo),
                                title: &file_name(&m.from),
                                detail: &format!("now {}", self.shown(&m.to)),
                                hover: &format!(
                                    "{}\n\u{2192} {}",
                                    m.from.display(),
                                    m.to.display()
                                ),
                                chips: self.badges.get(&m.photo).map(Vec::as_slice).unwrap_or(&[]),
                                note,
                            },
                        );
                        self.moves[i] = on;
                    }
                });
            }
        }
        if !self.plan.choices.is_empty() {
            self.choice_card(ui, &palette, open_master, thumbnails);
        }
        let new: Vec<usize> = (0..self.plan.additions.len())
            .filter(|i| self.plan.additions[*i].copy_of.is_empty())
            .collect();
        let copies: Vec<usize> = (0..self.plan.additions.len())
            .filter(|i| !self.plan.additions[*i].copy_of.is_empty())
            .collect();
        for (key, kind, indices) in [("new", Kind::New, new), ("copies", Kind::Copies, copies)] {
            if indices.is_empty() {
                continue;
            }
            let mut all = indices.iter().all(|i| self.additions[*i]);
            let some = indices.iter().any(|i| self.additions[*i]);
            let open_list = self.expanded.contains(key);
            let (toggled, expand) =
                card_header(ui, &palette, kind, indices.len(), &mut all, some, open_list);
            if toggled {
                for &i in &indices {
                    self.additions[i] = all;
                }
            }
            self.toggle(key, expand);
            if self.expanded.contains(key) {
                list(ui, &palette, |ui| {
                    for &i in &indices {
                        let a = self.plan.additions[i].clone();
                        let detail = format!("in {}", self.place(a.file.parent()));
                        let mut on = self.additions[i];
                        let copy_of = a
                            .copy_of
                            .first()
                            .map(|id| format!("Same as a photo already here (#{})", id.0));
                        row(
                            ui,
                            &palette,
                            Row {
                                on: Some((&mut on, true)),
                                thumbnail: None,
                                title: &file_name(&a.file),
                                detail: &detail,
                                hover: &a.file.display().to_string(),
                                chips: &[],
                                note: copy_of.as_deref(),
                            },
                        );
                        self.additions[i] = on;
                    }
                });
            }
        }
        if !self.plan.missing.is_empty() {
            let mut all = self.removals.iter().all(|r| *r);
            let some = self.removals.iter().any(|r| *r);
            let open_list = self.expanded.contains("missing");
            let (toggled, expand) = card_header(
                ui,
                &palette,
                Kind::Missing,
                self.plan.missing.len(),
                &mut all,
                some,
                open_list,
            );
            if toggled {
                for (i, m) in self.plan.missing.iter().enumerate() {
                    self.removals[i] = all && open_master != Some(m.photo);
                }
            }
            self.toggle("missing", expand);
            if self.expanded.contains("missing") {
                list(ui, &palette, |ui| {
                    for i in 0..self.plan.missing.len() {
                        let m = self.plan.missing[i].clone();
                        let open = open_master == Some(m.photo);
                        let mut on = self.removals[i];
                        let detail = format!("was in {}", self.place(m.path.parent()));
                        row(
                            ui,
                            &palette,
                            Row {
                                on: Some((&mut on, !open)),
                                thumbnail: thumbnails.get(&m.photo),
                                title: &file_name(&m.path),
                                detail: &detail,
                                hover: &m.path.display().to_string(),
                                chips: self.badges.get(&m.photo).map(Vec::as_slice).unwrap_or(&[]),
                                note: open.then_some("Open in Develop"),
                            },
                        );
                        self.removals[i] = on;
                    }
                });
            }
        }
        let not_checked =
            self.plan.not_checked.len() + self.plan.conflicts.len() + self.plan.ambiguous.len();
        if not_checked > 0 {
            let mut lines: Vec<String> = self
                .plan
                .not_checked
                .iter()
                .map(|n| format!("{}: {}", n.path.display(), n.reason))
                .collect();
            lines.extend(self.plan.conflicts.iter().map(|c| c.message()));
            lines.extend(
                self.plan
                    .ambiguous
                    .iter()
                    .map(|d| format!("{}: more than one catalog folder could be it", d.display())),
            );
            ui.add_space(2.);
            ui.horizontal(|ui| {
                ui.add_space(4.);
                ui.label(
                    egui::RichText::new(format!(
                        "\u{24d8}  {} could not be checked here, so nothing in {} counts as missing.",
                        super::widgets::plural(not_checked, "place", "places"),
                        if not_checked == 1 { "it" } else { "them" }
                    ))
                    .size(12.)
                    .color(palette.gray(140)),
                )
                .on_hover_text(lines.join("\n"));
            });
        }
    }
    /// Shows or hides the list of the card `key`.
    fn toggle(&mut self, key: &'static str, flip: bool) {
        if flip && !self.expanded.remove(key) {
            self.expanded.insert(key);
        }
    }
    /// The card for photos found more than once: for each, the files it may
    /// be, to pick one or leave it.
    fn choice_card(
        &mut self,
        ui: &mut egui::Ui,
        palette: &theme::Palette,
        open_master: Option<PhotoId>,
        thumbnails: &HashMap<PhotoId, egui::TextureHandle>,
    ) {
        card(ui, palette, |ui| {
            ui.horizontal(|ui| {
                kind_icon(ui, palette, Kind::Choice);
                ui.label(
                    egui::RichText::new(Kind::Choice.title(self.plan.choices.len()))
                        .size(13.)
                        .color(palette.gray(230)),
                );
            });
            ui.label(
                egui::RichText::new(Kind::Choice.explanation())
                    .size(12.)
                    .color(palette.gray(145)),
            );
            for c in 0..self.plan.choices.len() {
                let choice = self.plan.choices[c].clone();
                // The photos this choice is about, each with what it holds.
                let mut photos: Vec<PhotoId> = choice.options.iter().map(|m| m.photo).collect();
                photos.dedup();
                ui.add_space(4.);
                for photo in &photos {
                    let from = choice
                        .options
                        .iter()
                        .find(|m| m.photo == *photo)
                        .map(|m| m.from.clone())
                        .unwrap_or_default();
                    row(
                        ui,
                        palette,
                        Row {
                            on: None,
                            thumbnail: thumbnails.get(photo),
                            title: &file_name(&from),
                            detail: &format!("missing from {}", self.place(from.parent())),
                            hover: &from.display().to_string(),
                            chips: self.badges.get(photo).map(Vec::as_slice).unwrap_or(&[]),
                            note: None,
                        },
                    );
                }
                ui.indent(("folder-sync-choice", c), |ui| {
                    ui.spacing_mut().item_spacing.y = 4.;
                    let leave = self.picks[c].is_none();
                    if option(
                        ui,
                        palette,
                        leave,
                        "Decide later",
                        "stays missing; these files are offered again next time",
                        true,
                    )
                    .clicked()
                    {
                        self.picks[c] = None;
                    }
                    for (i, m) in choice.options.iter().enumerate() {
                        let enabled = !(open_master == Some(m.photo) && !m.verified);
                        let same_name = m.to.file_name() == m.from.file_name();
                        let how = match (m.verified, same_name) {
                            (true, true) => "same content and name",
                            (true, false) => "same content, renamed",
                            (false, _) => "probably the same, not verified",
                        };
                        let detail = format!("{how} \u{b7} in {}", self.place(m.to.parent()));
                        let title = if photos.len() > 1 {
                            format!("{} is {}", file_name(&m.to), file_name(&m.from))
                        } else {
                            file_name(&m.to)
                        };
                        if option(
                            ui,
                            palette,
                            self.picks[c] == Some(i),
                            &title,
                            &detail,
                            enabled,
                        )
                        .on_hover_text(m.to.display().to_string())
                        .clicked()
                        {
                            self.picks[c] = Some(i);
                        }
                    }
                });
            }
        });
    }
}

/// The review dialog's width.
const WIDTH: f32 = 660.;

/// The kinds of change the review shows, each a card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Moved,
    Probable,
    New,
    Copies,
    Missing,
    Choice,
}
impl Kind {
    fn title(self, n: usize) -> String {
        match self {
            Self::Moved => format!(
                "Update {} moved or renamed",
                super::widgets::plural(n, "photo", "photos")
            ),
            Self::Probable => format!(
                "{} probably moved",
                super::widgets::plural(n, "photo", "photos")
            ),
            Self::New => format!(
                "Add {}",
                super::widgets::plural(n, "new photo", "new photos")
            ),
            Self::Copies => format!(
                "Add {}",
                super::widgets::plural(
                    n,
                    "copy of a photo already here",
                    "copies of photos already here"
                )
            ),
            Self::Missing => format!(
                "Remove {} from the catalog",
                super::widgets::plural(n, "missing photo", "missing photos")
            ),
            Self::Choice => format!(
                "{} found more than once",
                super::widgets::plural(n, "photo", "photos")
            ),
        }
    }
    /// What the card does, in a line.
    fn explanation(self) -> &'static str {
        match self {
            Self::Moved => {
                "Found elsewhere with exactly the same content. They keep their ratings, keywords, edits and virtual copies."
            }
            Self::Probable => {
                "Look like photos that moved, but no full checksum was on record to prove it. Tick the ones you recognize; their edits open only if the file passes the usual check."
            }
            Self::New => "Added as Add Photo Folder adds them, with their sidecar metadata.",
            Self::Copies => {
                "Exact copies of photos already in the catalog, usually duplicates. Left out unless you tick them."
            }
            Self::Missing => {
                "Not found on disk. Removing takes their edits, ratings and keywords out of the catalog; files elsewhere are not touched."
            }
            Self::Choice => {
                "Each of these photos is missing, and more than one file has exactly its content. Pick the file that is the photo, or leave it to decide later."
            }
        }
    }
    fn icon(self) -> Icon {
        match self {
            Self::Moved | Self::Probable => Icon::Folder,
            Self::New | Self::Copies => Icon::FolderPlus,
            Self::Missing => Icon::Close,
            Self::Choice => Icon::Check,
        }
    }
    fn tint(self, palette: &theme::Palette) -> egui::Color32 {
        match self {
            Self::Moved | Self::New => palette.accent(),
            Self::Probable | Self::Copies => palette.gray(170),
            Self::Missing => palette.danger(),
            Self::Choice => palette.warning(),
        }
    }
}

/// A thin rule across the dialog.
fn separator(ui: &mut egui::Ui, palette: &theme::Palette) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.), egui::Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        egui::Stroke::new(1., palette.gray(45)),
    );
}

/// A card: a rounded panel a little lighter than the dialog.
fn card(ui: &mut egui::Ui, palette: &theme::Palette, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(palette.gray(38))
        .stroke(egui::Stroke::new(1., palette.gray(48)))
        .corner_radius(8.)
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = egui::vec2(8., 6.);
            add(ui);
        });
}

/// The kind's icon in a soft disc of its colour.
fn kind_icon(ui: &mut egui::Ui, palette: &theme::Palette, kind: Kind) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(22., 22.), egui::Sense::hover());
    let tint = kind.tint(palette);
    ui.painter()
        .circle_filled(rect.center(), 11., tint.gamma_multiply(0.18));
    if kind == Kind::Choice {
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "?",
            egui::FontId::proportional(13.),
            tint,
        );
    } else {
        super::icons::paint_at(ui.painter(), kind.icon(), rect.center(), 13., tint);
    }
}

/// A card's header: its box (mixed when only some rows are ticked), icon,
/// title and the explanation, and a Show/Hide toggle for its rows. Returns
/// whether the box was toggled and whether the list was.
fn card_header(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    kind: Kind,
    n: usize,
    all: &mut bool,
    some: bool,
    open: bool,
) -> (bool, bool) {
    let mut toggled = false;
    let mut expand = false;
    card(ui, palette, |ui| {
        ui.horizontal(|ui| {
            if check(ui, palette, *all, some && !*all, true).clicked() {
                *all = !*all;
                toggled = true;
            }
            kind_icon(ui, palette, kind);
            ui.label(
                egui::RichText::new(kind.title(n))
                    .size(13.)
                    .color(palette.gray(230)),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let text = if open { "Hide" } else { "Show" };
                let link = ui.add(
                    egui::Button::new(egui::RichText::new(text).size(12.).color(palette.gray(170)))
                        .frame(false),
                );
                expand = link.clicked();
            });
        });
        ui.horizontal(|ui| {
            ui.add_space(30.);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(kind.explanation())
                        .size(12.)
                        .color(palette.gray(145)),
                )
                .wrap(),
            );
        });
    });
    (toggled, expand)
}

/// A card's rows, indented under it.
fn list(ui: &mut egui::Ui, palette: &theme::Palette, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(palette.gray(30))
        .corner_radius(6.)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .outer_margin(egui::Margin {
            left: 18,
            right: 0,
            top: -4,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing = egui::vec2(8., 2.);
            add(ui);
        });
}

/// One photo or file in a list.
struct Row<'a> {
    /// Its box and whether it can be changed; none for a row without one.
    on: Option<(&'a mut bool, bool)>,
    thumbnail: Option<&'a egui::TextureHandle>,
    title: &'a str,
    detail: &'a str,
    hover: &'a str,
    /// What the photo holds: rating, Edited, Keywords…
    chips: &'a [String],
    /// Why it can't be ticked, or what it is.
    note: Option<&'a str>,
}
fn row(ui: &mut egui::Ui, palette: &theme::Palette, row: Row<'_>) {
    ui.horizontal(|ui| {
        ui.set_min_height(44.);
        if let Some((on, enabled)) = row.on
            && check(ui, palette, *on, false, enabled).clicked()
            && enabled
        {
            *on = !*on;
        }
        let (rect, _) = ui.allocate_exact_size(egui::vec2(52., 36.), egui::Sense::hover());
        match row.thumbnail {
            Some(texture) => {
                let size = texture.size_vec2();
                let fit = (rect.width() / size.x).min(rect.height() / size.y);
                let image = egui::Rect::from_center_size(rect.center(), size * fit);
                ui.painter().image(
                    texture.id(),
                    image,
                    egui::Rect::from_min_max(egui::pos2(0., 0.), egui::pos2(1., 1.)),
                    egui::Color32::WHITE,
                );
            }
            None => {
                ui.painter()
                    .rect_filled(rect.shrink(2.), 4., palette.gray(44));
            }
        }
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.;
            ui.add_space(3.);
            ui.horizontal(|ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(row.title)
                            .size(13.)
                            .color(palette.gray(225)),
                    )
                    .truncate(),
                )
                .on_hover_text(row.hover);
                for chip_text in row.chips {
                    chip(ui, palette, chip_text);
                }
            });
            let mut detail = row.detail.to_string();
            if let Some(note) = row.note {
                if !detail.is_empty() {
                    detail.push_str(" \u{b7} ");
                }
                detail.push_str(note);
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(detail)
                        .size(11.5)
                        .color(palette.gray(140)),
                )
                .truncate(),
            );
        });
    });
}

/// A small rounded label: "★★★", "Edited".
fn chip(ui: &mut egui::Ui, palette: &theme::Palette, text: &str) {
    egui::Frame::new()
        .fill(palette.gray(50))
        .corner_radius(4.)
        .inner_margin(egui::Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(10.5)
                    .color(palette.gray(200)),
            );
        });
}

/// A box drawn as the app draws its own: accent when on, a dash when only
/// some of what it covers is on.
fn check(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    on: bool,
    mixed: bool,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18., 18.), sense);
    let rect = egui::Rect::from_center_size(rect.center(), egui::vec2(16., 16.));
    let painter = ui.painter();
    let filled = (on || mixed) && enabled;
    if filled {
        painter.rect_filled(rect, 4., palette.accent());
    } else {
        let stroke = if response.hovered() {
            palette.gray(150)
        } else {
            palette.gray(95)
        };
        painter.rect_filled(rect, 4., palette.gray(30));
        painter.rect_stroke(
            rect,
            4.,
            egui::Stroke::new(1., stroke),
            egui::StrokeKind::Inside,
        );
    }
    if filled && on {
        super::icons::paint_at(
            painter,
            Icon::Check,
            rect.center(),
            12.,
            palette.on_accent(),
        );
    } else if filled {
        painter.hline(
            rect.shrink(4.).x_range(),
            rect.center().y,
            egui::Stroke::new(2., palette.on_accent()),
        );
    }
    response.on_hover_cursor(if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::NotAllowed
    })
}

/// One answer to a choice, on one line: a round button, the answer, and
/// in grey why it is one.
fn option(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    selected: bool,
    title: &str,
    detail: &str,
    enabled: bool,
) -> egui::Response {
    let fill = if selected {
        palette.accent().gamma_multiply(0.14)
    } else {
        palette.gray(32)
    };
    let stroke = if selected {
        palette.accent().gamma_multiply(0.8)
    } else {
        palette.gray(46)
    };
    let inner = egui::Frame::new()
        .fill(fill)
        .stroke(egui::Stroke::new(1., stroke))
        .corner_radius(6.)
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(16., 16.), egui::Sense::hover());
                let painter = ui.painter();
                painter.circle_stroke(
                    rect.center(),
                    6.5,
                    egui::Stroke::new(
                        1.5,
                        if selected {
                            palette.accent()
                        } else {
                            palette.gray(110)
                        },
                    ),
                );
                if selected {
                    painter.circle_filled(rect.center(), 3.5, palette.accent());
                }
                let color = if enabled {
                    palette.gray(225)
                } else {
                    palette.gray(120)
                };
                ui.label(egui::RichText::new(title).size(12.5).color(color));
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(detail)
                            .size(11.5)
                            .color(palette.gray(135)),
                    )
                    .truncate(),
                );
            });
        });
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    ui.interact(inner.response.rect, ui.id().with((title, detail)), sense)
        .on_hover_cursor(if enabled {
            egui::CursorIcon::PointingHand
        } else {
            egui::CursorIcon::NotAllowed
        })
}

/// The deepest folder holding every file the plan names, for showing
/// paths from it; none when they share no more than the disk.
fn common_folder(plan: &SyncPlan) -> Option<PathBuf> {
    let mut dirs = plan
        .moves
        .iter()
        .chain(plan.choices.iter().flat_map(|c| c.options.iter()))
        .flat_map(|m| [m.from.parent(), m.to.parent()])
        .chain(plan.additions.iter().map(|a| a.file.parent()))
        .chain(plan.missing.iter().map(|m| m.path.parent()))
        .flatten();
    let mut common = dirs.next()?.to_path_buf();
    for dir in dirs {
        while !dir.starts_with(&common) {
            if !common.pop() {
                return None;
            }
        }
    }
    (common.components().count() > 2).then_some(common)
}

/// A file's name.
fn file_name(path: &std::path::Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
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
            root: None,
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
        assert!(losses[0].contains("Keywords"), "{losses:?}");
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

    /// Renders the review of a scenario with every kind of change to
    /// `target/folder-sync-review.png`, for looking at the design.
    #[test]
    #[ignore = "writes an image to look at"]
    fn render_the_review() -> anyhow::Result<()> {
        let (_dir, mut e, trip) = editor()?;
        let ctx = e.context.clone();
        crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
        crate::app::icons::install(&ctx);
        let b = id(&e, "b.dng");
        e.library
            .as_mut()
            .unwrap()
            .session
            .set_ratings(&[(b, 3, 0, String::new())])?;
        // Moved, new, a copy, a missing one and one found twice.
        std::fs::create_dir_all(trip.join("selects"))?;
        std::fs::rename(trip.join("a.dng"), trip.join("selects/keep-01.dng"))?;
        std::fs::write(trip.join("new-1.dng"), bytes(7))?;
        std::fs::write(trip.join("new-2.dng"), bytes(8))?;
        std::fs::copy(
            trip.join("selects/keep-01.dng"),
            trip.join("keep-01 copy.dng"),
        )?;
        std::fs::copy(trip.join("b.dng"), trip.join("selects/b.dng"))?;
        std::fs::rename(trip.join("b.dng"), trip.join("b-renamed.dng"))?;
        std::fs::write(trip.join("c.dng"), bytes(9))?;
        e.start_folder_sync(SyncScope::All, "Sony 7CR".into());
        scanned(&mut e);
        e.poll_folder_sync();
        let review = e.folder_sync.review.as_mut().expect("a review");
        review
            .expanded
            .extend(["verified", "new", "copies", "missing"]);
        let mut textures = Default::default();
        let image = super::super::ui_snapshot::render(
            &ctx,
            &mut textures,
            egui::vec2(1100., 1500.),
            2.,
            4,
            |ui| e.draw(ui),
        );
        let out =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/folder-sync-review.png");
        image.save(&out)?;
        eprintln!("{}", out.display());
        // The other cards, their rows shown.
        let review = e.folder_sync.review.as_mut().expect("a review");
        review.plan.choices.clear();
        review.picks.clear();
        let image = super::super::ui_snapshot::render(
            &ctx,
            &mut textures,
            egui::vec2(1100., 1500.),
            2.,
            4,
            |ui| e.draw(ui),
        );
        image.save(out.with_file_name("folder-sync-review-2.png"))?;
        Ok(())
    }
}
