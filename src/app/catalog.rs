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
                self.document.lightroom_notice = if warnings.is_empty() {
                    "Compatible Lightroom settings applied; rendering uses RAWmakase's pipeline."
                        .into()
                } else {
                    format!(
                        "Compatible settings applied. Preserved but not rendered: {}",
                        warnings.join("; ")
                    )
                };
            }
            Err(e) => {
                self.document.lightroom_notice = format!("Lightroom settings not applied: {e:#}")
            }
        }
    }
}
