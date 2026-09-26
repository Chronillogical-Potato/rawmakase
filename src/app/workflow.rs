use super::Editor;
use super::worker::{Event, LoadJob, RenderJob};
use crate::develop::{self, Geometry, Recipe};
use eframe::egui;
use std::path::PathBuf;

impl Editor {
    pub(super) fn open(&mut self, path: PathBuf) {
        if self.activity.is_busy() {
            return;
        }
        if path.extension().is_some_and(|e| e == "rawmakase") {
            self.load_catalog(path, &self.context.clone());
        } else if path.extension().is_some_and(|e| e == "lrcat") {
            self.status =
                "Use Library → Import Lightroom catalog to select a new RAWmakase catalog destination"
                    .into();
            self.library_mode = true;
        } else {
            self.open_raw(path, None);
        }
    }
    pub(super) fn open_raw(&mut self, path: PathBuf, photo: Option<i64>) {
        if self.activity.is_busy() {
            return;
        }
        if !self.flush() {
            return;
        }
        self.document.reset(photo);
        self.library_mode = false;
        let (id, cancel) = self.load.start();
        self.preview.clear_document();
        self.presets.clear_document();
        self.view.clear_document();
        self.status = "Reading RAW…".into();
        self.loader.submit(LoadJob {
            catalog: photo.is_some(),
            id,
            path,
            cancel,
        });
    }
    pub(super) fn flush(&mut self) -> bool {
        if !self.document.save.needs_save() {
            return true;
        }
        if let Some(path) = &self.document.path {
            let saved = if let (Some(l), Some(id)) = (&self.library, self.document.catalog_photo) {
                l.catalog
                    .save_edit(id, path, &self.document.recipe, &self.document.export)
                    .map(|()| l.catalog.path.clone())
            } else {
                crate::storage::save(path, &self.document.recipe, &self.document.export)
            };
            match saved {
                Ok(p) => {
                    self.status = format!(
                        "Saved {}",
                        p.file_name().unwrap_or_default().to_string_lossy()
                    );
                    self.document.save.saved();
                }
                Err(e) => {
                    self.document.save.failed(e.to_string());
                    self.status = format!("Edits not saved: {e}");
                    return false;
                }
            }
        }
        true
    }
    pub(super) fn history(&mut self, old: Recipe) {
        if self.document.history.record(old, &self.document.recipe) {
            self.document.save.mark_changed();
        }
    }
    pub(super) fn undo(&mut self) {
        if self.document.history.undo(&mut self.document.recipe) {
            self.document.save.mark_changed();
            self.schedule();
        }
    }
    pub(super) fn redo(&mut self) {
        if self.document.history.redo(&mut self.document.recipe) {
            self.document.save.mark_changed();
            self.schedule();
        }
    }
    pub(super) fn effective_recipe(&self) -> Recipe {
        let mut r = if self.view.compare {
            let mut r = self
                .document
                .metadata
                .as_ref()
                .map(|m| Recipe::with_profiles(m, &self.document.profiles))
                .unwrap_or_default();
            r.crop = self.document.recipe.crop;
            r.rotation = self.document.recipe.rotation;
            r.flip_x = self.document.recipe.flip_x;
            r.flip_y = self.document.recipe.flip_y;
            r.straighten = self.document.recipe.straighten;
            r
        } else {
            self.presets
                .preview
                .as_ref()
                .unwrap_or(&self.document.recipe)
                .clone()
        };
        if self.view.crop_mode {
            r.crop = [0., 0., 1., 1.];
        }
        r
    }
    /// The 1:1 region to render when zoomed to 100% or more; below 100% the
    /// whole photo is rendered at the zoomed size instead.
    pub(super) fn region(&self) -> Option<[u32; 4]> {
        if !self.view.zoom100 || self.view.zoom_level < 1. {
            return None;
        }
        let im = self.document.full()?;
        let g = Geometry::new(im, &self.effective_recipe(), 0);
        let z = self.view.zoom_level;
        let w = ((self.view.viewport.x / z).ceil() as u32).clamp(1, g.width);
        let h = ((self.view.viewport.y / z).ceil() as u32).clamp(1, g.height);
        let x = (self.view.pan[0] * g.width as f32 - w as f32 / 2.)
            .round()
            .clamp(0., (g.width - w) as f32) as u32;
        let y = (self.view.pan[1] * g.height as f32 - h as f32 / 2.)
            .round()
            .clamp(0., (g.height - h) as f32) as u32;
        Some([x, y, w, h])
    }
    pub(super) fn schedule(&mut self) {
        let image = self.document.full().cloned();
        if let Some(image) = image {
            let (id, cancel) = self.preview.task.start();
            let region = self.region();
            self.preview.last_region = region;
            let geometry = Geometry::new(&image, &self.effective_recipe(), 0);
            let fit = crate::develop::quality::fit_edge(
                geometry.width,
                geometry.height,
                [self.view.viewport.x as u32, self.view.viewport.y as u32],
            );
            self.preview.last_fit_edge = fit;
            let max_edge = if self.view.zoom100 && self.view.zoom_level < 1. {
                (geometry.width.max(geometry.height) as f32 * self.view.zoom_level).round() as u32
            } else {
                fit
            };
            self.preview.pending_mode = region.map_or(
                super::state::TextureMode::Whole,
                super::state::TextureMode::Region,
            );
            self.renderer.submit(RenderJob {
                draft: self
                    .document
                    .draft()
                    .cloned()
                    .unwrap_or_else(|| image.clone()),
                max_edge,
                cancel,
                id,
                image,
                recipe: self.effective_recipe(),
                region,
                monitor: self.view.monitor.clone(),
                clipping: self.view.clipping,
            });
        }
    }
    pub(super) fn set_texture(&mut self, ctx: &egui::Context, w: u32, h: u32, data: &[u8]) {
        if !self.view.zoom100
            && let Some(full) = image::RgbImage::from_raw(w, h, data.to_vec())
        {
            let scale = (360. / w.max(h) as f32).min(1.);
            let small = image::imageops::thumbnail(
                &full,
                ((w as f32 * scale) as u32).max(1),
                ((h as f32 * scale) as u32).max(1),
            );
            let small = egui::ColorImage::from_rgb(
                [small.width() as usize, small.height() as usize],
                small.as_raw(),
            );
            match &mut self.preview.navigator {
                Some(t) => t.set(small, egui::TextureOptions::LINEAR),
                None => {
                    self.preview.navigator =
                        Some(ctx.load_texture("navigator", small, egui::TextureOptions::LINEAR))
                }
            }
        }
        let image = egui::ColorImage::from_rgb([w as usize, h as usize], data);
        if let Some(t) = &mut self.preview.texture {
            t.set(image, egui::TextureOptions::LINEAR);
        } else {
            self.preview.texture =
                Some(ctx.load_texture("photo", image, egui::TextureOptions::LINEAR));
        }
    }
    pub(super) fn start_export(&mut self, path: PathBuf, overwrite: bool, ctx: &egui::Context) {
        let (Some(im), Some(source)) = (self.document.full().cloned(), self.document.path.clone())
        else {
            return;
        };
        if im.fast {
            self.status = "Wait for the full-resolution image before exporting".into();
            return;
        }
        if self.activity.is_exporting() {
            return;
        }
        let r = self.document.recipe.clone();
        let options = self.document.export.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        if !self.activity.begin_export() {
            return;
        }
        self.status = "Exporting…".into();
        std::thread::spawn(move || {
            let result = develop::render(&im, &r, options.max_edge).and_then(|out| {
                crate::export::export(&path, &source, &out, &im.metadata, &options, overwrite)
            });
            let status = match result {
                Ok(()) => format!("Exported {}", path.display()),
                Err(e) => format!("Export failed: {e}"),
            };
            let _ = tx.send(Event::Exported(status));
            ctx.request_repaint();
        });
    }
    pub(super) fn navigate(&mut self, delta: isize) {
        if let (Some(l), Some(id)) = (&self.library, self.document.catalog_photo) {
            if let Some(next) = l.navigate(id, delta as i32) {
                self.develop_catalog_photo(next);
            }
            return;
        }
        if self.document.files.is_empty() {
            return;
        }
        let i = self
            .document
            .path
            .as_ref()
            .and_then(|p| self.document.files.iter().position(|f| f == p))
            .unwrap_or(0);
        let next = (i as isize + delta).clamp(0, self.document.files.len() as isize - 1) as usize;
        if next != i {
            self.open(self.document.files[next].clone());
        }
    }
}
