//! One photo's export from start to finish: the full-size image (decoded here when
//! only a quick preview is loaded), the render, its metadata and the file.
use super::{
    Embed, ExportSettings,
    assemble::{Policy, Values, assemble},
    exif,
};
use crate::{
    decode_cache::DecodeCache,
    develop::Recipe,
    raw::{CameraImage, Raw},
};
use anyhow::{Result, ensure};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// The photo as it was when Export was chosen.
#[derive(Clone)]
pub struct Photo {
    pub image: Arc<CameraImage>,
    pub source: PathBuf,
    pub recipe: Recipe,
    /// Its catalog metadata: rating, label, keywords and descriptive fields.
    pub values: Values,
}

/// Exports `photo` to `target`, reporting progress from 0 to 1. Stops between
/// stages once `cancel` is set.
pub fn run(
    photo: Photo,
    settings: &ExportSettings,
    target: &Path,
    overwrite: bool,
    cancel: &AtomicBool,
    progress: impl Fn(f32),
) -> Result<()> {
    let cancelled = || -> Result<()> {
        ensure!(!cancel.load(Ordering::Relaxed), "Cancelled");
        Ok(())
    };
    progress(0.05);
    let image = full_size(photo.image.clone(), &photo.source, cancel)?;
    cancelled()?;
    progress(0.4);
    let options = settings.options();
    let rendered = crate::develop::render(&image, &photo.recipe, options.max_edge)?;
    cancelled()?;
    progress(0.85);
    let policy = Policy::of(settings);
    let file = policy
        .reads_file()
        .then(|| exif::read(&photo.source))
        .flatten();
    // LibRaw's capture settings stand in for EXIF that could not be read.
    let libraw = file.is_none().then(|| {
        let (main, exif) = super::metadata::from_metadata(&image.metadata);
        exif::CameraExif {
            main,
            exif,
            gps: Vec::new(),
        }
    });
    let assembled = assemble(policy, file.as_ref(), libraw, &photo.values);
    let xmp = assembled
        .xmp
        .as_ref()
        .map(|fields| xmp(&photo, &image, settings, fields));
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    super::export_with(
        target,
        &photo.source,
        &rendered,
        &image.metadata,
        &options,
        &Embed {
            camera: Some(assembled.exif),
            capture: true,
            location: true,
            camera_fallback: policy.camera,
            xmp,
            ppi: settings.ppi,
        },
        overwrite,
    )?;
    progress(1.);
    Ok(())
}

/// The full-resolution image: the open one, the decode cache's, or a new decode.
fn full_size(
    image: Arc<CameraImage>,
    source: &Path,
    cancel: &AtomicBool,
) -> Result<Arc<CameraImage>> {
    if !image.fast {
        return Ok(image);
    }
    let cached = DecodeCache::key(source)
        .ok()
        .and_then(|key| DecodeCache::default().load(&key, &image.metadata));
    Ok(Arc::new(match cached {
        Some(full) => full,
        None => Raw::open(source)?.develop(false, cancel)?,
    }))
}

fn xmp(
    photo: &Photo,
    image: &CameraImage,
    settings: &ExportSettings,
    fields: &super::assemble::XmpFields,
) -> String {
    crate::xmp::write::packet(
        &photo.recipe,
        &image.metadata,
        &crate::xmp::write::Photo {
            raw_name: photo
                .source
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            captured: fields.captured.clone(),
            created: fields.created.clone(),
            now: crate::time::now_xmp(),
            rating: fields.rating,
            label: fields.label.clone(),
            keywords: fields.keywords.clone(),
            title: fields.title.clone(),
            caption: fields.caption.clone(),
            rights: fields.rights.clone(),
            creators: fields.creators.clone(),
            lens: fields.lens,
            settings: fields.develop,
            format: settings.mime_type().into(),
        },
    )
}
