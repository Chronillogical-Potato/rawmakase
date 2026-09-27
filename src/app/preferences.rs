//! Application preferences, apart from the photo's develop settings. Laid out
//! like Lightroom's Preferences and Catalog Settings: tabs on the left, one
//! fixed-size page on the right so switching tabs never moves the window.
use super::Editor;
use super::dialogs::{CatalogDialog, FileDialog};
use super::widgets::{form_row, modal_frame, pretty_path, primary_button};
use crate::app::theme;
use crate::raw::Demosaic;
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum Tab {
    #[default]
    General,
    Catalog,
    Profiles,
    Performance,
    Display,
}
impl Tab {
    const ALL: [Tab; 5] = [
        Tab::General,
        Tab::Catalog,
        Tab::Profiles,
        Tab::Performance,
        Tab::Display,
    ];
    fn title(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Catalog => "Catalog",
            Tab::Profiles => "Profiles & Presets",
            Tab::Performance => "Performance",
            Tab::Display => "Display",
        }
    }
}

/// Disk usage shown on the pages, measured when the window opens, when a tab
/// is chosen and after an import or a purge, never every frame.
#[derive(Default)]
struct Usage {
    camera_profiles: usize,
    lens_profiles: usize,
    decode_cache: (usize, u64),
    previews: u64,
    catalog: Option<u64>,
    folders: Option<usize>,
}

#[derive(Default)]
pub(super) struct Preferences {
    pub(super) open: bool,
    tab: Tab,
    usage: Usage,
    /// Measured with a file dialog open, so its import is counted on close.
    stale: bool,
    /// The status line when the window opened; only later messages are shown.
    status_at_open: String,
}

const WIDTH: f32 = 780.;
const HEIGHT: f32 = 520.;
const SIDEBAR: f32 = 196.;

fn camera_profiles_dir() -> PathBuf {
    crate::storage::data_dir().join("camera-profiles")
}
fn lens_profiles_dir() -> PathBuf {
    crate::storage::data_dir().join("lens-profiles")
}
fn presets_dir() -> PathBuf {
    crate::storage::data_dir().join("xmp-presets")
}
fn decode_cache_dir() -> PathBuf {
    crate::decode_cache::cache_dir().join("decoded")
}

