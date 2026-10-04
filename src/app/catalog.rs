use super::Editor;
use super::dialogs::{CatalogDialog, FolderAction};
use super::widgets::confirm_modal;
use super::worker::Event;
use eframe::egui;
use std::path::PathBuf;

impl Editor {
    /// Whether no work in progress stops a catalog change and the open edit
    /// is saved; it is saved only when nothing is in progress.
    fn ready_for_catalog(&mut self) -> bool {
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
        std::thread::spawn(move || {
            let result = crate::app::library::Library::load(&path, ctx.clone())
                .map(Box::new)
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::CatalogReady(result));
            ctx.request_repaint();
        });
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
        std::thread::spawn(move || {
            // Sidecars of the folder added that could not be read.
            let mut report = crate::catalog::SidecarReport::default();
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
                        let Some(path) = rfd::FileDialog::new()
                            .set_title(if matches!(action, FolderAction::Add) {
                                "Add photo folder"
                            } else {
                                "Select replacement folder"
                            })
                            .pick_folder()
                        else {
                            return Ok(None);
                        };
                        let current =
                            current.ok_or_else(|| anyhow::anyhow!("Open a catalog first"))?;
                        let mut cat = crate::catalog::Catalog::open(&current)?;
                        match action {
                            FolderAction::Add => {
                                report = cat
                                    .add_folder_with(
                                        &path,
                                        &crate::catalog::MetadataDefaults::load(),
                                    )?
                                    .1;
                            }
                            FolderAction::RelinkRoot(id) => cat.relink_root(id, &path)?,
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
                Ok(Some(path)) => Event::CatalogReady(
                    crate::app::library::Library::load(&path, ctx.clone())
                        .map(|mut l| {
                            if matches!(kind, CatalogDialog::Folder(FolderAction::RelinkRoot(_) | FolderAction::RelinkFolder(_))) {
                                l.wait_for_availability();
                                let available=l.available_count();
                                l.message=format!("Folder relinked. {available} of {} photos are available.",l.photos.len());
                                if available==0 {l.message.push_str(" No files matched this location; check that the selected folder contains the expected subfolders.");}
                            }
                            folder_added(&mut l, &report);
                            Box::new(l)
                        })
                        .map_err(|e| format!("{e:#}")),
                ),
                Ok(None) => Event::DialogClosed,
                Err(e) => Event::CatalogReady(Err(format!("{e:#}"))),
            };
            let _ = tx.send(event);
            ctx.request_repaint();
        });
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
                self.pending_photo = Some((path, false));
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
        self.pending_photo = Some((path, true));
        self.status = format!("Adding {name}'s folder to the Library…");
        let tx = self.tx.clone();
        let ctx = self.context.clone();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<_> {
                let (_, report) = crate::catalog::Catalog::open(&current)?
                    .add_folder_with(&folder, &crate::catalog::MetadataDefaults::load())?;
                let mut library = crate::app::library::Library::load(&current, ctx.clone())?;
                folder_added(&mut library, &report);
                Ok(library)
            })()
            .map(Box::new)
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::CatalogReady(result));
            ctx.request_repaint();
        });
    }
    /// The catalog photo stored at `path`, if any: its master rather than
    /// a virtual copy.
    pub(super) fn catalog_photo_at(&self, path: &std::path::Path) -> Option<i64> {
        self.library
            .as_ref()?
            .photos
            .iter()
            .filter(|p| p.path == path)
            .min_by_key(|p| p.master.is_some())
            .map(|p| p.id)
    }
    /// Opens the photo waiting to be added once the catalog is ready, adding
    /// its folder first if that has not happened yet.
    pub(super) fn open_pending_photo(&mut self) {
        let Some((path, added)) = self.pending_photo.take() else {
            return;
        };
        if let Some(id) = self.catalog_photo_at(&path) {
            self.develop_catalog_photo(id);
        } else if !added {
            self.add_to_library(path);
        } else {
            self.status = format!(
                "{} could not be added to the Library",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
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
    pub(super) fn apply_lightroom_edits(&mut self) {
        let (Some(l), Some(id), Some(m)) = (
            &self.library,
            self.document.catalog_photo,
            &self.document.metadata,
        ) else {
            return;
        };
        let result = (|| -> anyhow::Result<_> {
            let text = l
                .catalog
                .lightroom_develop(id)?
                .ok_or_else(|| anyhow::anyhow!("No Lightroom Develop settings"))?;
            crate::catalog::lightroom::convert_develop(
                &text,
                m,
                &self.document.profiles,
                self.document.full().map(|image| image.as_ref()),
            )
        })();
        match result {
            Ok((r, warnings)) => {
                self.document.recipe = r;
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
                self.document.recipe =
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
/// Reports the sidecars a folder added could not read, if any, on the
/// Library's status line.
fn folder_added(
    library: &mut crate::app::library::Library,
    report: &crate::catalog::SidecarReport,
) {
    if let Some(summary) = report.summary() {
        library.set_message_with_detail(format!("Folder added · {summary}"), report.details());
    }
}
