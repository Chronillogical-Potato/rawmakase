//! First-run setup: a catalog, then optional Lightroom profiles and presets.
use super::Editor;
use super::bulk_import::{ImportKind, Summary, find_files};
use super::dialogs::{CatalogDialog, FileDialog};
use super::task::Task;
use super::widgets::pretty_path;
use super::worker::Event;
use crate::app::theme;
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

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
/// What the setup view shows and the scan it runs when it opens.
#[derive(Default)]
pub(super) struct Onboarding {
    pub(super) visible: bool,
    scanned: bool,
    scanned_for: Option<PathBuf>,
    /// How many times the catalog had saved photo info read from files when
    /// it last scanned; a later save may add cameras.
    info_saves: u64,
    /// The scan under way; it walks Camera Raw's folders, thousands of files,
    /// so it runs off the UI thread and arrives as [`Event::OnboardingScanned`].
    scan: Task,
    found: Found,
    /// The last profile or preset import, shown under the steps.
    pub(super) last_import: Option<Box<Summary>>,
}
/// What the setup view found on disk, refreshed when it opens.
#[derive(Default)]
pub struct Found {
    /// The catalog's cameras as Adobe names them ("Sony ILCE-7CR").
    cameras: BTreeSet<String>,
    /// Adobe base and Camera Matching profiles for those cameras, then looks.
    adobe_profiles: Vec<PathBuf>,
    /// Your profiles named for those cameras ("Sony ILCE-7M2 Portra 400 SO.dcp"),
    /// or all of them when no camera is known yet.
    user_profiles: Vec<PathBuf>,
    /// Adobe lens profiles for the catalog's camera makers and common
    /// third-party lens brands, plus your own lens profiles.
    lens_profiles: Vec<PathBuf>,
    /// Models the catalog records that Adobe has no profiles for, to match
    /// your own profiles by.
    models: Vec<String>,
    /// Makers of the catalog's cameras, e.g. "Sony".
    makers: BTreeSet<String>,
    user_presets: Vec<PathBuf>,
}
impl Onboarding {
    pub(super) fn new(visible: bool) -> Self {
        Self {
            visible,
            ..Default::default()
        }
    }
    fn scanning(&self) -> bool {
        self.scan.is_running()
    }
}
/// The full name `model` goes by among `names`, which spell out the maker:
/// Lightroom records "ILCE-7M2" where Adobe's profiles say "Sony ILCE-7M2", but
/// "LEICA M10" in both.
fn full_name<'a>(model: &str, names: &'a [String]) -> Option<&'a String> {
    let model = model.to_lowercase();
    if let Some(name) = names.iter().find(|name| name.to_lowercase() == model) {
        return Some(name);
    }
    // By model alone only when one maker has it: "M10" is a Leica and a Canon.
    let suffix = format!(" {model}");
    let mut makers = names
        .iter()
        .filter(|name| name.to_lowercase().ends_with(&suffix));
    let name = makers.next()?;
    makers.all(|other| other == name).then_some(name)
}
/// Whether any of `names` ends in `model`, as "Canon EOS M10" ends in "M10".
fn shares_model(model: &str, names: &[String]) -> bool {
    let suffix = format!(" {}", model.to_lowercase());
    names.iter().any(|n| n.to_lowercase().ends_with(&suffix))
}
/// The cameras Adobe has profiles for, as its folders and files name them.
fn adobe_cameras(profiles: &std::path::Path) -> Vec<String> {
    let names = |dir: PathBuf| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    let mut cameras = names(profiles.join("Camera"));
    cameras.extend(
        names(profiles.join("Adobe Standard"))
            .into_iter()
            .filter_map(|n| n.strip_suffix(" Adobe Standard.dcp").map(str::to_string)),
    );
    cameras
}
impl Found {
    /// What there is for `models`, the cameras the catalog records.
    fn scan(models: &[String], cancel: &AtomicBool) -> Self {
        let mut found = Self::default();
        let shared = shared_camera_raw();
        let known = shared
            .as_ref()
            .map(|s| adobe_cameras(&s.join("CameraProfiles")))
            .unwrap_or_default();
        for model in models {
            match full_name(model, &known) {
                Some(name) => {
                    found.cameras.insert(name.clone());
                }
                // Without Adobe's profiles to name the maker, the model is all
                // there is.
                None if shared.is_none() => {
                    found.cameras.insert(model.clone());
                }
                // A camera Adobe lacks may still have profiles of yours; one
                // several makers share ("M10") is left out rather than guessed.
                None if !shares_model(model, &known) => found.models.push(model.clone()),
                None => {}
            }
        }
        found.find_on_disk(cancel);
        found
    }
    /// Camera Raw's profiles and presets, narrowed to the cameras found.
    /// Stops early once `cancel` is set, as the result is then dropped.
    fn find_on_disk(&mut self, cancel: &AtomicBool) {
        let cancelled = || cancel.load(Ordering::Relaxed);
        if let Some(shared) = shared_camera_raw() {
            let profiles = shared.join("CameraProfiles");
            for camera in &self.cameras {
                let base = profiles
                    .join("Adobe Standard")
                    .join(format!("{camera} Adobe Standard.dcp"));
                if base.is_file() {
                    self.adobe_profiles.push(base);
                }
                self.adobe_profiles.extend(find_files(
                    &profiles.join("Camera").join(camera),
                    &["dcp"],
                    &[],
                ));
            }
            if !self.adobe_profiles.is_empty() {
                // Looks last: they need their base profile in the same import.
                self.adobe_profiles.extend(find_files(
                    &shared.join("Settings/Adobe/Profiles/Adobe Raw"),
                    &["xmp"],
                    &[],
                ));
            }
        }
        if cancelled() {
            return;
        }
        self.makers = self
            .cameras
            .iter()
            .filter_map(|c| c.split_whitespace().next())
            .map(str::to_string)
            .collect();
        if let Some(shared) = shared_camera_raw()
            && !self.makers.is_empty()
            && let Ok(entries) = std::fs::read_dir(shared.join("LensProfiles/1.0"))
        {
            // Adobe groups lens profiles by maker; the camera maker's own
            // lenses plus the usual third-party brands cover most kits
            // without importing thousands of profiles.
            const THIRD_PARTY: [&str; 12] = [
                "Sigma",
                "Tamron",
                "Samyang",
                "Rokinon",
                "Zeiss",
                "Tokina",
                "Viltrox",
                "Voigtlander",
                "Laowa",
                "TTArtisan",
                "7Artisans",
                "Sirui",
            ];
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if self.makers.iter().any(|m| m.to_lowercase() == name)
                    || THIRD_PARTY.iter().any(|b| b.to_lowercase() == name)
                {
                    self.lens_profiles
                        .extend(find_files(&entry.path(), &["lcp"], &[]));
                }
            }
        }
        if cancelled() {
            return;
        }
        let user = user_camera_raw();
        if let Some(user) = &user {
            self.lens_profiles
                .extend(find_files(&user.join("LensProfiles"), &["lcp"], &[]));
        }
        // Third-party packs ship a DCP per camera model, often thousands in all;
        // only those for the catalog's cameras are useful. Without a catalog,
        // take them all.
        let cameras: Vec<String> = self.cameras.iter().chain(&self.models).cloned().collect();
        self.user_profiles = user
            .as_ref()
            .map(|d| find_files(&d.join("CameraProfiles"), &["dcp"], &[]))
            .unwrap_or_default()
            .into_iter()
            .filter(|p| {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                cameras.is_empty() || cameras.iter().any(|c| names_camera(&name, c))
            })
            .collect();
        self.user_presets = user
            .as_ref()
            .map(|d| find_files(&d.join("Settings"), &["xmp"], &["Defaults", "GPU"]))
            .unwrap_or_default();
    }
}