/// Files under `dir` (with one of `extensions`, if any) and their total size.
fn files(dir: &Path, extensions: &[&str]) -> (usize, u64) {
    let mut out = (0, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else if extensions.is_empty()
                || path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| extensions.iter().any(|x| e.eq_ignore_ascii_case(x)))
            {
                out.0 += 1;
                out.1 += meta.len();
            }
        }
    }
    out
}
fn bytes(n: u64) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    const MB: f64 = (1u64 << 20) as f64;
    let n = n as f64;
    if n >= GB {
        format!("{:.1} GB", n / GB)
    } else if n >= MB {
        format!("{:.0} MB", n / MB)
    } else if n > 0. {
        format!("{:.0} KB", (n / 1024.).max(1.))
    } else {
        "Empty".into()
    }
}
fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl Editor {
    pub(super) fn open_preferences(&mut self, tab: Tab) {
        self.preferences.open = true;
        self.preferences.tab = tab;
        self.preferences.status_at_open = self.status.clone();
        self.measure_usage();
    }
    fn measure_usage(&mut self) {
        let catalog = self.library.as_ref().map(|l| &l.catalog);
        self.preferences.usage = Usage {
            camera_profiles: files(&camera_profiles_dir(), &["dcp", "xmp"]).0,
            lens_profiles: files(&lens_profiles_dir(), &["lcp"]).0,
            decode_cache: files(&decode_cache_dir(), &["decoded"]),
            previews: std::fs::metadata(crate::catalog::preview_cache::PreviewCache::path())
                .map_or(0, |m| m.len()),
            catalog: catalog.and_then(|c| std::fs::metadata(&c.path).ok().map(|m| m.len())),
            folders: catalog.and_then(|c| c.folders().ok().map(|f| f.len())),
        };
        self.preferences.stale = self.activity.is_dialog();
    }
    /// ⌘, (Ctrl+, elsewhere) opens Preferences, as in Lightroom.
    pub(super) fn preferences_shortcut(&mut self, ctx: &egui::Context) {
        if !self.preferences.open
            && !self.activity.is_busy()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma))
        {
            self.open_preferences(Tab::General);
        }
    }
    pub(super) fn preferences_window(&mut self, ctx: &egui::Context) {
        if !self.preferences.open {
            return;
        }
        if self.preferences.stale && !self.activity.is_dialog() {
            self.measure_usage();
        }
        let response = egui::Modal::new(egui::Id::new("preferences"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame())
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(WIDTH, HEIGHT), Sense::hover());
                let sidebar = egui::Rect::from_min_size(rect.min, Vec2::new(SIDEBAR, HEIGHT));
                ui.painter().rect_filled(
                    sidebar,
                    egui::CornerRadius {
                        nw: 10,
                        sw: 10,
                        ne: 0,
                        se: 0,
                    },
                    theme::gray(27),
                );
                ui.painter().vline(
                    sidebar.right(),
                    sidebar.y_range(),
                    Stroke::new(1., theme::gray(45)),
                );
                let mut side = ui.new_child(
                    egui::UiBuilder::new().max_rect(sidebar.shrink2(Vec2::new(12., 20.))),
                );
                self.preferences_tabs(&mut side);
                let page = egui::Rect::from_min_max(
                    egui::pos2(sidebar.right() + 32., rect.top() + 26.),
                    egui::pos2(rect.right() - 32., rect.bottom() - 72.),
                );
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(page));
                content.set_clip_rect(page.expand(4.));
                content.spacing_mut().item_spacing = Vec2::new(8., 10.);
                content.spacing_mut().button_padding = Vec2::new(12., 5.);
                content.spacing_mut().interact_size.y = 28.;
                content.label(
                    egui::RichText::new(self.preferences.tab.title())
                        .size(18.)
                        .color(theme::gray(236)),
                );
                content.add_space(10.);
                match self.preferences.tab {
                    Tab::General => self.general_page(&mut content),
                    Tab::Catalog => self.catalog_page(&mut content),
                    Tab::Profiles => self.profiles_page(&mut content),
                    Tab::Performance => self.performance_page(&mut content),
                    Tab::Display => self.display_page(&mut content),
                }
                let footer = egui::Rect::from_min_max(
                    egui::pos2(sidebar.right() + 32., rect.bottom() - 56.),
                    egui::pos2(rect.right() - 24., rect.bottom() - 16.),
                );
                ui.painter().hline(
                    (sidebar.right() + 1.)..=rect.right(),
                    rect.bottom() - 64.,
                    Stroke::new(1., theme::gray(45)),
                );
                let mut bar = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(footer)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                bar.spacing_mut().button_padding = Vec2::new(18., 6.);
                let done = primary_button(&mut bar, "Done").clicked();
                // Only what happened while the window was open, e.g. an import.
                if self.status != self.preferences.status_at_open {
                    bar.add_space(16.);
                    bar.add(
                        egui::Label::new(
                            egui::RichText::new(&self.status)
                                .size(12.)
                                .color(theme::gray(160)),
                        )
                        .truncate(),
                    );
                }
                done
            });
        if response.inner || response.should_close() {
            self.preferences.open = false;
        }
    }
    fn preferences_tabs(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 2.;
        ui.label(
            egui::RichText::new("Preferences")
                .size(11.)
                .color(theme::gray(130)),
        );
        ui.add_space(8.);
        for tab in Tab::ALL {
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.), Sense::click());
            let selected = self.preferences.tab == tab;
            if selected {
                ui.painter().rect_filled(rect, 5., theme::accent());
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 5., theme::gray(40));
            }
            ui.painter().text(
                rect.left_center() + Vec2::new(12., 0.),
                egui::Align2::LEFT_CENTER,
                tab.title(),
                egui::FontId::proportional(13.),
                if selected {
                    theme::on_accent_text(250)
                } else {
                    theme::gray(205)
                },
            );
            if response.clicked() && !selected {
                self.preferences.tab = tab;
                self.measure_usage();
            }
        }
    }

    fn general_page(&mut self, ui: &mut egui::Ui) {
        group(ui, "About");
        form_row(ui, "RAWmakase", |ui| {
            value(ui, crate::updates::config().current_version);
            ui.add_space(8.);
            self.update_status(ui);
        });
        form_row(ui, "LibRaw", |ui| {
            value(ui, &crate::raw::version());
        });
        form_row(ui, "Updates", |ui| self.automatic_updates_checkbox(ui));
        gap(ui);
        group(ui, "Locations");
        let data = crate::storage::data_dir();
        form_row(ui, "App data", |ui| path_value(ui, &data));
        form_row(ui, "", |ui| reveal_button(ui, &data));
        gap(ui);
        group(ui, "Help");
        form_row(ui, "", |ui| {
            if ui.button("Keyboard Shortcuts").clicked() {
                self.view.shortcuts = true;
                self.preferences.open = false;
            }
            if ui.button("Setup Assistant…").clicked() {
                self.preferences.open = false;
                self.open_onboarding();
            }
        });
    }

    fn catalog_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let usage = &self.preferences.usage;
        if let Some(library) = &self.library {
            let path = library.catalog.path.clone();
            group(ui, "Current catalog");
            form_row(ui, "Name", |ui| {
                value(ui, &path.file_stem().unwrap_or_default().to_string_lossy());
            });
            form_row(ui, "Location", |ui| path_value(ui, &path));
            form_row(ui, "", |ui| reveal_button(ui, &path));
            form_row(ui, "Photos", |ui| {
                value(ui, &library.photos.len().to_string());
            });
            form_row(ui, "Folders", |ui| {
                value(ui, &usage.folders.map_or("–".into(), |n| n.to_string()));
            });
            form_row(ui, "Catalog size", |ui| {
                value(ui, &usage.catalog.map_or("–".into(), bytes));
            });
        } else {
            group(ui, "Current catalog");
            form_row(ui, "", |ui| {
                value(ui, "No catalog is open.");
            });
        }
        form_row(ui, "Previews", |ui| {
            value(ui, &bytes(usage.previews));
        });
        gap(ui);
        group(ui, "Catalogs");
        form_row(ui, "", |ui| {
            for (kind, label) in [
                (CatalogDialog::Open, "Open…"),
                (CatalogDialog::Create, "New…"),
                (CatalogDialog::ImportLightroom, "Import Lightroom Catalog…"),
            ] {
                if ui.button(label).clicked() {
                    self.catalog_dialog(kind, &ctx);
                }
            }
        });
    }

    fn profiles_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let usage = &self.preferences.usage;
        let (cameras, lenses) = (usage.camera_profiles, usage.lens_profiles);
        let presets = self.presets.library.presets.len();
        let mut chosen = None;
        group(ui, "Camera profiles");
        form_row(ui, "Imported", |ui| {
            value(ui, &plural(cameras, "profile", "profiles"));
        });
        form_row(ui, "", |ui| {
            if ui.button("Import Profiles…").clicked() {
                chosen = Some(FileDialog::CameraProfile);
            }
            reveal_button(ui, &camera_profiles_dir());
        });
        gap(ui);
        group(ui, "Lens profiles");
        form_row(ui, "Imported", |ui| {
            value(ui, &plural(lenses, "profile", "profiles"));
        });
        form_row(ui, "", |ui| {
            if ui.button("Import Lens Profiles…").clicked() {
                chosen = Some(FileDialog::LensProfile);
            }
            reveal_button(ui, &lens_profiles_dir());
        });
        gap(ui);
        group(ui, "Develop presets");
        form_row(ui, "Installed", |ui| {
            value(ui, &plural(presets, "preset", "presets"));
        });
        form_row(ui, "", |ui| {
            if ui.button("Import Preset…").clicked() {
                chosen = Some(FileDialog::ImportXmp);
            }
            reveal_button(ui, &presets_dir());
        });
        gap(ui);
        form_row(ui, "", |ui| {
            hint(
                ui,
                "The Setup Assistant finds Lightroom's own profiles and presets on this Mac.",
            );
        });
        if let Some(kind) = chosen {
            self.dialog(kind, &ctx);
            self.preferences.stale = true;
        }
    }

    fn performance_page(&mut self, ui: &mut egui::Ui) {
        group(ui, "Demosaic");
        let current = crate::raw::demosaic();
        let mut picked = None;
        for (choice, label, note) in [
            (
                Demosaic::Rawmakase,
                "RAWmakase",
                "Faster, with equal or better detail.",
            ),
            (
                Demosaic::Libraw,
                "LibRaw",
                "AHD for Bayer sensors, Markesteijn for X-Trans.",
            ),
        ] {
            form_row(
                ui,
                if choice == Demosaic::Rawmakase {
                    "Engine"
                } else {
                    ""
                },
                |ui| {
                    if ui.radio(current == choice, label).clicked() && current != choice {
                        picked = Some(choice);
                    }
                    hint(ui, note);
                },
            );
        }
        if let Some(choice) = picked {
            crate::raw::set_demosaic(choice);
            let _ = self.save_session();
            // Takes effect on the next full-size decode, so the open photo is
            // reopened.
            if let Some(path) = self.document.path.clone() {
                let photo = self.document.catalog_photo;
                self.open_raw(path, photo);
            }
        }
        gap(ui);
        group(ui, "Decoded photo cache");
        let (entries, size) = self.preferences.usage.decode_cache;
        let dir = decode_cache_dir();
        form_row(ui, "Location", |ui| path_value(ui, &dir));
        form_row(ui, "Size", |ui| {
            value(
                ui,
                &format!(
                    "{} of 4 GB · {}",
                    bytes(size),
                    plural(entries, "photo", "photos")
                ),
            );
        });
        let mut purge = false;
        form_row(ui, "", |ui| {
            purge = ui
                .add_enabled(entries > 0, egui::Button::new("Purge Cache"))
                .clicked();
            reveal_button(ui, &dir);
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Reopening a photo skips decoding while it is cached. Purging only removes decoded copies, never photos or edits.",
            );
        });
        if purge {
            let removed = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "decoded"))
                .filter(|e| std::fs::remove_file(e.path()).is_ok())
                .count();
            self.status = format!(
                "Purged {} from the cache",
                plural(removed, "photo", "photos")
            );
            self.measure_usage();
        }
    }

    fn display_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        group(ui, "Interface");
        let choices = self.themes.choices();
        let name = |file: &Option<String>| match file {
            None => "RAWmakase".to_string(),
            Some(file) => choices.iter().find(|(f, _)| f == file).map_or_else(
                || fastframe_theme::display_name(file).to_string(),
                |(_, n)| n.clone(),
            ),
        };
        let mut chosen = self.themes.selected.clone();
        form_row(ui, "Theme", |ui| {
            egui::ComboBox::from_id_salt("interface-theme")
                .width(220.)
                .selected_text(name(&chosen))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut chosen, None, "RAWmakase");
                    for (file, label) in &choices {
                        ui.selectable_value(&mut chosen, Some(file.clone()), label);
                    }
                });
            reveal_button(ui, &super::theme::Themes::folder());
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                &self.themes.status().unwrap_or_else(|| {
                    "Palette files in the themes folder are listed here. On Omarchy the desktop's theme is followed. The photo's backdrop stays neutral grey.".into()
                }),
            );
        });
        if chosen != self.themes.selected {
            self.themes.selected = chosen;
            let _ = self.save_session();
        }
        gap(ui);
        group(ui, "Monitor profile");
        let monitor = self.view.monitor.clone();
        form_row(ui, "Profile", |ui| match &monitor {
            Some(path) => {
                value(ui, &path.file_name().unwrap_or_default().to_string_lossy());
            }
            None => {
                value(ui, "sRGB");
            }
        });
        let mut choose = false;
        let mut srgb = false;
        form_row(ui, "", |ui| {
            choose = ui.button("Choose Profile…").clicked();
            srgb = ui
                .add_enabled(monitor.is_some(), egui::Button::new("Use sRGB"))
                .clicked();
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Photos are shown in this profile. Choose your display's calibrated ICC profile, or keep sRGB.",
            );
        });
        if choose {
            self.dialog(FileDialog::MonitorProfile, &ctx);
        }
        if srgb {
            self.view.monitor = None;
            let _ = self.save_session();
            self.schedule();
        }
    }
}

fn group(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(12.)
            .strong()
            .color(theme::gray(175)),
    );
}
fn gap(ui: &mut egui::Ui) {
    ui.add_space(14.);
}
fn value(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).color(theme::gray(225))).truncate());
}
fn path_value(ui: &mut egui::Ui, path: &Path) {
    ui.add(
        egui::Label::new(egui::RichText::new(pretty_path(path)).color(theme::gray(225))).truncate(),
    )
    .on_hover_text(path.display().to_string());
}
fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).size(12.).color(theme::gray(135))).wrap());
}
fn reveal_button(ui: &mut egui::Ui, path: &Path) {
    let exists = path.exists();
    if ui
        .add_enabled(exists, egui::Button::new(crate::platform::reveal::LABEL))
        .clicked()
    {
        let _ = crate::platform::reveal::reveal(path);
    }
}
/// The workspace bar's Preferences button: a gear in a 28 px slot.
pub(super) fn gear_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 4., theme::gray(38));
    }
    let color = theme::gray(if response.hovered() { 235 } else { 175 });
    super::icons::paint_at(
        ui.painter(),
        super::icons::Icon::Settings,
        rect.center(),
        18.,
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
