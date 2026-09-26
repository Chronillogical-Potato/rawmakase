//! File chooser intent, independent of menu order and numeric UI identifiers.
use super::{Editor, worker::Event};
use eframe::egui;

#[derive(Clone, Copy)]
pub(super) enum FileDialog {
    OpenRaw,
    OpenFolder,
    Export,
    MonitorProfile,
    LoadPreset,
    SavePreset,
    CameraProfile,
    LensProfile,
    ImportXmp,
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
        let name = self
            .document
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        std::thread::spawn(move || {
            if matches!(kind, FileDialog::CameraProfile) {
                let event = rfd::FileDialog::new()
                    .add_filter("Lightroom / camera profiles", &["dcp", "xmp"])
                    .pick_files()
                    .map(Event::CameraProfile)
                    .unwrap_or(Event::DialogClosed);
                let _ = tx.send(event);
                ctx.request_repaint();
                return;
            }
            if matches!(kind, FileDialog::LensProfile) {
                let event = rfd::FileDialog::new()
                    .add_filter("Adobe lens profiles", &["lcp"])
                    .pick_files()
                    .map(Event::LensProfiles)
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
                FileDialog::Export => rfd::FileDialog::new()
                    .add_filter("JPEG", &["jpg"])
                    .add_filter("16-bit TIFF", &["tiff"])
                    .set_file_name(format!("{name}-edited.jpg"))
                    .save_file(),
                FileDialog::MonitorProfile => rfd::FileDialog::new()
                    .add_filter("ICC profile", &["icc", "icm"])
                    .pick_file(),
                FileDialog::ImportXmp => rfd::FileDialog::new()
                    .add_filter("XMP preset", &["xmp"])
                    .pick_file(),
                FileDialog::CameraProfile | FileDialog::LensProfile => {
                    unreachable!("Handled by the multi-file chooser")
                }
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
                    FileDialog::Export => Event::ExportPath(p),
                    FileDialog::MonitorProfile => Event::Monitor(p),
                    FileDialog::CameraProfile | FileDialog::LensProfile => unreachable!(),
                    FileDialog::ImportXmp => Event::XmpImport(p),
                    FileDialog::LoadPreset => Event::PresetLoad(p),
                    FileDialog::SavePreset => Event::PresetSave(p),
                })
                .unwrap_or(Event::DialogClosed);
            let _ = tx.send(event);
            ctx.request_repaint();
        });
    }
}
