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
    decode_cache::DecodeCache,
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
    /// Where a full-size decode is looked up first.
    pub cache: DecodeCache,
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
    pub rendered: crate::rendered::Rendered,
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
        .then(|| crate::photo::camera_exif(&photo.source))
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
    let image = full_size(
        photo.image.clone(),
        &photo.source,
        photo.demosaic,
        &photo.cache,
        cancel,
    )?;
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
    let libraw = file
        .is_none()
        .then(|| stand_in(&photo.source, &image.metadata));
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

/// What stands in for the camera's EXIF of `source`, which has none that
/// can be read: the decoder's capture settings in `metadata`, with the
/// dates a RAW container's own records give (a CRW's TimeStamp).
pub fn stand_in(source: &Path, metadata: &crate::camera_data::Metadata) -> exif::CameraExif {
    let mut stand_in = exif::CameraExif::from_libraw(metadata);
    stand_in.exif.extend(crate::raw::capture_date_tags(source));
    stand_in
}

/// `raw` at full resolution, as Develop decodes the photo it opens: the decode
/// cache's copy when it has one.
pub fn decode_full(
    raw: Raw,
    source: &Path,
    demosaic: Demosaic,
    cache: &DecodeCache,
    cancel: &AtomicBool,
) -> Result<CameraImage> {
    let full = FullSize::with_cache(source, demosaic, cache.clone());
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
    cache: &DecodeCache,
    cancel: &AtomicBool,
) -> Result<Arc<CameraImage>> {
    if !image.fast {
        return Ok(image);
    }
    let full = FullSize::with_cache(source, demosaic, cache.clone());
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
            creator_tool: crate::build_info::SOFTWARE.into(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exif::{CameraExif, Field, tag};

    /// The photo at `source`, its image decoded already, as Develop has it.
    fn photo(source: &Path, cache: &Path) -> Photo {
        let metadata = crate::camera_data::Metadata {
            make: "Canon".into(),
            model: "EOS 300D".into(),
            shutter: 0.01,
            aperture: 4.,
            iso: 200.,
            focal: 35.,
            ..Default::default()
        };
        Photo {
            image: Arc::new(CameraImage {
                width: 4,
                height: 2,
                recovered: Default::default(),
                pixels: vec![[0.2; 3]; 8],
                metadata,
                fast: false,
                scale_factor: 1.,
                scale_clipped: 0,
            }),
            source: source.to_path_buf(),
            recipe: Recipe::default(),
            values: Default::default(),
            watermark: None,
            demosaic: Demosaic::default(),
            cache: DecodeCache::new(cache.to_path_buf(), u64::MAX),
        }
    }

    /// The EXIF of `photo`'s JPEG export.
    fn exported(photo: Photo, dir: &Path) -> Result<CameraExif> {
        let settings = ExportSettings {
            include: crate::export_settings::Include::AllExceptCameraRaw,
            ..Default::default()
        };
        let target = dir.join("out.jpg");
        let _ = std::fs::remove_file(&target);
        let cancel = AtomicBool::new(false);
        run(
            photo,
            &settings,
            &target,
            super::super::Replace::NoClobber,
            &cancel,
            |_| {},
        )?;
        crate::exif::read(&target).ok_or_else(|| anyhow::anyhow!("no EXIF in the export"))
    }

    fn text(exif: &CameraExif, tag: u16) -> Option<String> {
        exif.main
            .iter()
            .chain(&exif.exif)
            .chain(&exif.gps)
            .find(|f| f.tag == tag)
            .and_then(Field::text)
    }

    /// A CR3's and an MRW's camera EXIF go with their exports as a CR2's
    /// does: the creator, copyright and description, the dates, the
    /// serial numbers, the exposure program and GPS (#671).
    #[test]
    fn a_cr3s_and_an_mrws_camera_exif_go_with_their_exports() -> Result<()> {
        use super::super::exif::{Container, camera_raw};
        let camera = CameraExif {
            main: vec![
                Field::ascii(tag::IMAGE_DESCRIPTION, "Harbour at dusk"),
                Field::ascii(tag::MAKE, "Canon"),
                Field::ascii(tag::MODEL, "Canon EOS R5m2"),
                Field::ascii(tag::ARTIST, "Ann Example"),
                Field::ascii(tag::COPYRIGHT, "Ann Example 2024"),
            ],
            exif: vec![
                Field::short(0x8822, 1),
                Field::ascii(tag::DATE_TIME_ORIGINAL, "2024:08:20 15:24:34"),
                Field::ascii(tag::DATE_TIME_DIGITIZED, "2024:08:20 15:24:35"),
                Field::ascii(tag::OFFSET_TIME_ORIGINAL, "+02:00"),
                Field::ascii(0xa431, "143302103530"),
                Field::ascii(tag::LENS_MODEL, "RF135mm F1.8 L IS USM"),
                Field::ascii(tag::MAKER_NOTE, "maker note"),
            ],
            gps: vec![
                Field::ascii(tag::GPS_LATITUDE_REF, "N"),
                Field::rational(tag::GPS_LATITUDE, 52, 1),
            ],
        };
        let dir = tempfile::tempdir()?;
        for (container, name) in [(Container::Cr3, "a.CR3"), (Container::Mrw, "a.MRW")] {
            let source = dir.path().join(name);
            std::fs::write(&source, camera_raw(container, camera.clone()))?;
            let out = exported(photo(&source, dir.path()), dir.path())?;
            for (tag, value) in [
                (tag::IMAGE_DESCRIPTION, "Harbour at dusk"),
                (tag::MODEL, "Canon EOS R5m2"),
                (tag::ARTIST, "Ann Example"),
                (tag::COPYRIGHT, "Ann Example 2024"),
                (tag::DATE_TIME_ORIGINAL, "2024:08:20 15:24:34"),
                (tag::DATE_TIME_DIGITIZED, "2024:08:20 15:24:35"),
                (tag::OFFSET_TIME_ORIGINAL, "+02:00"),
                (0xa431, "143302103530"),
                (tag::LENS_MODEL, "RF135mm F1.8 L IS USM"),
                (tag::GPS_LATITUDE_REF, "N"),
            ] {
                assert_eq!(text(&out, tag).as_deref(), Some(value), "{name} {tag:#x}");
            }
            let program = out.exif.iter().find(|f| f.tag == 0x8822);
            assert_eq!(program.and_then(Field::number), Some(1.), "{name}");
            assert_eq!(out.gps.len(), 2, "{name}");
            // The maker note's offsets point into the RAW.
            assert!(out.get(tag::MAKER_NOTE).is_none(), "{name}");
        }
        Ok(())
    }

    /// A CRW has no EXIF: its export carries the decoder's capture settings
    /// and the date of its TimeStamp record (#671).
    #[test]
    fn a_crws_export_is_dated_by_its_time_stamp() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("a.CRW");
        // 2016-12-23 17:05:30 on the camera's clock.
        std::fs::write(&source, super::super::exif::dated_crw(1_482_512_730))?;
        let out = exported(photo(&source, dir.path()), dir.path())?;
        assert_eq!(
            text(&out, tag::DATE_TIME_ORIGINAL).as_deref(),
            Some("2016:12:23 17:05:30")
        );
        assert_eq!(text(&out, tag::MODEL).as_deref(), Some("EOS 300D"));
        let f_number = out.exif.iter().find(|f| f.tag == tag::F_NUMBER);
        assert_eq!(f_number.and_then(Field::number), Some(4.));
        Ok(())
    }

    /// Export reads the full-size image from the cache it is given, which
    /// follows the folder chosen in Preferences, not the default directory.
    #[test]
    fn a_full_size_decode_reads_the_given_cache() -> Result<()> {
        let chart = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/charts/synthetic-d65.dng");
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().to_path_buf(), u64::MAX);
        let raw = crate::photo::open(&chart)?;
        let cached = CameraImage {
            width: 4,
            height: 3,
            recovered: Default::default(),
            pixels: vec![[0.5; 3]; 12],
            metadata: raw.metadata.clone(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        };
        let _ = cached.recovered.set(Arc::new(cached.clone()));
        cache.store(&DecodeCache::key(&chart, Demosaic::default())?, &cached)?;
        let image = decode_full(
            raw,
            &chart,
            Demosaic::default(),
            &cache,
            &AtomicBool::new(false),
        )?;
        assert_eq!((image.width, image.height), (4, 3));
        Ok(())
    }
}
