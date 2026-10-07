//! One photo's export from start to finish: the full-size image (decoded here when
//! only a quick preview is loaded), the render, its metadata and the file.
use super::{
    Embed,
    assemble::{Policy, Values, assemble, copyright},
};
use crate::export_settings::ExportSettings;
use crate::{
    camera_data::{CameraImage, Demosaic},
    decode::{DecodePolicy, FullSize},
    exif,
    model::recipe::Recipe,
    raw::Raw,
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
    /// The watermark chosen in the Export dialog, a preset or the Simple
    /// Copyright Watermark.
    pub watermark: Option<crate::watermark::Watermark>,
    /// The demosaic for a full-size decode, if `image` is a draft.
    pub demosaic: Demosaic,
}

/// Exports `photo` to `target`, reporting progress from 0 to 1. Stops between
/// stages once `cancel` is set. Returns what the export has to say besides
/// "Exported", such as a Simple Copyright Watermark left out for want of a
/// copyright.
pub fn run(
    photo: Photo,
    settings: &ExportSettings,
    target: &Path,
    replace: super::Replace,
    cancel: &AtomicBool,
    progress: impl Fn(f32),
) -> Result<Option<String>> {
    let prepared = prepare(&photo, settings, cancel, &progress)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    super::export_with(
        target,
        &photo.source,
        &prepared.rendered,
        &prepared.metadata,
        &prepared.options,
        &prepared.embed,
        replace,
    )?;
    progress(1.);
    Ok(prepared.notice)
}

/// A photo rendered for export, with what goes in the file beside its pixels.
pub struct Prepared {
    pub rendered: crate::develop::Rendered,
    pub metadata: crate::camera_data::Metadata,
    pub options: crate::export_settings::ExportOptions,
    pub embed: Embed,
    /// What the export has to say besides "Exported".
    pub notice: Option<String>,
}

/// Renders `photo` for an export with `settings`, reporting progress up to 0.85.
/// Stops between stages once `cancel` is set.
pub fn prepare(
    photo: &Photo,
    settings: &ExportSettings,
    cancel: &AtomicBool,
    progress: &impl Fn(f32),
) -> Result<Prepared> {
    let cancelled = || -> Result<()> {
        ensure!(!cancel.load(Ordering::Relaxed), "Cancelled");
        Ok(())
    };
    progress(0.05);
    let policy = Policy::of(settings);
    let simple_copyright = matches!(
        &photo.watermark,
        Some(w) if w.name == crate::watermark::SIMPLE_COPYRIGHT
    );
    // Read once: for the export's metadata and the Simple Copyright
    // Watermark.
    let file = (policy.reads_file() || simple_copyright)
        .then(|| exif::read(&photo.source))
        .flatten();
    // Loaded first: a missing image or font fails the export before it
    // renders.
    let mut notice = None;
    let watermark = match &photo.watermark {
        Some(_) if simple_copyright => match copyright(&photo.values, file.as_ref()) {
            Some(text) => Some(crate::watermark::Watermark::simple_copyright(&text).ready()?),
            None => {
                notice = Some("no copyright in the file for the Simple Copyright Watermark".into());
                None
            }
        },
        Some(w) => Some(w.ready()?),
        None => None,
    };
    let image = full_size(photo.image.clone(), &photo.source, photo.demosaic, cancel)?;
    cancelled()?;
    progress(0.4);
    let options = settings.options();
    let mut rendered = crate::develop::render(&image, &photo.recipe.checked()?, options.max_edge)?;
    if let Some(w) = &watermark
        && !w.apply(&mut rendered)
    {
        notice = Some("the watermark's text has no characters its font can draw".into());
    }
    cancelled()?;
    progress(0.85);
    let file = file.filter(|_| policy.reads_file());
    // LibRaw's capture settings stand in for EXIF that could not be read.
    let libraw = file
        .is_none()
        .then(|| exif::CameraExif::from_libraw(&image.metadata));
    let assembled = assemble(policy, file.as_ref(), libraw, &photo.values);
    let xmp = assembled
        .xmp
        .as_ref()
        .map(|fields| xmp(photo, &image, settings, fields));
    Ok(Prepared {
        rendered,
        metadata: image.metadata.clone(),
        options,
        embed: Embed {
            camera: Some(assembled.exif),
            camera_fallback: policy.camera,
            xmp,
            ppi: settings.ppi,
        },
        notice,
    })
}

/// `raw` at full resolution, as Develop decodes the photo it opens: the decode
/// cache's copy when it has one.
pub fn decode_full(
    raw: Raw,
    source: &Path,
    demosaic: Demosaic,
    cancel: &AtomicBool,
) -> Result<CameraImage> {
    let full = FullSize::new(source, demosaic);
    match full.cached(&raw.metadata) {
        Some(image) => Ok(image),
        None => full.decode(raw, DecodePolicy::Export, cancel),
    }
}

/// The full-resolution image: the open one, the decode cache's, or a new decode.
fn full_size(
    image: Arc<CameraImage>,
    source: &Path,
    demosaic: Demosaic,
    cancel: &AtomicBool,
) -> Result<Arc<CameraImage>> {
    if !image.fast {
        return Ok(image);
    }
    let full = FullSize::new(source, demosaic);
    Ok(Arc::new(match full.cached(&image.metadata) {
        Some(cached) => cached,
        None => full.decode(crate::photo::open(source)?, DecodePolicy::Export, cancel)?,
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
            crop: Some(crate::develop::rendered_crop(
                &photo.recipe,
                &image.metadata,
            )),
        },
    )
}
