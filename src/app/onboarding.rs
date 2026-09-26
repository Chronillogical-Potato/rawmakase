//! First-run setup: a catalog, then optional Lightroom profiles and presets.
use super::Editor;
use super::dialogs::{CatalogDialog, FileDialog};
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Camera Raw's shared folder, installed with Lightroom for all users. It
/// holds Adobe's camera profiles and the Adobe looks (Adobe Color…).
fn shared_camera_raw() -> Option<PathBuf> {
    let path = if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/Adobe/CameraRaw")
    } else if cfg!(windows) {
        PathBuf::from("C:\\ProgramData\\Adobe\\CameraRaw")
    } else {
        return None;
    };
    path.is_dir().then_some(path)
}
/// Camera Raw's per-user folder: your presets and third-party profiles.
fn user_camera_raw() -> Option<PathBuf> {
    let path = if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/Adobe/CameraRaw")
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?).join("Adobe\\CameraRaw")
    } else {
        return None;
    };
    path.is_dir().then_some(path)
}
/// `~/…` for display.
fn pretty(path: &Path) -> String {
    let text = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
    }
}
/// Files with one of `extensions` under `dir`, a few folders deep.
fn find_files(dir: &Path, extensions: &[&str]) -> Vec<PathBuf> {
    fn walk(dir: &Path, extensions: &[&str], depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && depth < 6 {
                walk(&path, extensions, depth + 1, out);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.iter().any(|x| e.eq_ignore_ascii_case(x)))
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, extensions, 0, &mut out);
    out.sort();
    out
}

/// What the setup view found on disk, refreshed when it opens or imports.
#[derive(Default)]
pub(super) struct Onboarding {
    pub(super) visible: bool,
    scanned: bool,
    scanned_for: Option<PathBuf>,
    /// Cameras seen in the catalog ("Sony ILCE-7CR"), from one photo per folder.
    cameras: BTreeSet<String>,
    /// Adobe base and Camera Matching profiles for those cameras, then looks.
    adobe_profiles: Vec<PathBuf>,
    /// Your profiles named for those cameras ("Sony ILCE-7M2 Portra 400 SO.dcp"),
    /// or all of them when no camera is known yet.
    user_profiles: Vec<PathBuf>,
    /// Progress of a running profile import: (finished, message).
    importing: Option<Arc<Mutex<(bool, String)>>>,
    user_presets: Vec<PathBuf>,
    message: String,
}
impl Onboarding {
    pub(super) fn new(visible: bool) -> Self {
        Self {
            visible,
            ..Default::default()
        }
    }
    fn scan(&mut self, library: Option<&crate::app::library::Library>) {
        self.cameras.clear();
        if let Some(library) = library {
            let mut folders = BTreeSet::new();
            for photo in &library.photos {
                if folders.len() >= 200 {
                    break;
                }
                if crate::storage::is_raw(&photo.path)
                    && folders.insert(photo.folder)
                    && let Ok(raw) = crate::raw::Raw::open(&photo.path)
                {
                    self.cameras
                        .insert(format!("{} {}", raw.metadata.make, raw.metadata.model));
                }
            }
        }
        self.adobe_profiles.clear();
        if let Some(shared) = shared_camera_raw() {
            let profiles = shared.join("CameraProfiles");
            for camera in &self.cameras {
                let base = profiles
                    .join("Adobe Standard")
                    .join(format!("{camera} Adobe Standard.dcp"));
                if base.is_file() {
                    self.adobe_profiles.push(base);
                }
                self.adobe_profiles
                    .extend(find_files(&profiles.join("Camera").join(camera), &["dcp"]));
            }
            if !self.adobe_profiles.is_empty() {
                // Looks last: they need their base profile in the same import.
                self.adobe_profiles.extend(find_files(
                    &shared.join("Settings/Adobe/Profiles/Adobe Raw"),
                    &["xmp"],
                ));
            }
        }
        let user = user_camera_raw();
        // Third-party packs ship a DCP per camera model, often thousands in all;
        // only those for the catalog's cameras are useful. Without a catalog,
        // take them all.
        let cameras: Vec<String> = self.cameras.iter().map(|c| format!("{c} ")).collect();
        self.user_profiles = user
            .as_ref()
            .map(|d| find_files(&d.join("CameraProfiles"), &["dcp"]))
            .unwrap_or_default()
            .into_iter()
            .filter(|p| {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                cameras.is_empty() || cameras.iter().any(|c| name.starts_with(c.as_str()))
            })
            .collect();
        self.user_presets = user
            .as_ref()
            .map(|d| find_files(&d.join("Settings"), &["xmp"]))
            .unwrap_or_default();
        self.scanned = true;
    }
}

