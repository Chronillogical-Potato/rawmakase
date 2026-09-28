use super::Editor;
use super::dialogs::{CatalogDialog, FolderAction};
use super::worker::Event;
use eframe::egui;
use std::path::PathBuf;

impl Editor {
    pub(super) fn load_catalog(&mut self, path: PathBuf, ctx: &egui::Context) {
        if self.activity.is_busy() || !self.flush() {
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
        if self.activity.is_busy() || !self.flush() {
            return;
        }
        if !self.activity.begin_dialog() {
            return;
        }
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let current = self.library.as_ref().map(|l| l.catalog.path.clone());
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<Option<PathBuf>> {
                Ok(match kind {
                    CatalogDialog::Create => {
                        let Some(path) = rfd::FileDialog::new()
                            .add_filter("RAWmakase catalog", &["rawmakase"])
                            .set_file_name("Photos.rawmakase")
                            .save_file()
                        else {
                            return Ok(None);
                        };
                        crate::catalog::Catalog::create(&path)?;
                        Some(path)
                    }
                    CatalogDialog::Open => rfd::FileDialog::new()
                        .add_filter("RAWmakase catalog", &["rawmakase"])
                        .pick_file(),
                    CatalogDialog::ImportLightroom => {
                        let Some(source) = rfd::FileDialog::new()
                            .add_filter("Lightroom catalog", &["lrcat"])
                            .pick_file()
                        else {
                            return Ok(None);
                        };
                        let Some(destination) = rfd::FileDialog::new()
                            .add_filter("RAWmakase catalog", &["rawmakase"])
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
                                cat.add_folder(&path)?;
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
                                let available=l.available_count();
                                l.message=format!("Folder relinked. {available} of {} photos are available.",l.photos.len());
                                if available==0 {l.message.push_str(" No files matched this location; check that the selected folder contains the expected subfolders.");}
                            }
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
        if self.activity.is_busy() || !self.flush() || !self.activity.begin_dialog() {
            return;
        }
        self.pending_photo = Some((path, true));
        self.status = format!("Adding {name}'s folder to the Library…");
        let tx = self.tx.clone();
        let ctx = self.context.clone();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<_> {
                crate::catalog::Catalog::open(&current)?.add_folder(&folder)?;
                crate::app::library::Library::load(&current, ctx.clone())
            })()
            .map(Box::new)
            .map_err(|e| format!("{e:#}"));
            let _ = tx.send(Event::CatalogReady(result));
            ctx.request_repaint();
        });
    }
    /// The catalog photo stored at `path`, if any.
    fn catalog_photo_at(&self, path: &std::path::Path) -> Option<i64> {
        self.library
            .as_ref()?
            .photos
            .iter()
            .find(|p| p.path == path)
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
        if !p.path.is_file() {
            self.status =
                "Photo is offline. Use Locate root folder or right-click its folder to relink it."
                    .into();
            return;
        }
        if !crate::storage::is_raw(&p.path) {
            self.status = format!(
                "{} can be browsed in Library; Develop opens camera RAW files.",
                p.format
            );
            return;
        }
        if self.activity.is_busy() || !self.flush() {
            return;
        }
        if let Some(l) = &mut self.library {
            l.selected = Some(id)
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
                self.document.lightroom_notice = format!("Lightroom settings not applied: {e:#}")
            }
        }
    }
}
