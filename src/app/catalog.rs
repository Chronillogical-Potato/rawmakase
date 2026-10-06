use super::Editor;
use super::dialogs::{CatalogDialog, FolderAction};
use super::folder_locations::{FolderQuestion, folder_added, reopened};
use super::widgets::confirm_modal;
use super::worker::Event;
use eframe::egui;
use std::path::PathBuf;

impl Editor {
    /// Whether no work in progress stops a catalog change and the open edit
    /// is saved; it is saved only when nothing is in progress.
    pub(super) fn ready_for_catalog(&mut self) -> bool {
        !self.activity.is_busy() && self.flush()
    }
    pub(super) fn load_catalog(&mut self, path: PathBuf, ctx: &egui::Context) {
        if !self.ready_for_catalog() {
            return;
        }
        if !self.activity.begin_dialog() {
            return;
        }
        self.status = "Opening catalog…".into();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        super::task::spawn(
            tx,
            ctx.clone(),
            move |tx| {
                let result = crate::app::library::Library::load(&path, ctx.clone())
                    .map(Box::new)
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::CatalogReady(result));
            },
            |tx, error| {
                let _ = tx.send(Event::CatalogReady(Err(error)));
            },
        );
    }
    pub(super) fn catalog_dialog(&mut self, kind: CatalogDialog, ctx: &egui::Context) {
        if !self.ready_for_catalog() {
            return;
        }
        if !self.activity.begin_dialog() {
            return;
        }
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let current = self.library.as_ref().map(|l| l.catalog.path.clone());
        super::task::spawn(
            tx,
            ctx.clone(),
            move |tx| {
                // Sidecars of the folder added that could not be read, and
                // folders it skipped.
                let mut report = crate::catalog::SidecarReport::default();
                let mut conflicts = Vec::new();
                let result = (|| -> anyhow::Result<Option<PathBuf>> {
                    Ok(match kind {
                        CatalogDialog::Create => {
                            let Some(path) = catalog_file_dialog()
                                .set_file_name("Photos.rawmakase")
                                .save_file()
                            else {
                                return Ok(None);
                            };
                            crate::catalog::Catalog::create(&path)?;
                            Some(path)
                        }
                        CatalogDialog::Open => catalog_file_dialog().pick_file(),
                        CatalogDialog::ImportLightroom => {
                            let Some(source) = rfd::FileDialog::new()
                                .add_filter("Lightroom catalog", &["lrcat"])
                                .pick_file()
                            else {
                                return Ok(None);
                            };
                            let Some(destination) = catalog_file_dialog()
                                .set_file_name(format!(
                                    "{}.rawmakase",
                                    source.file_stem().unwrap_or_default().to_string_lossy()
                                ))
                                .save_file()
                            else {
                                return Ok(None);
                            };
                            let _ = tx.send(Event::CatalogWorking(format!(
                                "Importing {}…",
                                source.file_name().unwrap_or_default().to_string_lossy()
                            )));
                            ctx.request_repaint();
                            Some(crate::catalog::lightroom::import_lightroom(
                                &source,
                                &destination,
                            )?)
                        }
                        CatalogDialog::Folder(action) => {
                            crate::platform::network::prepare_filesystem_bridge();
                            let current =
                                current.ok_or_else(|| anyhow::anyhow!("Open a catalog first"))?;
                            let mut cat = crate::catalog::Catalog::open(&current)?;
                            let mut dialog = rfd::FileDialog::new().set_title(match action {
                                FolderAction::Add => "Add photo folder",
                                _ => "Find missing folder on this computer",
                            });
                            // Where another computer has it, when that is here too.
                            if let Some(there) = suggestion(&cat, action) {
                                dialog = dialog.set_directory(there);
                            }
                            let Some(path) = dialog.pick_folder() else {
                                return Ok(None);
                            };
                            let ask = |question| {
                                let _ = tx.send(Event::FolderQuestion(Box::new(question)));
                            };
                            match action {
                                FolderAction::Add => {
                                    let added = cat.import_folder(
                                        &path,
                                        &crate::catalog::MetadataDefaults::load(),
                                        &[],
                                    )?;
                                    if !added.ambiguous.is_empty() {
                                        ask(FolderQuestion::Ambiguous {
                                            catalog: current,
                                            folder: path,
                                            open: added.ambiguous,
                                            chosen: Vec::new(),
                                        });
                                        return Ok(None);
                                    }
                                    report = added.report;
                                    conflicts = added.conflicts;
                                }
                                FolderAction::RelinkRoot(id) => {
                                    let overrides = cat.root_overrides(id)?;
                                    if !overrides.is_empty() {
                                        ask(FolderQuestion::Overrides {
                                            catalog: current,
                                            root: id,
                                            path,
                                            overrides,
                                        });
                                        return Ok(None);
                                    }
                                    cat.relink_root(id, &path)?
                                }
                                FolderAction::RelinkFolder(id) => cat.relink_folder(id, &path)?,
                            }
                            Some(current)
                        }
                    })
                })();
                if let Ok(Some(path)) = &result {
                    let _ = tx.send(Event::CatalogWorking(format!(
                        "Opening {}…",
                        path.file_stem().unwrap_or_default().to_string_lossy()
                    )));
                    ctx.request_repaint();
                }
                let event = match result {
                    Ok(Some(path)) => reopened(
                        &path,
                        matches!(
                            kind,
                            CatalogDialog::Folder(
                                FolderAction::RelinkRoot(_) | FolderAction::RelinkFolder(_)
                            )
                        ),
                        &report,
                        &conflicts,
                        &ctx,
                    ),
                    Ok(None) => Event::DialogClosed,
                    Err(e) => Event::CatalogReady(Err(format!("{e:#}"))),
                };
                let _ = tx.send(event);
            },
            |tx, error| {
                let _ = tx.send(Event::CatalogReady(Err(error)));
            },
        );
    }
    /// Adds a photo from outside the Library (dropped on the window or passed
    /// on the command line) by adding its folder to the catalog, then opens it
    /// in Develop.
    pub(super) fn add_to_library(&mut self, path: PathBuf) {
        let path = path.canonicalize().unwrap_or(path);
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        self.library_mode = true;
        if let Some(id) = self.catalog_photo_at(&path) {
            self.develop_catalog_photo(id);
            return;
        }
        let Some(current) = self.library.as_ref().map(|l| l.catalog.path.clone()) else {
            if self.activity.is_dialog() {
                // The catalog is still opening; add the photo once it is ready.
                self.pending_photo = Some(super::PendingPhoto {
                    path,
                    folder_added: false,
                });
            } else {
                self.status = format!("Open or create a catalog to edit {name}");
            }
            return;
        };
        let Some(folder) = path.parent().map(PathBuf::from) else {
            return;
        };
        if !self.ready_for_catalog() || !self.activity.begin_dialog() {
            return;
        }
        self.pending_photo = Some(super::PendingPhoto {
            path,
            folder_added: true,
        });
        self.status = format!("Adding {name}'s folder to the Library…");
        let tx = self.tx.clone();
        let ctx = self.context.clone();
        super::task::spawn(
            tx,
            ctx.clone(),
            move |tx| {
                let result = (|| -> anyhow::Result<_> {
                    let added = crate::catalog::Catalog::open(&current)?.import_folder(
                        &folder,
                        &crate::catalog::MetadataDefaults::load(),
                        &[],
                    )?;
                    anyhow::ensure!(
                        added.ambiguous.is_empty(),
                        "{} matches more than one folder of the catalog; add it with Add Folder…",
                        folder.display()
                    );
                    let mut library = crate::app::library::Library::load(&current, ctx.clone())?;
                    // Says why the photo wasn't added when its folder is linked elsewhere.
                    folder_added(&mut library, &added.report, &added.conflicts);
                    Ok(library)
                })()
                .map(Box::new)
                .map_err(|e| format!("{e:#}"));
                let _ = tx.send(Event::CatalogReady(result));
            },
            |tx, error| {
                let _ = tx.send(Event::CatalogReady(Err(error)));
            },
        );
    }
    /// The catalog photo stored at `path`, if any: its master rather than
    /// a virtual copy.
    pub(super) fn catalog_photo_at(&self, path: &std::path::Path) -> Option<i64> {
        self.library
            .as_ref()?
            .photos
            .iter()
            // A location stored through a symlink spells the path differently;
            // only photos of the same name are looked up on disk.
            .filter(|p| {
                p.path == path
                    || (path.file_name() == Some(std::ffi::OsStr::new(&p.filename))
                        && p.path.canonicalize().is_ok_and(|real| real == path))
            })
            .min_by_key(|p| p.master.is_some())
            .map(|p| p.id)
    }
    /// Opens the photo waiting to be added once the catalog is ready, adding
    /// its folder first if that has not happened yet.
    pub(super) fn open_pending_photo(&mut self) {
        let Some(super::PendingPhoto { path, folder_added }) = self.pending_photo.take() else {
            return;
        };
        if let Some(id) = self.catalog_photo_at(&path) {
            self.develop_catalog_photo(id);
        } else if !folder_added {
            self.add_to_library(path);
        } else {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            // Adding its folder may have said why (linked elsewhere).
            self.status = match self.library.as_ref().map(|l| l.message.as_str()) {
                Some(why) if !why.is_empty() => {
                    format!("{name} could not be added to the Library · {why}")
                }
                _ => format!("{name} could not be added to the Library"),
            };
        }
    }
    pub(super) fn develop_catalog_photo(&mut self, id: i64) {
        let Some(p) = self.library.as_ref().and_then(|l| l.photo(id)).cloned() else {
            return;
        };
        let exists = p.path.is_file();
        if exists && let Some(l) = &mut self.library {
            l.found(&p.path);
        }
        if let Some(refusal) = crate::app::library::develop_refusal(&p, exists) {
            // Said in a dialog: in the status bar alone, it looks as if the
            // click did nothing.
            let reason = refusal.detail();
            self.status = reason.clone();
            self.not_editable =
                Some((format!("{} can't be opened in Develop", p.filename), reason));
            return;
        }
        // Already open, e.g. in the Loupe: Develop shows it as it is.
        if self.document.catalog_photo == Some(id)
            && (self.document.full().is_some() || self.load.is_running())
        {
            if let Some(l) = &mut self.library {
                l.make_active(id)
            }
            // Zoomed in meanwhile (the zoom is shared): Crop cannot stay open.
            if self.view.zoom.on && self.view.is(super::state::Tool::Crop) {
                self.view.tool = super::state::Tool::None;
            }
            self.library_mode = false;
            return;
        }
        if !self.ready_for_catalog() {
            return;
        }
        if let Some(l) = &mut self.library {
            l.make_active(id)
        }
        self.open_raw(p.path, Some(id));
    }
    /// Starts the open photo from its Lightroom settings `text`, converted as every
    /// photo's Lightroom edit is (`crate::edits`).
    pub(super) fn apply_lightroom_edits(&mut self, text: &str) {
        let Some(m) = &self.document.metadata else {
            return;
        };
        let result = crate::edits::lightroom_edit(text, m, &self.document.profiles);
        match result {
            Ok((r, warnings)) => {
                self.document.edit.recipe = r;
                // Short for the status bar; the full list shows on hover.
                self.document.lightroom_notice = if warnings.is_empty() {
                    "Lightroom edit applied".into()
                } else {
                    format!(
                        "Lightroom edit applied · {} settings not rendered yet\n\n{}",
                        warnings.len(),
                        warnings.join("\n")
                    )
                };
            }
            Err(e) => {
                // Lightroom's edit starts from Adobe Default, not the raw defaults.
                self.document.edit.recipe =
                    crate::develop::Recipe::with_profiles(m, &self.document.profiles);
                self.document.lightroom_notice = format!("Lightroom settings not applied: {e:#}")
            }
        }
    }
}
impl Editor {
    /// Carries out a virtual copy command after saving the open edit, so a
    /// new copy starts from what is on screen. In Develop a new copy opens.
    pub(super) fn virtual_copy(&mut self, action: crate::app::library::CopyAction) {
        use crate::app::library::CopyAction;
        let result = match action {
            CopyAction::Remove(id) => {
                self.remove_copy = Some(id);
                return;
            }
            _ if !self.ready_for_catalog() => return,
            CopyAction::Create(id) => self
                .library
                .as_mut()
                .map(|l| l.create_virtual_copy(id).map(Some)),
            CopyAction::SetMaster(id) => self
                .library
                .as_mut()
                .map(|l| l.set_copy_as_master(id).map(|()| None)),
        };
        let (Some(result), Some(library)) = (result, &self.library) else {
            return;
        };
        match result {
            Ok(open) => {
                self.status = library.message.clone();
                if let Some(id) = open
                    && !self.library_mode
                {
                    self.develop_catalog_photo(id);
                }
            }
            Err(e) => self.status = format!("Virtual copy failed: {e:#}"),
        }
    }
    /// Asks before removing a virtual copy, as Lightroom does; a copy shown
    /// in Develop gives way to its master.
    pub(super) fn remove_copy_window(&mut self, ctx: &egui::Context) {
        let Some(id) = self.remove_copy else {
            return;
        };
        let Some(photo) = self.library.as_ref().and_then(|l| l.photo(id)).cloned() else {
            self.remove_copy = None;
            return;
        };
        // Only a click or the focused button confirms: a stray Return must
        // never remove a copy.
        let Some(remove) = confirm_modal(
            ctx,
            "remove-virtual-copy",
            &format!("Remove “{}” of {}?", photo.copy_name, photo.filename),
            "Its edit, rating and keywords are removed from the catalog. \
             The photo file and its other copies are not affected.",
            false,
            &[("Cancel", false), ("Remove", true)],
            false,
        ) else {
            return;
        };
        self.remove_copy = None;
        if remove {
            self.remove_virtual_copy(id);
        }
    }
    /// Says why Develop could not open a photo.
    pub(super) fn not_editable_window(&mut self, ctx: &egui::Context) {
        let Some((title, reason)) = &self.not_editable else {
            return;
        };
        if confirm_modal(ctx, "not-editable", title, reason, false, &[("OK", ())], ()).is_some() {
            self.not_editable = None;
        }
    }
    /// Asks before Read Metadata from Files, as Lightroom does: it replaces
    /// the catalog's values, edits included.
    pub(super) fn read_metadata_window(&mut self, ctx: &egui::Context) {
        let Some(ids) = self.read_metadata.clone() else {
            return;
        };
        let n = ids.len();
        let title = if n == 1 {
            "Read metadata from the file?".to_string()
        } else {
            format!("Read metadata from {n} files?")
        };
        let Some(read) = confirm_modal(
            ctx,
            "read-metadata",
            &title,
            "Replaces title, caption, keywords and other metadata in the \
             catalog with the values in the files, including your edits. \
             Fields the files don't have are kept. Virtual copies are not read.",
            false,
            &[("Cancel", false), ("Read", true)],
            false,
        ) else {
            return;
        };
        self.read_metadata = None;
        if !read || !self.ready_for_catalog() {
            return;
        }
        let Some(library) = &mut self.library else {
            return;
        };
        library.read_metadata_from_files(&ids).ok();
        self.status = library.message.clone();
    }
    /// Removes virtual copy `id` once confirmed.
    pub(super) fn remove_virtual_copy(&mut self, id: i64) {
        if !self.ready_for_catalog() {
            return;
        }
        let Some(library) = &mut self.library else {
            return;
        };
        match library.remove_virtual_copy(id) {
            Ok(master) => {
                self.status = library.message.clone();
                self.undo_log.forget_photo(id);
                // The reference, when it was this copy, goes with it.
                self.load_reference();
                if self.document.catalog_photo == Some(id) {
                    // Removed from Develop: show its master there instead.
                    if let Some(master) = master
                        && !self.library_mode
                    {
                        self.develop_catalog_photo(master);
                    }
                    // Nothing may save into the removed copy.
                    if self.document.catalog_photo == Some(id) {
                        self.document.reset(None);
                        self.preview.clear_document();
                        self.library_mode = true;
                    }
                }
            }
            Err(e) => self.status = format!("Virtual copy not removed: {e:#}"),
        }
    }
}
/// A file dialog for RAWmakase catalogs.
fn catalog_file_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new().add_filter("RAWmakase catalog", &["rawmakase"])
}
/// Where another computer has the root or folder of `action`, when that
/// path is on this computer too: the place Find Missing Folder starts.
fn suggestion(catalog: &crate::catalog::Catalog, action: FolderAction) -> Option<PathBuf> {
    let (root, relative) = match action {
        FolderAction::Add => return None,
        FolderAction::RelinkRoot(id) => (id, String::new()),
        FolderAction::RelinkFolder(id) => {
            let folder = catalog.folders().ok()?.into_iter().find(|f| f.id == id)?;
            (folder.root, folder.relative)
        }
    };
    let locations = catalog
        .folder_locations()
        .ok()?
        .into_iter()
        .find(|r| r.root == root)?;
    // Each other computer's own rows, so a folder found there through its
    // root or a parent is suggested too.
    let mut by_computer: std::collections::BTreeMap<&str, Vec<(String, PathBuf)>> =
        Default::default();
    for (computer, at, path) in &locations.elsewhere {
        by_computer
            .entry(computer)
            .or_default()
            .push((at.clone(), path.clone()));
    }
    by_computer.values().find_map(|rows| {
        crate::catalog::locations::resolve_in(&locations.original, rows, &relative, cfg!(windows))
            .filter(|path| path.is_dir())
    })
}