/// Imports what it can and skips the rest: unreadable or embed-prohibited DCPs,
/// duplicate names, and names already imported with different contents. The
/// library importer is all-or-nothing and takes at most 1024 files per batch.
fn import_leniently(paths: &[PathBuf], progress: &Mutex<(bool, String)>) -> (usize, usize) {
    let destination = crate::storage::data_dir().join("camera-profiles");
    let mut names = BTreeSet::new();
    let (mut dcps, mut looks, mut skipped) = (Vec::new(), Vec::new(), 0);
    for (i, path) in paths.iter().enumerate() {
        if i % 50 == 0 {
            progress.lock().unwrap().1 = format!("Checking profiles… {i} of {}", paths.len());
        }
        let Some(name) = path.file_name() else {
            skipped += 1;
            continue;
        };
        let Ok(bytes) = std::fs::read(path) else {
            skipped += 1;
            continue;
        };
        let target = destination.join(name);
        let clash = target.exists() && std::fs::read(&target).ok().as_ref() != Some(&bytes);
        if clash || !names.insert(name.to_owned()) {
            skipped += 1;
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
        {
            looks.push(path.clone());
        } else if crate::camera_profiles::from_bytes(&bytes).is_ok() {
            dcps.push(path.clone());
        } else {
            skipped += 1;
        }
    }
    let mut imported = 0;
    // Looks last: they need their base profile already in the library.
    for batch in dcps.chunks(1000).chain(std::iter::once(&looks[..])) {
        if batch.is_empty() {
            continue;
        }
        progress.lock().unwrap().1 = format!("Importing profiles… {imported} done");
        match crate::camera_profiles::import_files(batch) {
            Ok(done) => imported += done.len(),
            Err(_) => skipped += batch.len(),
        }
    }
    (imported, skipped)
}
impl Editor {
    pub(super) fn onboarding_ui(&mut self, ui: &mut egui::Ui) {
        // Rescan when opened and whenever a different catalog is loaded.
        let catalog = self.library.as_ref().map(|l| l.catalog.path.clone());
        if !self.onboarding.scanned || self.onboarding.scanned_for != catalog {
            let library = self.library.as_deref();
            self.onboarding.scan(library);
            self.onboarding.scanned_for = catalog;
        }
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::from_gray(24)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let width = ui.available_width().min(600.);
                        let margin = ((ui.available_width() - width) / 2.).max(20.);
                        ui.add_space(48.);
                        ui.horizontal(|ui| {
                            ui.add_space(margin);
                            ui.vertical(|ui| {
                                ui.set_width(width);
                                self.onboarding_steps(ui, &ctx);
                            });
                        });
                        ui.add_space(48.);
                    });
            });
    }

    fn onboarding_steps(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.spacing_mut().item_spacing = Vec2::new(8., 0.);
        text(ui, "Set up RAWmakase", 24., 240);
        ui.add_space(6.);
        text(
            ui,
            "Pick a catalog, then bring over your Lightroom look. Steps 2 and 3 are optional.",
            13.,
            150,
        );
        ui.add_space(24.);

        let busy = self.activity.is_busy();
        let catalog = self.library.as_ref().map(|l| {
            format!(
                "{} · {} photos",
                l.catalog
                    .path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy(),
                l.photos.len()
            )
        });
        let has_catalog = catalog.is_some();
        step(ui, 1, "Catalog", catalog.as_deref(), |ui| {
            body(
                ui,
                "Import your Lightroom Classic catalog to keep folders, ratings, flags, \
                 labels, keywords and edits. Your .lrcat is only read, and photos stay \
                 where they are.",
            );
            ui.add_space(12.);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if primary(ui, "Import Lightroom catalog…").clicked() {
                        self.catalog_dialog(CatalogDialog::ImportLightroom, ctx);
                    }
                    if secondary(ui, "New empty catalog…").clicked() {
                        self.catalog_dialog(CatalogDialog::Create, ctx);
                    }
                    if secondary(ui, "Open RAWmakase catalog…").clicked() {
                        self.catalog_dialog(CatalogDialog::Open, ctx);
                    }
                });
            });
        });

        // Profiles before presets: many presets name a profile.
        let adobe = self.onboarding.adobe_profiles.len();
        let user_profiles = self.onboarding.user_profiles.len();
        let cameras = self
            .onboarding
            .cameras
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        step(ui, 2, "Camera profiles", None, |ui| {
            body(
                ui,
                "Profiles set the starting look, such as Adobe Color. Lightroom keeps \
                 Adobe's profiles in a shared folder for all users, and profiles you \
                 added in your own folder.",
            );
            ui.add_space(10.);
            if let Some(shared) = shared_camera_raw() {
                location(ui, "Adobe", &pretty(&shared.join("CameraProfiles")));
            }
            if let Some(user) = user_camera_raw() {
                location(ui, "Yours", &pretty(&user.join("CameraProfiles")));
            }
            ui.add_space(8.);
            hint(
                ui,
                &if self.onboarding.cameras.is_empty() {
                    format!(
                        "Found {user_profiles} of your profiles. Choose a catalog to also \
                         find Adobe's profiles and narrow yours to your cameras."
                    )
                } else if adobe + user_profiles == 0 {
                    format!("No profiles found for {cameras}.")
                } else {
                    format!(
                        "Found {adobe} Adobe and {user_profiles} of your profiles for {cameras}"
                    )
                },
            );
            ui.add_space(10.);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy && self.onboarding.importing.is_none(), |ui| {
                    if adobe + user_profiles > 0
                        && primary(ui, &format!("Import {} profiles", adobe + user_profiles))
                            .clicked()
                    {
                        let mut paths = self.onboarding.user_profiles.clone();
                        paths.extend(self.onboarding.adobe_profiles.iter().cloned());
                        let progress = Arc::new(Mutex::new((false, String::new())));
                        self.onboarding.importing = Some(progress.clone());
                        let ctx = ctx.clone();
                        std::thread::spawn(move || {
                            let (imported, skipped) = import_leniently(&paths, &progress);
                            *progress.lock().unwrap() = (
                                true,
                                if skipped > 0 {
                                    format!(
                                        "Imported {imported} profiles; skipped {skipped} that \
                                         are unreadable, don't allow reuse or clash by name."
                                    )
                                } else {
                                    format!("Imported {imported} profiles")
                                },
                            );
                            ctx.request_repaint();
                        });
                    }
                    if secondary(ui, "Choose files…").clicked() {
                        self.dialog(FileDialog::CameraProfile, ctx);
                    }
                });
            });
        });

        let presets = self.presets.library.presets.len();
        let found = self.onboarding.user_presets.len();
        step(ui, 3, "Presets", None, |ui| {
            body(
                ui,
                "Your Lightroom develop presets are .xmp files. Presets that need \
                 something RAWmakase lacks still appear and apply what they can.",
            );
            ui.add_space(10.);
            if let Some(user) = user_camera_raw() {
                location(ui, "Yours", &pretty(&user.join("Settings")));
            }
            if presets > 0 {
                ui.add_space(8.);
                hint(ui, &format!("{presets} presets already in RAWmakase."));
            }
            ui.add_space(10.);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    if found > 0 && primary(ui, "Import your presets").clicked() {
                        let (mut ok, mut skipped) = (0, 0);
                        for path in &self.onboarding.user_presets {
                            match crate::presets::import_file(path) {
                                Ok(_) => ok += 1,
                                Err(_) => skipped += 1,
                            }
                        }
                        self.onboarding.message = if skipped > 0 {
                            format!(
                                "Imported {ok} presets; skipped {skipped} files that aren't \
                                 presets or clash with an installed one."
                            )
                        } else {
                            format!("Imported {ok} presets")
                        };
                        self.reload_presets(ctx);
                    }
                    if secondary(ui, "Choose a file…").clicked() {
                        self.dialog(FileDialog::ImportXmp, ctx);
                    }
                });
            });
        });

        step(ui, 4, "Good to know", None, |ui| {
            for line in [
                "Masks, healing and lens profiles aren't rendered yet; they stay in the catalog.",
                "Export presets, watermarks and plug-ins don't carry over.",
                "Calibrated display? Set it in Develop under Settings › Monitor Profile.",
            ] {
                body(ui, line);
                ui.add_space(4.);
            }
        });

        if let Some(progress) = self.onboarding.importing.clone() {
            let (done, message) = progress.lock().unwrap().clone();
            self.onboarding.message = message;
            if done {
                self.onboarding.importing = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
        if !self.onboarding.message.is_empty() {
            let message = self.onboarding.message.clone();
            text(ui, &message, 12., 200);
            ui.add_space(14.);
        }
        ui.add_space(6.);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    has_catalog,
                    egui::Button::new(egui::RichText::new("Start").size(14.).color(Color32::WHITE))
                        .fill(Color32::from_rgb(62, 88, 115))
                        .min_size(Vec2::new(120., 34.)),
                )
                .on_disabled_hover_text("Choose a catalog first")
                .clicked()
            {
                self.finish_onboarding(true);
            }
            ui.add_space(8.);
            if ui
                .add(egui::Button::new(egui::RichText::new("Skip for now").size(13.)).frame(false))
                .on_hover_text("Reopen it any time from the catalog menu › Setup assistant")
                .clicked()
            {
                self.finish_onboarding(has_catalog);
            }
        });
    }

    /// Hides the view; `done` records that setup is complete so it stays hidden.
    fn finish_onboarding(&mut self, done: bool) {
        self.onboarding.visible = false;
        self.onboarding_done = done;
        self.library_mode = self.library.is_some();
        let _ = self.save_session();
    }
    pub(super) fn open_onboarding(&mut self) {
        self.onboarding.visible = true;
        self.onboarding.scanned = false;
        self.onboarding.message.clear();
    }
}

