//! Accept worker results at a single generation-checked boundary.
use super::{
    Editor,
    worker::{self, Event, LoadedHeader, Pane, RenderStage, TaskKind},
};
use crate::export::ExportOptions;
use eframe::egui;

impl Editor {
    pub(super) fn events(&mut self, ctx: &egui::Context) {
        self.import_progress(ctx);
        // A Point Color sample stops as soon as its tab is no longer where the photo is
        // edited (the Library, Before, another tab), whichever way that happened.
        if self.document.point_color_pick.is_running() && !self.point_color_tab_shown() {
            self.document.point_color_pick.invalidate();
        }
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::CatalogWorking(message) => {
                    self.status = message.clone();
                    self.catalog_work = Some(message);
                }
                Event::CatalogReady(result) => {
                    self.catalog_work = None;
                    self.catalog_ready(result);
                }

                Event::Monitor(p) => {
                    self.activity.finish_dialog();
                    self.view.monitor = Some(p);
                    let _ = self.save_session();
                    self.schedule();
                }
                Event::PresetLoad(p) => {
                    self.activity.finish_dialog();
                    match crate::presets::load_preset(&p) {
                        Ok(r) => {
                            let r = crate::presets::applied_to(r, &self.document.recipe);
                            let old = std::mem::replace(&mut self.document.recipe, r);
                            self.history(old);
                            self.ensure_upright();
                            self.schedule();
                        }
                        Err(e) => self.status = e.to_string(),
                    }
                }
                Event::Auto { id, kind, result } if id == self.load.id() => {
                    self.auto_ready(kind, result)
                }
                Event::PointColorSample {
                    id,
                    generation,
                    sampled,
                    result,
                } if id == self.load.id()
                    && self.document.point_color_pick.is_running()
                    && self.document.point_color_pick.id() == generation =>
                {
                    self.point_color_sample_ready(&sampled, result)
                }
                Event::TargetedSample {
                    id,
                    generation,
                    sampled,
                    result,
                } if id == self.load.id()
                    && self.document.targeted_pick.is_running()
                    && self.document.targeted_pick.id() == generation =>
                {
                    self.targeted_sample_ready(&sampled, result)
                }
                Event::Upright {
                    id,
                    generation,
                    analysed,
                    result,
                } if id == self.load.id() => self.upright_ready(generation, &analysed, result),
                Event::Straighten {
                    id,
                    generation,
                    analysed,
                    result,
                } if id == self.load.id() => {
                    self.auto_straighten_ready(generation, &analysed, result)
                }
                Event::XmpLibrary { scan, library } => self.presets_scanned(scan, library),
                Event::Profiles {
                    id,
                    profiles,
                    errors,
                } if id == self.load.id() => {
                    self.document.profiles = profiles;
                    self.document.profile_errors = errors;
                    self.refresh_photo_defaults();
                    self.refresh_preset_support();
                    if let Some(text) = self.document.pending_lightroom.take() {
                        self.apply_lightroom_edits(&text);
                        // The Lightroom edit is the starting point, not an unsaved change.
                        self.document.save.saved();
                    }
                }
                Event::Import(kind, paths) => {
                    self.activity.finish_dialog();
                    self.import(kind, paths, ctx);
                }
                Event::Imported(summary) => self.imported(summary, ctx),
                Event::Synced(result) => self.synced(*result),
                Event::PresetSave(p) => {
                    self.activity.finish_dialog();
                    match crate::presets::save_preset(&p, &self.document.recipe) {
                        Ok(()) => self.status = "Preset saved".into(),
                        Err(e) => self.status = e.to_string(),
                    }
                }
                Event::Header(header) if header.id == self.load.id() => self.header_ready(*header),
                Event::Embedded { id, image: im } if id == self.load.id() => {
                    // The camera JPEG is uncropped and unedited; when the Library
                    // already has the edited thumbnail, keep showing that until
                    // the first render instead of flashing the original.
                    let edited = self
                        .document
                        .catalog_photo
                        .zip(self.library.as_ref())
                        .is_some_and(|(id, l)| l.has_edited_thumbnail(id));
                    if edited {
                        continue;
                    }
                    let k = (360. / im.width().max(im.height()) as f32).min(1.);
                    let navigator = image::imageops::thumbnail(
                        &im,
                        ((im.width() as f32 * k) as u32).max(1),
                        ((im.height() as f32 * k) as u32).max(1),
                    );
                    self.set_pixels(
                        ctx,
                        false,
                        [im.width(), im.height()],
                        im.as_raw(),
                        Some(navigator),
                    );
                    self.preview.mode = super::state::TextureMode::Whole;
                    // Uncropped, so it is placed by the photo's crop once that is known.
                    self.preview.crop = Some([0., 0., 1., 1.]);
                    self.preview.status = "Camera preview • developing RAW…".into();
                }
                Event::Ready { id, full, status } if id == self.load.id() => {
                    self.document.set_image(full);
                    self.load.finish(id);
                    // An Upright mode chosen before the photo decoded still needs analysing.
                    self.ensure_upright();
                    if !self.document.save.is_protected() {
                        // A raw default that could not be used stays explained.
                        self.status = match self.defaults_note() {
                            Some(note) => format!("{status} · {note}"),
                            None => status,
                        };
                    }
                    self.schedule();
                }
                Event::Reference { ticket, result } => self.reference_ready(ticket, result),
                Event::Rendered {
                    id,
                    pane: Pane::Before,
                    preview,
                    stage,
                    ..
                } if id == self.preview.before.task.id() => {
                    self.before_rendered(ctx, preview, stage, id);
                }
                Event::Rendered {
                    id,
                    pane: Pane::After,
                    preview,
                    histogram,
                    thumbnail,
                    samples,
                    stage,
                    status,
                } if id == self.preview.task.id() => {
                    let region = matches!(
                        self.preview.pending_mode,
                        super::state::TextureMode::Region(_)
                    );
                    // A region's own histogram would describe only what is
                    // visible; the whole photo's follows as `Histogram`.
                    if !region {
                        self.preview.histogram = *histogram;
                    }
                    match preview {
                        worker::Preview::Pixels {
                            image,
                            display_rgb,
                            navigator,
                        } => self.set_pixels(
                            ctx,
                            region,
                            [image.width, image.height],
                            &display_rgb,
                            navigator,
                        ),
                        worker::Preview::Texture {
                            id,
                            size,
                            navigator,
                        } => self.set_presented(region, (id, size), navigator),
                    }
                    // Pixels asked for by a hover or loupe that has since ended are
                    // not kept.
                    let samples = samples.filter(|_| self.preview.samples_requested);
                    if region {
                        self.preview.region_samples = samples;
                    } else {
                        self.preview.samples = samples;
                    }
                    self.preview.samples_recipe = self.preview.pending_recipe.clone();
                    self.preview.mode = self.preview.pending_mode;
                    if !region {
                        self.preview.crop = Some(self.preview.pending_crop);
                    }
                    if stage != RenderStage::Draft {
                        self.preview.task.finish(id);
                        if let Some(small) = thumbnail {
                            self.refresh_library_thumbnail(ctx, small);
                        }
                    }
                    self.preview.status = status;
                }
                Event::Histogram { id, histogram } if id == self.preview.task.id() => {
                    self.preview.histogram = *histogram;
                }
                Event::Failed {
                    id,
                    task: TaskKind::Load,
                    error,
                } if id == self.load.id() => {
                    self.status = error;
                    self.load.finish(id);
                }
                Event::Failed {
                    id,
                    task: TaskKind::Render(Pane::After),
                    error,
                } if id == self.preview.task.id() => {
                    self.status = error;
                    self.preview.task.finish(id);
                    // A failed render on the GPU retires Before's textures too: render
                    // Before again, not the edit that failed.
                    let before = &mut self.preview.before;
                    if before.texture.is_none() && before.region.is_none() {
                        before.forget_job();
                        self.schedule_before();
                    }
                }
                Event::Failed {
                    id,
                    task: TaskKind::Render(Pane::Before),
                    error,
                } if id == self.preview.before.task.id() => {
                    self.status = format!("Before: {error}");
                    self.preview.before.task.finish(id);
                    // A failed render on the GPU retires the edit's textures too: render
                    // the edit again. Before keeps its failed job, so it is not retried.
                    if self.preview.texture.is_none() && self.preview.region.is_none() {
                        self.schedule();
                    }
                }
                Event::RendererReset(retired) => {
                    self.preview.forget_presented();
                    drop(retired);
                }
                Event::DialogClosed => {
                    self.activity.finish_dialog();
                }
                Event::BatchExported { ticket, outcomes } => self.batch_exported(ticket, outcomes),
                _ => {}
            }
        }
    }
    fn catalog_ready(&mut self, result: Result<Box<super::library::Library>, String>) {
        self.activity.finish_dialog();
        match result {
            Ok(mut l) => {
                self.load.invalidate();
                // The photo being left is Previous, as when moving between photos.
                if let Some(settings) = self.current_settings() {
                    self.previous_settings = Some(settings);
                }
                self.document.reset(None);
                self.preview.clear_document();
                self.presets.clear_document();
                self.view.clear_document();
                self.status = if l.message.is_empty() {
                    "Catalog ready. Offline photos remain in the library; locate their folders to develop them.".into()
                } else {
                    l.message.clone()
                };
                // Commands never cross catalogs; reloading this one (after
                // adding or relinking a folder) keeps them.
                let reloaded = self
                    .library
                    .as_ref()
                    .is_some_and(|old| old.catalog.path == l.catalog.path);
                if !reloaded {
                    self.undo_log.clear();
                    // Photo ids belong to their catalog, and so does the reference.
                    self.clear_reference();
                }
                l.set_defaults(self.raw_defaults.clone());
                self.library = Some(l);
                self.library_mode = true;
                // On launch, return to the folder, photo and module of last time.
                let restore = self.restore.take();
                if let Some(library) = &mut self.library {
                    if let Some((source, photo, _)) = &restore {
                        library.restore_source(source, *photo);
                    }
                    // The Library as it was shown, on launch and when this
                    // catalog is loaded again (a folder added or relinked);
                    // another catalog starts with every photo shown.
                    if restore.is_some() || reloaded {
                        library.apply_layout(&self.saved_layout);
                    }
                }
                // Develop reopens on its photo, even one the filters now hide.
                if let Some((_, photo, true)) = restore
                    && let Some(id) = photo.or_else(|| self.library.as_ref()?.selected())
                {
                    self.develop_catalog_photo(id);
                    // On launch, a photo gone offline leaves the Library shown
                    // without a dialog; the status bar says why.
                    self.not_editable = None;
                }
                self.open_pending_photo();
                let _ = self.save_session();
            }
            Err(e) => {
                self.pending_photo = None;
                self.status = format!("Catalog operation failed: {e}");
            }
        }
    }

    fn header_ready(&mut self, header: LoadedHeader) {
        let LoadedHeader {
            path: p,
            metadata: m,
            recipe: r,
            export: ex,
            status,
            ..
        } = header;
        self.document.file = crate::storage::Identity::read(&p).ok();
        self.document.metadata = Some(m);
        self.document.recipe = r;
        self.document.export = ex;
        self.document.save.saved();
        self.status = status;
        if let (Some(l), Some(photo)) = (&self.library, self.document.catalog_photo) {
            self.document.lightroom_history =
                l.catalog.lightroom_history(photo).unwrap_or_default();
            self.document.snapshots.list = l.catalog.snapshots(photo).unwrap_or_default();
            // The edit as the catalog stores it, read once: the Lightroom settings
            // applied below are the ones read with it.
            let record = l.catalog.edit_record(photo);
            let saved = record
                .as_ref()
                .map_err(|e| anyhow::anyhow!("{e:#}"))
                .and_then(|r| r.saved(&p));
            match saved {
                Ok(Some(saved)) => {
                    self.document.origin = super::state::EditOrigin::Saved;
                    self.document.recipe = saved.recipe;
                    self.document.export = saved.export;
                    // A History that cannot be read leaves the edit as it is.
                    if let Ok(Some(history)) = l.catalog.load_history(photo) {
                        self.document.history =
                            super::history::History::restored(history, &self.document.recipe);
                    }
                    self.document.save.saved();
                    self.document.lightroom_notice.clear();
                }
                Ok(None) => {
                    self.document.export = ExportOptions::default();
                    self.document.save.saved();
                    self.document.lightroom_notice.clear();
                    // No RAWmakase edit yet: start from the Lightroom edit, as
                    // Lightroom shows it, once camera profiles are known.
                    self.document.pending_lightroom =
                        record.ok().and_then(|r| r.lightroom().map(str::to_owned));
                    if self.document.pending_lightroom.is_some() {
                        self.document.origin = super::state::EditOrigin::Lightroom;
                    }
                }
                Err(e) => {
                    // Unreadable is not unedited: it must not follow the defaults.
                    self.document.origin = super::state::EditOrigin::Saved;
                    self.document.save.protect(e.to_string());
                    self.document.lightroom_notice = e.to_string();
                }
            }
        }
        self.document.path = Some(p);
        let _ = self.save_session();
    }
}
impl Editor {
    /// After a finished whole-photo render of the current edit, show it as
    /// the photo's Library and filmstrip thumbnail.
    /// Whether the viewport shows the catalog photo's edit as the library would.
    pub(super) fn shows_library_edit(&self) -> bool {
        self.library.is_some()
            && self.document.catalog_photo.is_some()
            && self.document.path.is_some()
            && !self.view.zoom.on
            && !self.view.compare.shows_before()
            && !self.view.is(super::state::Tool::Crop)
            && self.presets.preview.is_none()
    }
    fn refresh_library_thumbnail(&mut self, ctx: &egui::Context, small: image::RgbImage) {
        if self.preview.mode != super::state::TextureMode::Whole || !self.shows_library_edit() {
            return;
        }
        let Ok(json) = serde_json::to_string(&self.document.recipe) else {
            return;
        };
        let (Some(library), Some(id)) = (&mut self.library, self.document.catalog_photo) else {
            return;
        };
        library.update_edited(ctx, id, small, json);
    }
}
