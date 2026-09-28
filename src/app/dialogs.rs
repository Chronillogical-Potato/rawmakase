//! File chooser intent, independent of menu order and numeric UI identifiers.
use super::{Editor, bulk_import::ImportKind, worker::Event};
use eframe::egui;

#[derive(Clone, Copy)]
pub(super) enum FileDialog {
    OpenRaw,
    OpenFolder,
    MonitorProfile,
    LoadPreset,
    SavePreset,
    CameraProfile,
    LensProfile,
    ImportXmp,
    /// A folder to scan for profiles or presets, subfolders included.
    ImportFolder(ImportKind),
}

#[derive(Clone, Copy)]
pub(super) enum CatalogDialog {
    Create,
    Open,
    ImportLightroom,
    Folder(FolderAction),
}

#[derive(Clone, Copy)]
pub(super) enum FolderAction {
    Add,
    RelinkRoot(i64),
    RelinkFolder(i64),
}

impl Editor {
    pub(super) fn dialog(&mut self, kind: FileDialog, ctx: &egui::Context) {
        if !self.activity.begin_dialog() {
            return;
        }
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let import = match kind {
                FileDialog::CameraProfile => Some(ImportKind::CameraProfiles),
                FileDialog::LensProfile => Some(ImportKind::LensProfiles),
                FileDialog::ImportXmp => Some(ImportKind::Presets),
                _ => None,
            };
            if let Some(import) = import {
                let (name, extensions) = import.filter();
                let event = rfd::FileDialog::new()
                    .add_filter(name, extensions)
                    .pick_files()
                    .map(|paths| Event::Import(import, paths))
                    .unwrap_or(Event::DialogClosed);
                let _ = tx.send(event);
                ctx.request_repaint();
                return;
            }
            if let FileDialog::ImportFolder(import) = kind {
                let event = rfd::FileDialog::new()
                    .pick_folder()
                    .map(|folder| Event::Import(import, vec![folder]))
                    .unwrap_or(Event::DialogClosed);
                let _ = tx.send(event);
                ctx.request_repaint();
                return;
            }
            let selected = match kind {
                FileDialog::OpenRaw => rfd::FileDialog::new()
                    .add_filter("Camera RAW", &crate::storage::RAW_EXTENSIONS)
                    .pick_file(),
                FileDialog::OpenFolder => rfd::FileDialog::new().pick_folder(),
                FileDialog::MonitorProfile => rfd::FileDialog::new()
                    .add_filter("ICC profile", &["icc", "icm"])
                    .pick_file(),
                FileDialog::CameraProfile
                | FileDialog::LensProfile
                | FileDialog::ImportXmp
                | FileDialog::ImportFolder(_) => unreachable!("Handled by the import choosers"),
                FileDialog::LoadPreset => rfd::FileDialog::new()
                    .add_filter("RAWmakase preset", &["json"])
                    .pick_file(),
                FileDialog::SavePreset => rfd::FileDialog::new()
                    .set_file_name("preset.json")
                    .save_file(),
            };
            let event = selected
                .map(|p| match kind {
                    FileDialog::OpenRaw | FileDialog::OpenFolder => Event::Open(p),
                    FileDialog::MonitorProfile => Event::Monitor(p),
                    FileDialog::CameraProfile
                    | FileDialog::LensProfile
                    | FileDialog::ImportXmp
                    | FileDialog::ImportFolder(_) => unreachable!(),
                    FileDialog::LoadPreset => Event::PresetLoad(p),
                    FileDialog::SavePreset => Event::PresetSave(p),
                })
                .unwrap_or(Event::DialogClosed);
            let _ = tx.send(event);
            ctx.request_repaint();
        });
    }
}