/// A numbered card. When `done` is set it shows that summary and a check
/// instead of the number, but keeps its actions so the choice can change.
fn step(
    ui: &mut egui::Ui,
    number: usize,
    title: &str,
    done: Option<&str>,
    contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Frame::new()
        .fill(Color32::from_gray(32))
        .stroke(Stroke::new(1., Color32::from_gray(44)))
        .corner_radius(8.)
        .inner_margin(egui::Margin::symmetric(20, 18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(22.), Sense::hover());
                let c = rect.center();
                if done.is_some() {
                    ui.painter()
                        .circle_filled(c, 11., Color32::from_rgb(64, 132, 90));
                    ui.painter().add(egui::Shape::line(
                        vec![
                            c + Vec2::new(-4.5, 0.),
                            c + Vec2::new(-1.5, 3.),
                            c + Vec2::new(4.5, -3.),
                        ],
                        Stroke::new(1.8, Color32::WHITE),
                    ));
                } else {
                    ui.painter().circle_filled(c, 11., Color32::from_gray(52));
                    ui.painter().text(
                        c,
                        egui::Align2::CENTER_CENTER,
                        number.to_string(),
                        egui::FontId::proportional(12.),
                        Color32::from_gray(210),
                    );
                }
                ui.add_space(10.);
                text(ui, title, 16., 235);
                if let Some(done) = done {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        text(ui, done, 12., 150);
                    });
                }
            });
            ui.add_space(10.);
            ui.horizontal(|ui| {
                ui.add_space(32.);
                ui.vertical(contents);
            });
        });
    ui.add_space(12.);
}
fn text(ui: &mut egui::Ui, value: &str, size: f32, gray: u8) {
    ui.label(
        egui::RichText::new(value)
            .size(size)
            .color(Color32::from_gray(gray)),
    );
}
fn body(ui: &mut egui::Ui, value: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(value)
                .size(13.)
                .line_height(Some(19.))
                .color(Color32::from_gray(170)),
        )
        .wrap(),
    );
}
fn hint(ui: &mut egui::Ui, value: &str) {
    text(ui, value, 12., 135);
}
/// A labelled folder path in a quiet monospace chip.
fn location(ui: &mut egui::Ui, label: &str, path: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(48., 22.), Sense::hover());
        ui.painter().text(
            rect.left_center(),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(11.),
            Color32::from_gray(125),
        );
        egui::Frame::new()
            .fill(Color32::from_gray(24))
            .corner_radius(4.)
            .inner_margin(egui::Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(path)
                            .monospace()
                            .size(11.)
                            .color(Color32::from_gray(180)),
                    )
                    .truncate(),
                );
            });
    });
    ui.add_space(4.);
}
fn primary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).size(13.).color(Color32::WHITE))
            .fill(Color32::from_rgb(62, 88, 115))
            .min_size(Vec2::new(0., 28.)),
    )
}
fn secondary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(egui::RichText::new(label).size(13.)).min_size(Vec2::new(0., 28.)))
}