impl Editor {
    pub(super) fn onboarding_ui(&mut self, ui: &mut egui::Ui) {
        // Rescan when opened and whenever a different catalog is loaded.
        let catalog = self.library.as_ref().map(|l| l.catalog.path.clone());
        // And once the catalog has read cameras from new photos' files, when
        // the reader is done rather than at each of its saves.
        let read = self.library.as_ref().is_some_and(|l| {
            !l.reading_photo_info() && l.photo_info_saves() != self.onboarding.info_saves
        }) && !self.onboarding.scanning();
        if !self.onboarding.scanned || self.onboarding.scanned_for != catalog || read {
            self.start_onboarding_scan(catalog);
        }
        let ctx = ui.ctx().clone();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::gray(24)))
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
            "Pick a catalog, then bring over your Lightroom look. Steps 2 to 4 are optional.",
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
            if let Some(work) = &self.catalog_work {
                ui.add_space(10.);
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.).color(theme::gray(170)));
                    hint(
                        ui,
                        &format!("{work} This can take a few minutes for a large catalog."),
                    );
                });
            }
        });

        // Profiles before presets: many presets name a profile.
        let importing = self.importing.is_some();
        let enabled = !busy && !importing;
        let mut chosen = None;
        let scanning = self.onboarding.scanning();
        let adobe = self.onboarding.found.adobe_profiles.len();
        let user_profiles = self.onboarding.found.user_profiles.len();
        let cameras = self
            .onboarding
            .found
            .cameras
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        step(ui, 2, "Camera profiles", None, |ui| {
            body(
                ui,
                "Profiles set the starting look, such as Adobe Color. Choose a folder \
                 and RAWmakase imports the .dcp profiles and .xmp looks in it and its \
                 subfolders.",
            );
            ui.add_space(10.);
            let shared = shared_camera_raw();
            let user = user_camera_raw();
            if let Some(shared) = &shared {
                location(ui, "Adobe", &pretty_path(&shared.join("CameraProfiles")));
            }
            if let Some(user) = &user {
                location(ui, "Yours", &pretty_path(&user.join("CameraProfiles")));
            }
            ui.add_space(8.);
            if scanning && (shared.is_some() || user.is_some()) {
                looking(ui, "Looking for profiles for your cameras…");
            } else {
                hint(
                    ui,
                    &if shared.is_none() && user.is_none() {
                        "Lightroom keeps them in CameraRaw/CameraProfiles, under \
                     /Library/Application Support/Adobe on a Mac and C:\\ProgramData\\Adobe \
                     on Windows. Copy that folder here, or any folder of profiles."
                            .to_string()
                    } else if self.onboarding.found.cameras.is_empty() {
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
            }
            ui.add_space(10.);
            let found = (adobe + user_profiles > 0)
                .then(|| format!("Import {} profiles", adobe + user_profiles));
            if let Some(choice) = import_buttons(ui, found, enabled) {
                chosen = Some((ImportKind::CameraProfiles, choice));
            }
        });

        let presets = self.presets.library.presets.len();
        let found = self.onboarding.found.user_presets.len();
        let lenses = self.onboarding.found.lens_profiles.len();
        let makers = self
            .onboarding
            .found
            .makers
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        step(ui, 3, "Lens profiles", None, |ui| {
            body(
                ui,
                "Lens profiles correct distortion and vignetting, like Lightroom's Enable \
                 Profile Corrections. Choose a folder and RAWmakase imports the .lcp \
                 profiles in it and its subfolders.",
            );
            ui.add_space(10.);
            let shared = shared_camera_raw();
            let user = user_camera_raw();
            if let Some(shared) = &shared {
                location(ui, "Adobe", &pretty_path(&shared.join("LensProfiles")));
            }
            if let Some(user) = &user {
                location(ui, "Yours", &pretty_path(&user.join("LensProfiles")));
            }
            ui.add_space(8.);
            if scanning && (shared.is_some() || user.is_some()) {
                looking(ui, "Looking for lens profiles for your cameras…");
            } else {
                hint(
                    ui,
                    &if shared.is_none() && user.is_none() {
                        "Lightroom keeps them in CameraRaw/LensProfiles. Adobe's cover thousands \
                     of lenses; the folders of your camera maker and lens brands are enough."
                            .to_string()
                    } else if self.onboarding.found.makers.is_empty() {
                        "Choose a catalog to find lens profiles for your cameras.".to_string()
                    } else if lenses == 0 {
                        format!("No lens profiles found for {makers}.")
                    } else {
                        format!("Found {lenses} lens profiles for {makers} and third-party lenses")
                    },
                );
            }
            ui.add_space(10.);
            let found = (lenses > 0).then(|| format!("Import {lenses} lens profiles"));
            if let Some(choice) = import_buttons(ui, found, enabled) {
                chosen = Some((ImportKind::LensProfiles, choice));
            }
        });

        step(ui, 4, "Presets", None, |ui| {
            body(
                ui,
                "Your Lightroom develop presets are .xmp files. Choose a folder and \
                 RAWmakase imports the presets in it and its subfolders, keeping their \
                 groups. Presets that need something RAWmakase lacks still appear and \
                 apply what they can.",
            );
            ui.add_space(10.);
            if let Some(user) = user_camera_raw() {
                location(ui, "Yours", &pretty_path(&user.join("Settings")));
            }
            if presets > 0 {
                ui.add_space(8.);
                hint(ui, &format!("{presets} presets already in RAWmakase."));
            }
            ui.add_space(10.);
            let found = (found > 0).then(|| format!("Import {found} presets"));
            if let Some(choice) = import_buttons(ui, found, enabled) {
                chosen = Some((ImportKind::Presets, choice));
            }
        });
        if let Some((kind, choice)) = chosen {
            self.onboarding.last_import = None;
            match choice {
                Choice::Found => {
                    let paths = match kind {
                        ImportKind::CameraProfiles => {
                            let mut paths = self.onboarding.found.user_profiles.clone();
                            paths.extend(self.onboarding.found.adobe_profiles.iter().cloned());
                            paths
                        }
                        ImportKind::LensProfiles => self.onboarding.found.lens_profiles.clone(),
                        // The folder rather than its files, to skip Defaults and
                        // keep preset folders.
                        ImportKind::Presets => user_camera_raw()
                            .map(|user| vec![user.join("Settings")])
                            .unwrap_or_default(),
                    };
                    self.import(kind, paths, ctx);
                }
                Choice::Folder => self.dialog(FileDialog::ImportFolder(kind), ctx),
                Choice::Files => self.dialog(
                    match kind {
                        ImportKind::CameraProfiles => FileDialog::CameraProfile,
                        ImportKind::LensProfiles => FileDialog::LensProfile,
                        ImportKind::Presets => FileDialog::ImportXmp,
                    },
                    ctx,
                ),
            }
        }

        step(ui, 5, "Good to know", None, |ui| {
            for line in [
                "AI and color-range masks aren't rendered yet; they stay in the catalog.",
                "Export presets, watermarks and plug-ins don't carry over.",
                "Calibrated display? Set it in Develop under Settings › Monitor Profile.",
            ] {
                body(ui, line);
                ui.add_space(4.);
            }
        });

        if importing {
            let status = self.status.clone();
            text(ui, &status, 12., 200);
            ui.add_space(14.);
        } else if let Some(summary) = &self.onboarding.last_import {
            text(ui, &summary.message(), 12., 200);
            if !summary.failed.is_empty() {
                ui.add_space(4.);
                hint(ui, &summary.details(5));
            }
            ui.add_space(14.);
        }
        ui.add_space(6.);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    has_catalog,
                    egui::Button::new(
                        egui::RichText::new("Start")
                            .size(14.)
                            .color(theme::on_accent()),
                    )
                    .fill(theme::accent())
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
    /// Scans in the background; until the result arrives the steps say so.
    fn start_onboarding_scan(&mut self, catalog: Option<PathBuf>) {
        // The catalog already records each photo's camera; no RAW is opened.
        let models = self
            .library
            .as_ref()
            .and_then(|l| l.catalog.cameras().ok())
            .unwrap_or_default();
        self.onboarding.info_saves = self.library.as_ref().map_or(0, |l| l.photo_info_saves());
        let (generation, cancel) = self.onboarding.scan.start();
        self.onboarding.scanned = true;
        self.onboarding.scanned_for = catalog;
        self.onboarding.found = Found::default();
        let tx = self.tx.clone();
        let ctx = self.context.clone();
        std::thread::spawn(move || {
            let found = Found::scan(&models, &cancel);
            if !cancel.load(Ordering::Relaxed) {
                let _ = tx.send(Event::OnboardingScanned {
                    generation,
                    found: Box::new(found),
                });
                ctx.request_repaint();
            }
        });
    }
    /// Takes a scan's result unless a later scan superseded it.
    pub(super) fn onboarding_scanned(&mut self, generation: u64, found: Found) {
        if self.onboarding.scan.is_running() && self.onboarding.scan.id() == generation {
            self.onboarding.scan.finish(generation);
            self.onboarding.found = found;
        }
    }
    pub(super) fn open_onboarding(&mut self) {
        self.onboarding.visible = true;
        self.onboarding.scanned = false;
        self.onboarding.last_import = None;
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
        .fill(theme::gray(32))
        .stroke(Stroke::new(1., theme::gray(44)))
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
                    super::icons::paint_at(
                        ui.painter(),
                        super::icons::Icon::Check,
                        c,
                        14.,
                        theme::on_accent(),
                    );
                } else {
                    ui.painter().circle_filled(c, 11., theme::gray(52));
                    ui.painter().text(
                        c,
                        egui::Align2::CENTER_CENTER,
                        number.to_string(),
                        egui::FontId::proportional(12.),
                        theme::gray(210),
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
            .color(theme::gray(gray)),
    );
}
fn body(ui: &mut egui::Ui, value: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(value)
                .size(13.)
                .line_height(Some(19.))
                .color(theme::gray(170)),
        )
        .wrap(),
    );
}
fn hint(ui: &mut egui::Ui, value: &str) {
    text(ui, value, 12., 135);
}
/// Whether a profile file named `name` is for `camera`, by maker and model
/// ("Sony ILCE-7M2 Portra 400 SO.dcp") or by model alone.
fn names_camera(name: &str, camera: &str) -> bool {
    let (name, camera) = (name.to_lowercase(), camera.to_lowercase());
    name.starts_with(&format!("{camera} ")) || name.contains(&format!(" {camera} "))
}
/// A hint with a spinner, while the scan runs.
fn looking(ui: &mut egui::Ui, value: &str) {
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(12.).color(theme::gray(170)));
        hint(ui, value);
    });
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
            theme::gray(125),
        );
        egui::Frame::new()
            .fill(theme::gray(24))
            .corner_radius(4.)
            .inner_margin(egui::Margin::symmetric(8, 3))
            .show(ui, |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(path)
                            .monospace()
                            .size(11.)
                            .color(theme::gray(180)),
                    )
                    .truncate(),
                );
            });
    });
    ui.add_space(4.);
}
/// A step's import button, as chosen.
enum Choice {
    /// The files the step found.
    Found,
    Folder,
    Files,
}
/// A step's import buttons: the files found, if any, then a folder or files
/// of your choosing. The first one is the primary action.
fn import_buttons(ui: &mut egui::Ui, found: Option<String>, enabled: bool) -> Option<Choice> {
    let mut choice = None;
    ui.horizontal(|ui| {
        ui.add_enabled_ui(enabled, |ui| {
            if let Some(label) = &found
                && primary(ui, label).clicked()
            {
                choice = Some(Choice::Found);
            }
            let folder = if found.is_some() {
                secondary(ui, "Choose folder…")
            } else {
                primary(ui, "Choose folder…")
            };
            if folder
                .on_hover_text("Imports every file in the folder and its subfolders")
                .clicked()
            {
                choice = Some(Choice::Folder);
            }
            if secondary(ui, "Choose files…").clicked() {
                choice = Some(Choice::Files);
            }
        });
    });
    choice
}
fn primary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(13.)
                .color(theme::on_accent()),
        )
        .fill(theme::accent())
        .min_size(Vec2::new(0., 28.)),
    )
}
fn secondary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(egui::RichText::new(label).size(13.)).min_size(Vec2::new(0., 28.)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_the_assistant_scans_in_the_background() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let photos = dir.path().join("photos");
        std::fs::create_dir(&photos)?;
        std::fs::write(photos.join("image.ARW"), b"not a raw")?;
        let catalog = dir.path().join("test.rawmakase");
        let mut c = crate::catalog::Catalog::create(&catalog)?;
        c.add_folder(&photos)?;
        drop(c);
        let ctx = egui::Context::default();
        let library = crate::app::library::Library::load(&catalog, ctx.clone())?;
        let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
        editor.library = Some(Box::new(library));
        editor.open_onboarding();
        let frame = |editor: &mut Editor| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                editor.events(ui.ctx());
                editor.onboarding_ui(ui);
            });
            output.textures_delta.clear();
        };
        // The first frame draws the view and leaves the RAWs to the scan.
        frame(&mut editor);
        assert!(editor.onboarding.scanning());
        let started = std::time::Instant::now();
        while editor.onboarding.scanning() {
            assert!(started.elapsed().as_secs() < 30, "the scan never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
            frame(&mut editor);
        }
        assert_eq!(editor.onboarding.scanned_for.as_deref(), Some(&*catalog));
        assert!(editor.onboarding.found.cameras.is_empty());

        // A scan superseded by a later one is ignored.
        editor.start_onboarding_scan(Some(catalog.clone()));
        let stale = editor.onboarding.scan.id();
        editor.start_onboarding_scan(Some(catalog));
        let found = Found {
            cameras: BTreeSet::from(["Sony ILCE-7M2".to_string()]),
            ..Default::default()
        };
        editor.onboarding_scanned(stale, found);
        assert!(editor.onboarding.scanning());
        assert!(editor.onboarding.found.cameras.is_empty());
        Ok(())
    }

    #[test]
    fn catalog_models_take_adobe_names() {
        let adobe = [
            "Sony ILCE-7M2",
            "Sony ILCA-77M2",
            "LEICA M10",
            "Canon EOS M10",
        ]
        .map(str::to_string);
        let name = |model| full_name(model, &adobe).map(String::as_str);
        assert_eq!(name("ILCE-7M2"), Some("Sony ILCE-7M2"));
        assert_eq!(name("leica m10"), Some("LEICA M10"));
        assert_eq!(name("M10"), None);
        assert_eq!(name("7M2"), None);
        assert_eq!(name("iPhone 8"), None);
        // "M10" is a Leica and a Canon, so it matches none of your profiles either.
        assert!(shares_model("M10", &adobe));
        assert!(!shares_model("iPhone 8", &adobe));
        assert!(names_camera(
            "Sony ILCE-7M2 Portra 400 SO.dcp",
            "Sony ILCE-7M2"
        ));
        assert!(names_camera("Sony ILCE-7M2 Portra 400 SO.dcp", "ILCE-7M2"));
        assert!(!names_camera("Sony ILCE-7M2 Portra 400 SO.dcp", "7M2"));
        assert!(!names_camera("Sony ILCE-7M2R Portra.dcp", "ILCE-7M2"));
    }
}
