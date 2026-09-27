//! Lightroom's Export dialog and background exports. The dialog's choices are
//! remembered in the data folder, so Export with Previous repeats them without
//! asking. Each export runs on its own thread; the top bar shows its progress
//! while you keep editing.
use super::{Editor, worker::Event};
use crate::export::{Embed, ExportOptions, exif};
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) enum Destination {
    #[default]
    SameFolder,
    Desktop,
    Pictures,
    Folder,
}
impl Destination {
    const ALL: [Self; 4] = [
        Self::SameFolder,
        Self::Desktop,
        Self::Pictures,
        Self::Folder,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::SameFolder => "Same folder as original photo",
            Self::Desktop => "Desktop",
            Self::Pictures => "Pictures",
            Self::Folder => "Specific folder",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) enum Existing {
    #[default]
    Ask,
    Unique,
    Overwrite,
    Skip,
}
impl Existing {
    const ALL: [Self; 4] = [Self::Ask, Self::Unique, Self::Overwrite, Self::Skip];
    fn label(self) -> &'static str {
        match self {
            Self::Ask => "Ask what to do",
            Self::Unique => "Choose a new name for the exported file",
            Self::Overwrite => "Overwrite without warning",
            Self::Skip => "Skip",
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(super) enum Format {
    #[default]
    Jpeg,
    Tiff,
}

/// The dialog's choices, saved as `export.json` in the data folder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(super) struct Settings {
    pub destination: Destination,
    pub folder: Option<PathBuf>,
    pub subfolder: bool,
    pub subfolder_name: String,
    pub existing: Existing,
    pub rename: bool,
    pub custom_text: String,
    pub uppercase: bool,
    pub format: Format,
    pub quality: u8,
    pub resize: bool,
    pub long_edge: u32,
    pub ppi: u32,
    /// Camera, capture settings, lens and dates (EXIF).
    pub capture: bool,
    pub location: bool,
    /// The edit as Camera Raw settings (XMP).
    pub develop: bool,
    /// Rating, color label and keywords (XMP).
    pub descriptive: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            destination: Destination::SameFolder,
            folder: None,
            subfolder: false,
            subfolder_name: "Exported".into(),
            existing: Existing::Ask,
            rename: false,
            custom_text: "edited".into(),
            uppercase: false,
            format: Format::Jpeg,
            quality: 92,
            resize: false,
            long_edge: 2048,
            ppi: 240,
            capture: true,
            location: true,
            develop: true,
            descriptive: true,
        }
    }
}
fn settings_path() -> PathBuf {
    crate::storage::data_dir().join("export.json")
}
fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}
impl Settings {
    fn load() -> Option<Self> {
        let text = std::fs::read_to_string(settings_path()).ok()?;
        serde_json::from_str(&text).ok()
    }
    fn save(&self) {
        let _ = crate::storage::atomic_json(&settings_path(), self);
    }
    fn base_folder(&self, source: &Path) -> Option<PathBuf> {
        match self.destination {
            Destination::SameFolder => source.parent().map(Path::to_path_buf),
            Destination::Desktop => Some(home().join("Desktop")),
            Destination::Pictures => Some(home().join("Pictures")),
            Destination::Folder => self.folder.clone(),
        }
    }
    fn folder_for(&self, source: &Path) -> Option<PathBuf> {
        let base = self.base_folder(source)?;
        let name = self.subfolder_name.trim();
        Some(if self.subfolder && !name.is_empty() {
            base.join(name)
        } else {
            base
        })
    }
    fn file_name(&self, source: &Path) -> String {
        let stem = source.file_stem().unwrap_or_default().to_string_lossy();
        let text = self.custom_text.trim();
        let name = if self.rename && !text.is_empty() {
            format!("{stem}-{text}")
        } else {
            stem.to_string()
        };
        let extension = match self.format {
            Format::Jpeg => "jpg",
            Format::Tiff => "tif",
        };
        let extension = if self.uppercase {
            extension.to_uppercase()
        } else {
            extension.into()
        };
        format!("{name}.{extension}")
    }
    fn target(&self, source: &Path) -> Option<PathBuf> {
        Some(self.folder_for(source)?.join(self.file_name(source)))
    }
}
/// "DSC0001-2.jpg", "DSC0001-3.jpg"… for the first name not taken.
fn unique(path: &Path) -> PathBuf {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let extension = path.extension().unwrap_or_default().to_string_lossy();
    (2..)
        .map(|n| path.with_file_name(format!("{stem}-{n}.{extension}")))
        .find(|p| !p.exists())
        .unwrap_or_else(|| path.to_path_buf())
}
/// The current time in UTC as XMP writes it.
fn now_utc() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Everything an export needs from the open photo, taken when it starts so a
/// question about an existing file can wait while you move on.
#[derive(Clone)]
struct Snapshot {
    image: Arc<crate::raw::CameraImage>,
    source: PathBuf,
    recipe: crate::develop::Recipe,
    rating: i32,
    label: String,
    keywords: Vec<String>,
}

