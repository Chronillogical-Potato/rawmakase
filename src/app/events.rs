//! Accept worker results at a single generation-checked boundary.
use super::{
    Editor,
    worker::{Event, LoadedHeader, RenderStage, TaskKind},
};
use crate::export::ExportOptions;
use eframe::egui;

impl Editor {
    pub(super) fn events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::CatalogReady(result) => self.catalog_ready(result),

                Event::Open(p) => {
                    self.activity.finish_dialog();
                    self.open(p);
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
                            let old = std::mem::replace(&mut self.document.recipe, r);
                            self.history(old);
                            self.schedule();
                        }
                        Err(e) => self.status = e.to_string(),
                    }
                }
                Event::XmpLibrary(library) => {
                    self.presets.library = library;
                    self.refresh_preset_support();
                }
                Event::XmpImport(path) => {
                    self.activity.finish_dialog();
                    match crate::presets::import_file(&path) {
                        Ok(_) => {
                            self.status = "Preset imported".into();
                            self.reload_presets(ctx);
                        }
                        Err(e) => self.status = format!("Preset not imported: {e:#}"),
                    }
                }
                Event::Profiles {
                    id,
                    profiles,
                    errors,
                } if id == self.load.id() => {
                    self.document.profiles = profiles;
                    self.document.profile_errors = errors;
                    self.refresh_preset_support();
                    if std::mem::take(&mut self.document.pending_lightroom) {
                        self.apply_lightroom_edits();
                        // The Lightroom edit is the starting point, not an unsaved change.
                        self.document.save.saved();
                    }
                }
                Event::CameraProfile(p) => self.camera_profile_ready(p),
                Event::LensProfiles(p) => self.lens_profiles_ready(p),
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
                        .path
                        .as_ref()
                        .zip(self.library.as_ref())
                        .is_some_and(|(p, l)| l.has_edited_thumbnail(p));
                    if edited {
                        continue;
                    }
                    self.set_texture(ctx, im.width(), im.height(), im.as_raw());
                    self.preview.mode = super::state::TextureMode::Whole;
                    self.preview.status = "Camera preview • developing RAW…".into();
                }
                Event::Ready { id, full, status } if id == self.load.id() => {
                    self.document.set_image(full);
                    self.load.finish(id);
                    if !self.document.save.is_protected() {
                        self.status = status;
                    }
                    self.schedule();
                }
                Event::Thumbnail {
                    id,
                    path: p,
                    image: im,
                } if id == self.load.id() && self.preview.thumbs.len() < 32 => {
                    self.preview.thumbs.insert(
                        p,
                        ctx.load_texture(
                            format!("thumb-{}", self.preview.thumbs.len()),
                            egui::ColorImage::from_rgb(
                                [im.width() as usize, im.height() as usize],
                                im.as_raw(),
                            ),
                            egui::TextureOptions::LINEAR,
                        ),
                    );
                }
                Event::Rendered {
                    id,
                    image: im,
                    display_rgb: rgb,
                    stage,
                    status,
                } if id == self.preview.task.id() => {
                    self.preview.histogram = im.histogram();
                    if matches!(
                        self.preview.pending_mode,
                        super::state::TextureMode::Region(_)
                    ) {
                        self.set_region_texture(ctx, im.width, im.height, &rgb);
                    } else {
                        self.set_texture(ctx, im.width, im.height, &rgb);
                    }
                    self.preview.mode = self.preview.pending_mode;
                    if stage != RenderStage::Draft {
                        self.preview.task.finish(id);
                        self.refresh_library_thumbnail(ctx, &im);
                    }
                    self.preview.status = status;
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
                    task: TaskKind::Render,
                    error,
                } if id == self.preview.task.id() => {
                    self.status = error;
                    self.preview.task.finish(id);
                }
                Event::DialogClosed => {
                    self.activity.finish_dialog();
                }
                Event::Exported(s) => {
                    self.status = s;
                }
                _ => {}
            }
        }
    }
    fn catalog_ready(&mut self, result: Result<Box<super::library::Library>, String>) {
        self.activity.finish_dialog();
        match result {
            Ok(l) => {
                self.load.invalidate();
                self.document.reset(None);
                self.preview.clear_document();
                self.presets.clear_document();
                self.view.clear_document();
                self.status = if l.message.is_empty() {
                    "Catalog ready. Offline photos remain in the library; locate their folders to develop them.".into()
                } else {
                    l.message.clone()
                };
                self.library = Some(l);
                self.library_mode = true;
                // On launch, return to the folder, photo and module of last time.
                if let Some((source, photo, develop)) = self.restore.take()
                    && let Some(library) = &mut self.library
                {
                    library.restore_source(&source, photo);
                    if develop && let Some(id) = library.selected {
                        self.develop_catalog_photo(id);
                    }
                }
                let _ = self.save_session();
            }
            Err(e) => self.status = format!("Catalog operation failed: {e}"),
        }
    }

    fn lens_profiles_ready(&mut self, paths: Vec<std::path::PathBuf>) {
        self.activity.finish_dialog();
        match crate::lens::lcp::import_files(&paths) {
            Ok(imported) => {
                self.status = format!("Imported {} lens profiles", imported.len());
                // Lens profiles are matched when a photo opens: reopen it.
                if let Some(path) = self.document.path.clone() {
                    let photo = self.document.catalog_photo;
                    self.open_raw(path, photo);
                }
            }
            Err(e) => self.status = format!("Lens profiles not imported: {e:#}"),
        }
    }
    fn camera_profile_ready(&mut self, paths: Vec<std::path::PathBuf>) {
        self.activity.finish_dialog();
        match crate::camera_profiles::import_files(&paths) {
            Ok(imported) => {
                if let Some(m) = &self.document.metadata {
                    let (profiles, errors) = crate::camera_profiles::installed(m);
                    self.document.profiles = profiles;
                    self.document.profile_errors = errors;
                    // Importing a library never changes the active edit. The user selects
                    // a profile explicitly; new photos use the imported default.
                    self.refresh_preset_support();
                }
                self.status = format!(
                    "Imported {} profile files. Choose a profile from the Profile menu.",
                    imported.len()
                );
            }
            Err(e) => self.status = format!("Profiles not imported: {e:#}"),
        }
    }

    fn header_ready(&mut self, header: LoadedHeader) {
        let LoadedHeader {
            path: p,
            metadata: m,
            recipe: r,
            export: ex,
            protected,
            status,
            files,
            ..
        } = header;
        self.document.metadata = Some(m);
        self.document.recipe = r;
        self.document.export = ex;
        if protected {
            self.document.save.protect(status.clone());
        } else {
            self.document.save.saved();
        }
        self.status = status;
        if let (Some(l), Some(photo)) = (&self.library, self.document.catalog_photo) {
            self.document.lightroom_history =
                l.catalog.lightroom_history(photo).unwrap_or_default();
            match l.catalog.load_edit(photo, &p) {
                Ok(Some(saved)) => {
                    self.document.recipe = saved.recipe;
                    self.document.export = saved.export;
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
                        l.photo(photo).is_some_and(|p| p.has_lightroom_edits);
                }
                Err(e) => {
                    self.document.save.protect(e.to_string());
                    self.document.lightroom_notice = e.to_string();
                }
            }
        }
        self.document.path = Some(p);
        self.document.files = files;
        let _ = self.save_session();
    }
}
impl Editor {
    /// After a finished whole-photo render of the current edit, show it as
    /// the photo's Library and filmstrip thumbnail.
    fn refresh_library_thumbnail(&mut self, ctx: &egui::Context, im: &crate::develop::Rendered) {
        let showing_edit = self.preview.mode == super::state::TextureMode::Whole
            && !self.view.zoom100
            && !self.view.compare
            && !self.view.crop_mode
            && self.presets.preview.is_none();
        let (Some(library), Some(_), Some(path)) = (
            &mut self.library,
            self.document.catalog_photo,
            self.document.path.clone(),
        ) else {
            return;
        };
        let Ok(json) = serde_json::to_string(&self.document.recipe) else {
            return;
        };
        if !showing_edit {
            return;
        }
        let Some(full) = image::RgbImage::from_raw(im.width, im.height, im.rgb8()) else {
            return;
        };
        let k = (640. / im.width.max(im.height) as f32).min(1.);
        let small = image::imageops::thumbnail(
            &full,
            ((im.width as f32 * k) as u32).max(1),
            ((im.height as f32 * k) as u32).max(1),
        );
        library.update_edited(ctx, &path, small, json);
    }
}
