//! Develop's Reference View photo: another catalog photo, developed with its own
//! edit beside the one being edited. Its own worker, so loading it never holds up
//! the photo on screen; like opening a photo, a half-size decode comes first for
//! Fit and the full one follows for 100%.
use super::{Event, Latest, send};
use crate::{
    app::library::EditSource,
    camera_data,
    decode::{DecodePolicy, FullSize},
    decode_cache::DecodeCache,
    model::recipe::Recipe,
};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

/// A reference photo to develop.
pub(in crate::app) struct ReferenceJob {
    /// Matches the results to this request.
    pub ticket: u64,
    pub path: PathBuf,
    pub edit: EditSource,
    pub cancel: Arc<AtomicBool>,
    /// The demosaic of the full-size decode and of its decode-cache key.
    pub demosaic: camera_data::Demosaic,
}
/// The reference photo, developed: the half-size decode first, then the full one.
pub(crate) struct ReferenceImage {
    pub image: Arc<camera_data::CameraImage>,
    pub recipe: Recipe,
    pub resolution: Resolution,
}
/// How much of the photo's resolution an image has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// The half-size decode: enough for Fit.
    Half,
    /// Every pixel, for 100%.
    Full,
}

pub(in crate::app) fn reference_loader(
    tx: Sender<Event>,
    ctx: egui::Context,
) -> Latest<ReferenceJob> {
    Latest::new(move |job: ReferenceJob| {
        let ticket = job.ticket;
        let cancel = job.cancel.clone();
        let reply = |result: Result<Box<ReferenceImage>, String>| {
            if !cancel.load(Ordering::Relaxed) {
                send(&tx, &ctx, Event::Reference { ticket, result });
            }
        };
        let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            develop(&job, &DecodeCache::default(), |image| {
                reply(Ok(Box::new(image)))
            })
        }));
        match loaded {
            Ok(Ok(())) => {}
            Ok(Err(e)) => reply(Err(format!("{e:#}"))),
            Err(panic) => reply(Err(format!(
                "Loading failed: {}",
                super::panic_message(&*panic)
            ))),
        }
    })
}

/// Develops the job's photo, handing each stage to `ready`.
fn develop(
    job: &ReferenceJob,
    cache: &DecodeCache,
    mut ready: impl FnMut(ReferenceImage),
) -> anyhow::Result<()> {
    let raw = crate::photo::open(&job.path)?;
    let recipe = EditSource::recipe(Some(&job.edit), &raw)?;
    let full_size = FullSize::with_cache(&job.path, job.demosaic, cache.clone());
    if let Some(image) = full_size.cached(&raw.metadata) {
        // Closed before the reference is ready: Windows refuses to rename or
        // replace a file LibRaw still has open.
        drop(raw);
        ready(ReferenceImage {
            image: Arc::new(image),
            recipe,
            resolution: Resolution::Full,
        });
        return Ok(());
    }
    let half = raw.develop(camera_data::Decode::Half, &job.cancel)?;
    if job.cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    ready(ReferenceImage {
        image: Arc::new(half),
        recipe: recipe.clone(),
        resolution: Resolution::Half,
    });
    let opened = crate::photo::open(&job.path)?;
    let full = Arc::new(full_size.decode(opened, DecodePolicy::Show, &job.cancel)?);
    if job.cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    full_size.store(&full, &job.cancel);
    ready(ReferenceImage {
        image: full,
        recipe,
        resolution: Resolution::Full,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_data::Demosaic;

    /// The preference is never changed here: the job alone decides which
    /// demosaic is decoded, and the cache entry is keyed with that one.
    #[test]
    fn the_full_decode_is_cached_under_the_jobs_demosaic() -> anyhow::Result<()> {
        let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus/charts/synthetic-d65.dng");
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().to_path_buf(), u64::MAX);
        let key = |demosaic| DecodeCache::key(&chart, demosaic);
        let job = |demosaic| ReferenceJob {
            ticket: 1,
            path: chart.clone(),
            edit: EditSource::Defaults(Default::default()),
            cancel: Default::default(),
            demosaic,
        };
        let develop_stages = |job: &ReferenceJob| -> anyhow::Result<Vec<Resolution>> {
            let mut stages = Vec::new();
            develop(job, &cache, |image| stages.push(image.resolution))?;
            Ok(stages)
        };

        let preferred = Demosaic::default().effective();
        let other = match preferred {
            Demosaic::Rawmakase => Demosaic::Libraw,
            Demosaic::Libraw => Demosaic::Rawmakase,
        };
        assert_eq!(
            develop_stages(&job(other))?,
            [Resolution::Half, Resolution::Full]
        );
        assert!(cache.contains(&key(other)?));
        assert!(!cache.contains(&key(preferred)?));

        // The next job with that demosaic opens from the cache.
        assert_eq!(develop_stages(&job(other))?, [Resolution::Full]);
        Ok(())
    }

    /// Once the reference is ready its file can be renamed: Windows refuses
    /// while LibRaw still has it open.
    #[test]
    fn the_file_is_closed_when_the_reference_is_ready() -> anyhow::Result<()> {
        let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus/charts/synthetic-d65.dng");
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("photo.dng");
        std::fs::copy(chart, &path)?;
        let cache = DecodeCache::new(dir.path().join("cache"), u64::MAX);
        let job = ReferenceJob {
            ticket: 1,
            path: path.clone(),
            edit: EditSource::Defaults(Default::default()),
            cancel: Default::default(),
            demosaic: Demosaic::default().effective(),
        };
        let away = path.with_extension("away");
        // Developed, then opened from the cache.
        for _ in 0..2 {
            let mut renamed = Vec::new();
            develop(&job, &cache, |image| {
                if image.resolution == Resolution::Full {
                    renamed.push(
                        std::fs::rename(&path, &away).and_then(|()| std::fs::rename(&away, &path)),
                    );
                }
            })?;
            assert_eq!(renamed.len(), 1);
            renamed.pop().unwrap()?;
        }
        Ok(())
    }
}