/// A running export, shown in the top bar until it finishes.
struct Job {
    /// Progress in thousandths.
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

#[derive(Default)]
pub(super) struct Exports {
    pub(super) dialog: bool,
    draft: Settings,
    /// The folder chooser's answer, from its own thread.
    picked: Arc<Mutex<Option<PathBuf>>>,
    picking: Arc<AtomicBool>,
    jobs: Vec<Job>,
    /// A file that exists, waiting for Replace, Keep Both or Skip.
    conflict: Option<(PathBuf, Settings, Snapshot)>,
}

const WIDTH: f32 = 700.;
const HEIGHT: f32 = 600.;
const LABEL: f32 = 150.;

impl Editor {
    fn can_export(&mut self) -> bool {
        if self.document.full().is_none() || self.document.path.is_none() {
            self.status = "Open a photo to export it".into();
            return false;
        }
        true
    }
    /// File › Export…: the dialog, with the last export's choices.
    pub(super) fn open_export_dialog(&mut self) {
        if !self.can_export() {
            return;
        }
        self.exports.draft = Settings::load().unwrap_or_default();
        self.exports.dialog = true;
    }
    /// Export with Previous: the last export's choices, without the dialog.
    pub(super) fn export_with_previous(&mut self) {
        if !self.can_export() {
            return;
        }
        match Settings::load() {
            Some(settings) => self.run_export(settings),
            None => self.open_export_dialog(),
        }
    }
    /// The Export dialog or its existing-file question is showing.
    pub(super) fn export_modal(&self) -> bool {
        self.exports.dialog || self.exports.conflict.is_some()
    }
    pub(super) fn exporting(&self) -> bool {
        self.exports
            .jobs
            .iter()
            .any(|j| !j.done.load(Ordering::Relaxed))
    }
    fn snapshot(&self) -> Option<Snapshot> {
        let photo = self
            .document
            .catalog_photo
            .and_then(|id| self.library.as_ref()?.photo(id));
        let (rating, label, keywords) = photo.map_or((0, String::new(), Vec::new()), |p| {
            (
                p.rating,
                p.label.clone(),
                p.keywords
                    .split(',')
                    .map(|k| k.trim().to_string())
                    .collect(),
            )
        });
        Some(Snapshot {
            image: self.document.full()?.clone(),
            source: self.document.path.clone()?,
            recipe: self.document.recipe.clone(),
            rating,
            label,
            keywords,
        })
    }
    fn run_export(&mut self, settings: Settings) {
        settings.save();
        let Some(photo) = self.snapshot() else {
            return;
        };
        let Some(target) = settings.target(&photo.source) else {
            self.status = "Choose a folder to export to".into();
            return;
        };
        if target.exists() {
            match settings.existing {
                Existing::Ask => {
                    self.exports.conflict = Some((target, settings, photo));
                    return;
                }
                Existing::Unique => {
                    return self.start_job(photo, unique(&target), settings, false);
                }
                Existing::Overwrite => {}
                Existing::Skip => {
                    self.status = format!("Skipped: {} already exists", target.display());
                    return;
                }
            }
        }
        self.start_job(photo, target, settings, true);
    }
    fn start_job(&mut self, photo: Snapshot, target: PathBuf, settings: Settings, overwrite: bool) {
        let Snapshot {
            image,
            source,
            recipe,
            rating,
            label,
            keywords,
        } = photo;
        let job = Job {
            progress: Arc::new(AtomicU32::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
            done: Arc::new(AtomicBool::new(false)),
        };
        let (progress, cancel, done) = (job.progress.clone(), job.cancel.clone(), job.done.clone());
        self.exports.jobs.push(job);
        let tx = self.tx.clone();
        let ctx = self.context.clone();
        let name = target
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        self.status = format!("Exporting {name}…");
        std::thread::spawn(move || {
            let step = |n: u32| {
                progress.store(n, Ordering::Relaxed);
                ctx.request_repaint();
            };
            let result = (|| -> anyhow::Result<()> {
                step(50);
                // The open photo may still be the quick half-size decode.
                let image = if image.fast {
                    let cached = crate::decode_cache::DecodeCache::key(&source)
                        .ok()
                        .and_then(|key| {
                            crate::decode_cache::DecodeCache::default().load(&key, &image.metadata)
                        });
                    match cached {
                        Some(full) => Arc::new(full),
                        None => Arc::new(crate::raw::Raw::open(&source)?.develop(false, &cancel)?),
                    }
                } else {
                    image
                };
                anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Cancelled");
                step(400);
                let max_edge = if settings.resize {
                    settings.long_edge
                } else {
                    0
                };
                let rendered = crate::develop::render(&image, &recipe, max_edge)?;
                anyhow::ensure!(!cancel.load(Ordering::Relaxed), "Cancelled");
                step(850);
                let camera = settings.capture.then(|| exif::read(&source)).flatten();
                let xmp = (settings.develop || settings.descriptive).then(|| {
                    crate::xmp::write::packet(
                        &recipe,
                        &image.metadata,
                        &crate::xmp::write::Photo {
                            raw_name: source
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string(),
                            captured: camera.as_ref().and_then(|c| c.captured()),
                            now: now_utc(),
                            rating: if settings.descriptive { rating } else { 0 },
                            label: if settings.descriptive {
                                label
                            } else {
                                String::new()
                            },
                            keywords: if settings.descriptive {
                                keywords
                            } else {
                                Vec::new()
                            },
                            settings: settings.develop,
                            format: match settings.format {
                                Format::Jpeg => "image/jpeg",
                                Format::Tiff => "image/tiff",
                            }
                            .into(),
                        },
                    )
                });
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                crate::export::export_with(
                    &target,
                    &source,
                    &rendered,
                    &image.metadata,
                    &ExportOptions {
                        quality: settings.quality.clamp(1, 100),
                        max_edge,
                    },
                    &Embed {
                        camera,
                        capture: settings.capture,
                        location: settings.location,
                        xmp,
                        ppi: settings.ppi,
                    },
                    overwrite,
                )?;
                step(1000);
                Ok(())
            })();
            let status = match result {
                Ok(()) => format!("Exported {}", target.display()),
                Err(e) if cancel.load(Ordering::Relaxed) => {
                    let _ = e;
                    "Export cancelled".into()
                }
                Err(e) => format!("Export failed: {e:#}"),
            };
            done.store(true, Ordering::Relaxed);
            let _ = tx.send(Event::Exported(status));
            ctx.request_repaint();
        });
    }

    /// Lightroom's activity indicator: a bar in the top bar while exports run.
    pub(super) fn export_progress(&mut self, ui: &mut egui::Ui) {
        self.exports
            .jobs
            .retain(|j| !j.done.load(Ordering::Relaxed));
        if self.exports.jobs.is_empty() {
            return;
        }
        let n = self.exports.jobs.len();
        let fraction = self
            .exports
            .jobs
            .iter()
            .map(|j| j.progress.load(Ordering::Relaxed) as f32 / 1000.)
            .sum::<f32>()
            / n as f32;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(210., 28.), Sense::hover());
        let text = if n == 1 {
            "Exporting 1 photo".to_string()
        } else {
            format!("Exporting {n} photos")
        };
        ui.painter().text(
            egui::pos2(rect.left(), rect.top() + 7.),
            egui::Align2::LEFT_CENTER,
            text,
            egui::FontId::proportional(11.),
            Color32::from_gray(190),
        );
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.top() + 17.),
            Vec2::new(rect.width() - 26., 4.),
        );
        ui.painter().rect_filled(bar, 2., Color32::from_gray(50));
        ui.painter().rect_filled(
            egui::Rect::from_min_size(bar.min, Vec2::new(bar.width() * fraction, 4.)),
            2.,
            Color32::from_rgb(110, 150, 190),
        );
        let close = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 9., rect.center().y),
            Vec2::splat(18.),
        );
        let response = ui
            .interact(close, ui.id().with("cancel-export"), Sense::click())
            .on_hover_text("Cancel export");
        let color = Color32::from_gray(if response.hovered() { 235 } else { 150 });
        let c = close.center();
        for d in [Vec2::new(4., 4.), Vec2::new(4., -4.)] {
            ui.painter()
                .line_segment([c - d, c + d], Stroke::new(1.4, color));
        }
        if response.clicked() {
            for job in &self.exports.jobs {
                job.cancel.store(true, Ordering::Relaxed);
            }
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
    }

    pub(super) fn export_windows(&mut self, ctx: &egui::Context) {
        if let Some((target, settings, photo)) = self.exports.conflict.clone() {
            let mut choice = None;
            let response = egui::Modal::new(egui::Id::new("export-conflict"))
                .frame(dialog_frame())
                .show(ctx, |ui| {
                    ui.set_width(460.);
                    ui.add_space(20.);
                    ui.horizontal(|ui| {
                        ui.add_space(24.);
                        ui.vertical(|ui| {
                            ui.label(
                                egui::RichText::new("A file with this name already exists")
                                    .size(15.)
                                    .color(Color32::from_gray(236)),
                            );
                            ui.add_space(6.);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(target.display().to_string())
                                        .size(12.)
                                        .color(Color32::from_gray(150)),
                                )
                                .truncate(),
                            );
                        });
                    });
                    ui.add_space(20.);
                    ui.horizontal(|ui| {
                        ui.add_space(24.);
                        ui.spacing_mut().button_padding = Vec2::new(14., 6.);
                        if ui.button("Skip").clicked() {
                            choice = Some(Existing::Skip);
                        }
                        if ui.button("Use Unique Name").clicked() {
                            choice = Some(Existing::Unique);
                        }
                        if primary(ui, "Overwrite").clicked() {
                            choice = Some(Existing::Overwrite);
                        }
                    });
                    ui.add_space(20.);
                });
            if response.should_close() {
                choice = Some(Existing::Skip);
            }
            if let Some(choice) = choice {
                self.exports.conflict = None;
                match choice {
                    Existing::Overwrite => self.start_job(photo, target, settings, true),
                    Existing::Unique => self.start_job(photo, unique(&target), settings, false),
                    _ => self.status = "Export skipped".into(),
                }
            }
        }
        if !self.exports.dialog {
            return;
        }
        if let Some(folder) = self.exports.picked.lock().ok().and_then(|mut p| p.take()) {
            self.exports.draft.folder = Some(folder);
            self.exports.draft.destination = Destination::Folder;
        }
        let source = self.document.path.clone().unwrap_or_default();
        let mut action = None;
        let response = egui::Modal::new(egui::Id::new("export-dialog"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(dialog_frame())
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(WIDTH, HEIGHT), Sense::hover());
                let title = egui::Rect::from_min_size(rect.min, Vec2::new(WIDTH, 52.));
                ui.painter().text(
                    title.left_center() + Vec2::new(24., 0.),
                    egui::Align2::LEFT_CENTER,
                    format!(
                        "Export {}",
                        source.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    egui::FontId::proportional(16.),
                    Color32::from_gray(236),
                );
                let body = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 16., title.bottom()),
                    egui::pos2(rect.right() - 16., rect.bottom() - 64.),
                );
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(body));
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(&mut content, |ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(8., 8.);
                        ui.spacing_mut().interact_size.y = 26.;
                        self.export_sections(ui, &source);
                    });
                ui.painter().hline(
                    rect.x_range(),
                    rect.bottom() - 64.,
                    Stroke::new(1., Color32::from_gray(45)),
                );
                let footer = egui::Rect::from_min_max(
                    egui::pos2(rect.left() + 24., rect.bottom() - 56.),
                    egui::pos2(rect.right() - 24., rect.bottom() - 8.),
                );
                let mut bar = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(footer)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                bar.spacing_mut().button_padding = Vec2::new(18., 6.);
                if primary(&mut bar, "Export").clicked() {
                    action = Some(true);
                }
                if bar
                    .add(egui::Button::new("Cancel").min_size(Vec2::new(84., 30.)))
                    .clicked()
                {
                    action = Some(false);
                }
                if let Some(target) = self.exports.draft.target(&source) {
                    bar.add_space(12.);
                    bar.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("Saves to {}", pretty(&target)))
                                    .size(12.)
                                    .color(Color32::from_gray(140)),
                            )
                            .truncate(),
                        )
                        .on_hover_text(target.display().to_string());
                    });
                }
            });
        if response.should_close() && action.is_none() {
            action = Some(false);
        }
        match action {
            Some(true) => {
                let settings = self.exports.draft.clone();
                if settings.target(&source).is_none() {
                    self.status = "Choose a folder to export to".into();
                    return;
                }
                self.exports.dialog = false;
                self.run_export(settings);
            }
            Some(false) => self.exports.dialog = false,
            None => {}
        }
    }

    fn export_sections(&mut self, ui: &mut egui::Ui, source: &Path) {
        let s = &mut self.exports.draft;
        header(ui, "Export Location");
        row(ui, "Export To", |ui| {
            egui::ComboBox::from_id_salt("export-to")
                .width(300.)
                .selected_text(s.destination.label())
                .show_ui(ui, |ui| {
                    for d in Destination::ALL {
                        ui.selectable_value(&mut s.destination, d, d.label());
                    }
                });
        });
        let folder = s.base_folder(source);
        let mut choose = false;
        row(ui, "Folder", |ui| {
            choose = ui
                .add_enabled(
                    !self.exports.picking.load(Ordering::Relaxed),
                    egui::Button::new("Choose…"),
                )
                .clicked();
            let text = folder.as_deref().map_or("No folder chosen".into(), pretty);
            ui.add(
                egui::Label::new(egui::RichText::new(text).color(Color32::from_gray(150)))
                    .truncate(),
            );
        });
        row(ui, "", |ui| {
            ui.checkbox(&mut s.subfolder, "Put in Subfolder:");
            ui.add_enabled(
                s.subfolder,
                egui::TextEdit::singleline(&mut s.subfolder_name).desired_width(240.),
            );
        });
        row(ui, "Existing Files", |ui| {
            egui::ComboBox::from_id_salt("export-existing")
                .width(300.)
                .selected_text(s.existing.label())
                .show_ui(ui, |ui| {
                    for e in Existing::ALL {
                        ui.selectable_value(&mut s.existing, e, e.label());
                    }
                });
        });
        header(ui, "File Naming");
        row(ui, "", |ui| {
            ui.checkbox(&mut s.rename, "Rename To: Filename -");
            ui.add_enabled(
                s.rename,
                egui::TextEdit::singleline(&mut s.custom_text).desired_width(180.),
            );
        });
        row(ui, "Example", |ui| {
            ui.label(egui::RichText::new(s.file_name(source)).color(Color32::from_gray(225)));
        });
        row(ui, "Extensions", |ui| {
            egui::ComboBox::from_id_salt("export-case")
                .width(140.)
                .selected_text(if s.uppercase {
                    "Uppercase"
                } else {
                    "Lowercase"
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.uppercase, false, "Lowercase");
                    ui.selectable_value(&mut s.uppercase, true, "Uppercase");
                });
        });
        header(ui, "File Settings");
        row(ui, "Image Format", |ui| {
            egui::ComboBox::from_id_salt("export-format")
                .width(140.)
                .selected_text(match s.format {
                    Format::Jpeg => "JPEG",
                    Format::Tiff => "TIFF",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut s.format, Format::Jpeg, "JPEG");
                    ui.selectable_value(&mut s.format, Format::Tiff, "TIFF");
                });
        });
        // The same row height for both formats, so nothing below moves.
        match s.format {
            Format::Jpeg => row(ui, "Quality", |ui| {
                ui.spacing_mut().slider_width = 220.;
                ui.add(egui::Slider::new(&mut s.quality, 1..=100));
            }),
            Format::Tiff => row(ui, "Bit Depth", |ui| {
                ui.label(
                    egui::RichText::new("16 bits/component, uncompressed")
                        .color(Color32::from_gray(225)),
                );
            }),
        }
        row(ui, "Color Space", |ui| {
            ui.label(egui::RichText::new("sRGB").color(Color32::from_gray(225)));
        });
        header(ui, "Image Sizing");
        row(ui, "", |ui| {
            ui.checkbox(&mut s.resize, "Resize to Fit: Long Edge");
            ui.add_enabled(
                s.resize,
                egui::DragValue::new(&mut s.long_edge)
                    .range(100..=30_000)
                    .suffix(" px"),
            );
        });
        row(ui, "Resolution", |ui| {
            ui.add(
                egui::DragValue::new(&mut s.ppi)
                    .range(1..=10_000)
                    .suffix(" pixels per inch"),
            );
        });
        header(ui, "Metadata");
        row(ui, "", |ui| {
            ui.checkbox(&mut s.capture, "Camera and capture info (EXIF)");
        });
        row(ui, "", |ui| {
            ui.add_space(24.);
            ui.add_enabled(
                s.capture,
                egui::Checkbox::new(&mut s.location, "Include location info"),
            );
        });
        row(ui, "", |ui| {
            ui.checkbox(&mut s.develop, "Develop settings (Camera Raw XMP)");
        });
        row(ui, "", |ui| {
            ui.checkbox(&mut s.descriptive, "Rating, color label and keywords");
        });
        ui.add_space(8.);
        if choose {
            self.exports.picking.store(true, Ordering::Relaxed);
            let picked = self.exports.picked.clone();
            let picking = self.exports.picking.clone();
            let ctx = ui.ctx().clone();
            std::thread::spawn(move || {
                if let Some(folder) = rfd::FileDialog::new().pick_folder()
                    && let Ok(mut slot) = picked.lock()
                {
                    *slot = Some(folder);
                }
                picking.store(false, Ordering::Relaxed);
                ctx.request_repaint();
            });
        }
    }
}

