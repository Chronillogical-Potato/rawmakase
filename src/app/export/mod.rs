//! Export commands and background exports. The dialog lives in `dialog`; the
//! export itself (settings, pipeline, file writing) in `crate::export`. Each
//! export runs on its own thread and the top bar shows its progress while you
//! keep editing.
mod dialog;

use super::{Editor, worker::Event};
use crate::app::theme;
use crate::export::{
    Existing, ExportSettings,
    job::{self, Photo},
    settings::unique,
};
use eframe::egui::{self, Color32, Sense, Vec2};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

/// A running export, shown in the top bar until it finishes.
struct Job {
    /// Progress in thousandths.
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

/// A file that already exists, waiting for Overwrite, Use Unique Name or Skip.
#[derive(Clone)]
struct Conflict {
    target: PathBuf,
    settings: ExportSettings,
    photo: Photo,
}

#[derive(Default)]
pub(super) struct Exports {
    /// The dialog is open, editing `draft`.
    dialog: bool,
    draft: ExportSettings,
    /// The folder chooser's answer, from its own thread.
    picked: Arc<Mutex<Option<PathBuf>>>,
    picking: Arc<AtomicBool>,
    jobs: Vec<Job>,
    conflict: Option<Conflict>,
}

impl Editor {
    /// Export…: the dialog, starting from the last export's choices.
    pub(super) fn open_export_dialog(&mut self) {
        if self.export_photo().is_some() {
            self.exports.draft = ExportSettings::load().unwrap_or_default();
            self.exports.dialog = true;
        }
    }
    /// Export with Previous: the last export's choices, without the dialog.
    pub(super) fn export_with_previous(&mut self) {
        match ExportSettings::load() {
            Some(settings) => self.export(settings),
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

    /// The open photo as it is now, with its catalog rating, label and keywords.
    fn export_photo(&mut self) -> Option<Photo> {
        let catalog = self
            .document
            .catalog_photo
            .and_then(|id| self.library.as_ref()?.photo(id));
        let photo = Some(Photo {
            image: self.document.full()?.clone(),
            source: self.document.path.clone()?,
            recipe: self.document.recipe.clone(),
            rating: catalog.map_or(0, |p| p.rating),
            label: catalog.map(|p| p.label.clone()).unwrap_or_default(),
            keywords: catalog
                .map(|p| {
                    p.keywords
                        .split(',')
                        .map(|k| k.trim().to_string())
                        .collect()
                })
                .unwrap_or_default(),
        });
        if photo.is_none() {
            self.status = "Open a photo to export it".into();
        }
        photo
    }

    fn export(&mut self, settings: ExportSettings) {
        if let Err(e) = settings.save() {
            self.status = format!("Export settings not saved: {e:#}");
        }
        let Some(photo) = self.export_photo() else {
            return;
        };
        let Some(target) = settings.target(&photo.source) else {
            self.status = "Choose a folder to export to".into();
            return;
        };
        if !target.exists() {
            return self.start_export(photo, target, settings, false);
        }
        match settings.existing {
            Existing::Ask => {
                self.exports.conflict = Some(Conflict {
                    target,
                    settings,
                    photo,
                })
            }
            Existing::Unique => self.start_export(photo, unique(&target), settings, false),
            Existing::Overwrite => self.start_export(photo, target, settings, true),
            Existing::Skip => self.status = format!("Skipped: {} already exists", target.display()),
        }
    }

    fn start_export(
        &mut self,
        photo: Photo,
        target: PathBuf,
        settings: ExportSettings,
        overwrite: bool,
    ) {
        let job = Job {
            progress: Default::default(),
            cancel: Default::default(),
            done: Default::default(),
        };
        let (progress, cancel, done) = (job.progress.clone(), job.cancel.clone(), job.done.clone());
        self.exports.jobs.push(job);
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        self.status = format!(
            "Exporting {}…",
            target.file_name().unwrap_or_default().to_string_lossy()
        );
        std::thread::spawn(move || {
            let result = job::run(photo, &settings, &target, overwrite, &cancel, |p| {
                progress.store((p * 1000.) as u32, Ordering::Relaxed);
                ctx.request_repaint();
            });
            let status = match result {
                Ok(()) => format!("Exported {}", target.display()),
                Err(_) if cancel.load(Ordering::Relaxed) => "Export cancelled".into(),
                Err(e) => format!("Export failed: {e:#}"),
            };
            done.store(true, Ordering::Relaxed);
            let _ = tx.send(Event::Exported(status));
            ctx.request_repaint();
        });
    }

    /// Lightroom's activity indicator: a bar in the top bar while exports run,
    /// with a button that cancels them.
    pub(super) fn export_progress(&mut self, ui: &mut egui::Ui) {
        let jobs = &mut self.exports.jobs;
        jobs.retain(|j| !j.done.load(Ordering::Relaxed));
        if jobs.is_empty() {
            return;
        }
        let n = jobs.len();
        let fraction = jobs
            .iter()
            .map(|j| j.progress.load(Ordering::Relaxed) as f32 / 1000.)
            .sum::<f32>()
            / n as f32;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(210., 28.), Sense::hover());
        let painter = ui.painter();
        painter.text(
            egui::pos2(rect.left(), rect.top() + 7.),
            egui::Align2::LEFT_CENTER,
            if n == 1 {
                "Exporting 1 photo".to_string()
            } else {
                format!("Exporting {n} photos")
            },
            egui::FontId::proportional(11.),
            theme::gray(190),
        );
        let bar = egui::Rect::from_min_size(
            egui::pos2(rect.left(), rect.top() + 17.),
            Vec2::new(rect.width() - 26., 4.),
        );
        painter.rect_filled(bar, 2., theme::gray(50));
        painter.rect_filled(
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
        let color = theme::gray(if response.hovered() { 235 } else { 150 });
        crate::app::icons::paint_at(
            ui.painter(),
            crate::app::icons::Icon::Close,
            close.center(),
            13.,
            color,
        );
        if response.clicked() {
            for job in jobs.iter() {
                job.cancel.store(true, Ordering::Relaxed);
            }
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(200));
    }
}
