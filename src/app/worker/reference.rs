//! Develop's Reference View photo: another catalog photo, developed with its own
//! edit beside the one being edited. Its own worker, so loading it never holds up
//! the photo on screen; like opening a photo, a half-size decode comes first for
//! Fit and the full one follows for 100%.
use super::{Event, Latest, send};
use crate::{app::library::EditSource, decode_cache::DecodeCache, develop::Recipe, raw};
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
}
/// The reference photo, developed: the half-size decode first, then the full one.
pub struct ReferenceImage {
    pub image: Arc<raw::CameraImage>,
    pub recipe: Recipe,
    pub resolution: Resolution,
}
/// How much of the photo's resolution an image has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
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
            develop(&job, |image| reply(Ok(Box::new(image))))
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
fn develop(job: &ReferenceJob, mut ready: impl FnMut(ReferenceImage)) -> anyhow::Result<()> {
    let raw = raw::Raw::open(&job.path)?;
    let recipe = EditSource::recipe(Some(&job.edit), &raw)?;
    let key = DecodeCache::key(&job.path).ok();
    let cached = key
        .as_ref()
        .and_then(|key| DecodeCache::default().load(key, &raw.metadata));
    if let Some(image) = cached {
        ready(ReferenceImage {
            image: Arc::new(image),
            recipe,
            resolution: Resolution::Full,
        });
        return Ok(());
    }
    let half = raw.develop(true, &job.cancel)?;
    if job.cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    ready(ReferenceImage {
        image: Arc::new(half),
        recipe: recipe.clone(),
        resolution: Resolution::Half,
    });
    let full = Arc::new(raw::Raw::open(&job.path)?.develop(false, &job.cancel)?);
    crate::develop::quality::recovered(&full, &job.cancel)?;
    if job.cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    if let Some(key) = &key {
        let _ = DecodeCache::default().store(key, &full);
    }
    ready(ReferenceImage {
        image: full,
        recipe,
        resolution: Resolution::Full,
    });
    Ok(())
}
