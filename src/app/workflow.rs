use super::Editor;
use super::state::Picture;
use super::worker::{LoadJob, RenderJob};
use crate::develop::{Geometry, Recipe};
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
            // Photos are edited through the Library only.
            self.status = format!(
                "Add {}'s folder to the Library to edit it",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            self.library_mode = true;
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
    /// Saves the edit now, after any background save in flight; false if
    /// it could not be saved.
    pub(super) fn flush(&mut self) -> bool {
        if let Some(done) = self.autosave.wait() {
            self.background_saved(done);
        }
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
                    self.saved_to(&p);
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
    /// Autosave: collects a finished background save and, once the edit
    /// has settled, starts the next.
    pub(super) fn autosave(&mut self, ctx: &egui::Context) {
        if let Some(done) = self.autosave.poll() {
            self.background_saved(done);
        }
        if !self.document.save.ready() || self.document.history.in_gesture() || self.autosave.busy()
        {
            return;
        }
        let Some(raw) = self.document.path.clone() else {
            return;
        };
        let target = match (&self.library, self.document.catalog_photo) {
            (Some(l), Some(photo)) => super::autosave::Target::Catalog {
                path: l.catalog.path.clone(),
                photo,
            },
            _ => super::autosave::Target::Sidecar,
        };
        let job = super::autosave::Job {
            target,
            raw,
            recipe: self.document.recipe.clone(),
            export: self.document.export.clone(),
        };
        match self.autosave.submit(job, ctx) {
            Ok(()) => self.document.save.saving(),
            // No saver thread: save here, as before.
            Err(_) => {
                self.flush();
            }
        }
    }
    fn background_saved(&mut self, done: super::autosave::Done) {
        let result = done.as_ref().map(|_| ()).map_err(Clone::clone);
        if !self.document.save.finished(result) {
            return;
        }
        match done {
            Ok(p) => self.saved_to(&p),
            Err(e) => self.status = format!("Edits not saved: {e}"),
        }
    }
    fn saved_to(&mut self, path: &std::path::Path) {
        self.status = format!(
            "Saved {}",
            path.file_name().unwrap_or_default().to_string_lossy()
        );
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
        if self.view.is(super::state::Tool::Crop) {
            r.crop = [0., 0., 1., 1.];
        }
        r
    }
    /// What the active tool draws into the rendered preview.
    pub(super) fn overlay(&self) -> super::worker::Overlay {
        use super::{state::Tool, worker::Overlay};
        match self.view.tool {
            Tool::Remove if self.view.retouch.visualize => {
                Overlay::Spots(self.view.retouch.threshold)
            }
            Tool::Mask if self.view.masking.overlay => match self.view.masking.selected {
                Some(index) if index < self.document.recipe.masks.len() => Overlay::Mask {
                    index,
                    color: [230, 40, 40],
                    opacity: 0.5,
                },
                _ => Overlay::None,
            },
            _ => Overlay::None,
        }
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
                max_edge,
                cancel,
                id,
                image,
                recipe: self.effective_recipe(),
                region,
                monitor: self.view.monitor.clone(),
                clipping: self.view.clipping,
                navigator: !self.view.zoom100,
                thumbnail: region.is_none() && self.shows_library_edit(),
                overlay: self.overlay(),
            });
        }
    }
    /// A CPU render: the whole photo, or a 100% region drawn over it.
    pub(super) fn set_pixels(
        &mut self,
        ctx: &egui::Context,
        region: bool,
        [w, h]: [u32; 2],
        data: &[u8],
        navigator: Option<image::RgbImage>,
    ) {
        let image = egui::ColorImage::from_rgb([w as usize, h as usize], data);
        if region {
            Picture::upload(&mut self.preview.region, ctx, "photo region", image);
            return;
        }
        if !self.view.zoom100
            && let Some(small) = navigator
        {
            let small = egui::ColorImage::from_rgb(
                [small.width() as usize, small.height() as usize],
                small.as_raw(),
            );
            Picture::upload(&mut self.preview.navigator, ctx, "navigator", small);
        }
        Picture::upload(&mut self.preview.texture, ctx, "photo", image);
    }
    /// A GPU render, presented into textures the renderer registered.
    pub(super) fn set_presented(
        &mut self,
        region: bool,
        (id, size): (egui::TextureId, [usize; 2]),
        navigator: Option<(egui::TextureId, [usize; 2])>,
    ) {
        let picture = Some(Picture::presented(id, size));
        if region {
            self.preview.region = picture;
            return;
        }
        if !self.view.zoom100
            && let Some((id, size)) = navigator
        {
            self.preview.navigator = Some(Picture::presented(id, size));
        }
        self.preview.texture = picture;
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