fn dialog_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::from_gray(33))
        .stroke(Stroke::new(1., Color32::from_gray(52)))
        .corner_radius(10.)
}
fn primary(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(text).color(Color32::WHITE))
            .fill(Color32::from_rgb(62, 88, 115))
            .min_size(Vec2::new(84., 30.)),
    )
}
/// A Lightroom section band.
fn header(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.), Sense::hover());
    ui.painter().rect_filled(rect, 3., Color32::from_gray(44));
    ui.painter().text(
        rect.left_center() + Vec2::new(12., 0.),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(13.),
        Color32::from_gray(235),
    );
}
fn row(ui: &mut egui::Ui, label: &str, contents: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(LABEL, 26.), Sense::hover());
        ui.painter().text(
            rect.right_center() - Vec2::new(12., 0.),
            egui::Align2::RIGHT_CENTER,
            label,
            egui::FontId::proportional(13.),
            Color32::from_gray(150),
        );
        contents(ui);
    });
}
fn pretty(path: &Path) -> String {
    let text = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn target_follows_destination_subfolder_and_naming() {
        let source = Path::new("/photos/2026/DSC0001.ARW");
        let mut s = Settings::default();
        assert_eq!(
            s.target(source),
            Some(PathBuf::from("/photos/2026/DSC0001.jpg"))
        );
        s.subfolder = true;
        s.rename = true;
        s.custom_text = "web".into();
        s.format = Format::Tiff;
        s.uppercase = true;
        assert_eq!(
            s.target(source),
            Some(PathBuf::from("/photos/2026/Exported/DSC0001-web.TIF"))
        );
        s.destination = Destination::Folder;
        assert_eq!(s.target(source), None);
        let old: Settings = serde_json::from_str("{\"quality\": 80}").unwrap();
        assert_eq!(old.quality, 80);
        assert!(old.capture);
    }
    #[test]
    fn now_is_an_xmp_utc_date() {
        let now = now_utc();
        assert_eq!(now.len(), 20);
        assert!(now.ends_with('Z') && now.as_bytes()[10] == b'T');
    }
}
