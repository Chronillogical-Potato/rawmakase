use super::{Event, Latest, LoadJob, LoadedHeader, Prefetch, TaskKind, send};
use crate::{
    decode_cache::DecodeCache,
    export::ExportOptions,
    raw::{Decode, Demosaic, thumbnail},
};
use eframe::egui;
use std::{
    sync::{Arc, atomic::Ordering, mpsc::Sender},
    time::Instant,
};
/// The slow second stage of opening a photo, run on its own worker so the
/// next photo's quick stage never waits behind it.
struct FullJob {
    id: u64,
    path: std::path::PathBuf,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    started: Instant,
    demosaic: Demosaic,
    /// Decode cache key of `path`, when its identity could be read.
    key: Option<String>,
    prefetch: Option<Prefetch>,
}
/// Runs `load`, turning a panic into an error, so the photo does not stay loading.
fn caught(load: impl FnOnce() -> anyhow::Result<()>) -> anyhow::Result<()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(load)).unwrap_or_else(|panic| {
        Err(anyhow::anyhow!(
            "Loading failed: {}",
            super::panic_message(&*panic)
        ))
    })
}
/// Develops a neighbour into the decode cache on two threads, leaving the other
/// cores to rendering the photo on screen.
fn prefetcher() -> Latest<Prefetch> {
    Latest::new(|job: Prefetch| {
        static POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
        let Some(pool) = POOL.get_or_init(|| crate::raw::background_pool(2, "prefetch").ok())
        else {
            return;
        };
        let cache = DecodeCache::default();
        let Ok(key) = DecodeCache::key(&job.path, job.demosaic) else {
            return;
        };
        if job.cancel.load(Ordering::Relaxed) || cache.contains(&key) {
            return;
        }
        let _ = pool.install(|| -> anyhow::Result<()> {
            let image =
                crate::photo::open(&job.path)?.develop(Decode::Full(job.demosaic), &job.cancel)?;
            crate::develop::quality::recovered(&image, &job.cancel)?;
            if !job.cancel.load(Ordering::Relaxed) {
                cache.store(&key, &image)?;
            }
            Ok(())
        });
    })
}
fn full_loader(
    tx: Sender<Event>,
    ctx: egui::Context,
    prefetcher: Arc<Latest<Prefetch>>,
) -> Latest<FullJob> {
    Latest::new(move |mut job: FullJob| {
        let prefetch = job.prefetch.take();
        let result = caught(|| -> anyhow::Result<()> {
            if job.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            {
                let decode = Decode::Full(job.demosaic);
                let image = Arc::new(crate::photo::open(&job.path)?.develop(decode, &job.cancel)?);
                // Recovered here rather than by the first render, so the cache holds it.
                crate::develop::quality::recovered(&image, &job.cancel)?;
                if job.cancel.load(Ordering::Relaxed) {
                    return Ok(());
                }
                send(
                    &tx,
                    &ctx,
                    Event::Ready {
                        id: job.id,
                        full: image.clone(),
                        status: format!("Developed in {:.2}s", job.started.elapsed().as_secs_f32()),
                    },
                );
                if let Some(key) = &job.key {
                    let _ = DecodeCache::default().store(key, &image);
                }
            }
            // Only now, so decoding the neighbour never slows the photo on screen.
            if let Some(prefetch) = prefetch {
                prefetcher.submit(prefetch);
            }
            Ok(())
        });
        if let Err(e) = result
            && !job.cancel.load(Ordering::Relaxed)
        {
            send(
                &tx,
                &ctx,
                Event::Failed {
                    id: job.id,
                    task: TaskKind::Load,
                    error: format!("{e:#}"),
                },
            );
        }
    })
}
pub fn loader(tx: Sender<Event>, ctx: egui::Context) -> Latest<LoadJob> {
    let prefetcher = Arc::new(prefetcher());
    let full = full_loader(tx.clone(), ctx.clone(), prefetcher.clone());
    Latest::new(move |mut job: LoadJob| {
        let prefetch = job.prefetch.take();
        let result = caught(|| -> anyhow::Result<()> {
            let path = job.path.clone();
            let mut raw = crate::photo::open(&path)?;
            let metadata = raw.metadata.clone();
            let (profiles, warnings) = crate::camera_profiles::installed(&metadata);
            // The raw defaults; the catalog's edit replaces them once the header
            // is installed.
            let recipe = job.defaults.resolve(&metadata, &profiles).recipe;
            send(
                &tx,
                &ctx,
                Event::Header(Box::new(LoadedHeader {
                    id: job.id,
                    path: path.clone(),
                    metadata,
                    recipe,
                    export: ExportOptions::default(),
                    protected: false,
                    status: "Original".into(),
                })),
            );
            send(
                &tx,
                &ctx,
                Event::Profiles {
                    id: job.id,
                    profiles,
                    errors: warnings,
                },
            );
            if let Ok(im) = thumbnail(&mut raw) {
                // Some cameras embed a full-size JPEG (60 MP on a Sony A7CR);
                // a screen-sized copy is all the placeholder needs.
                let edge = im.width().max(im.height());
                let im = if edge > 2560 {
                    let k = 2560. / edge as f32;
                    image::imageops::resize(
                        &im,
                        (im.width() as f32 * k) as u32,
                        (im.height() as f32 * k) as u32,
                        image::imageops::FilterType::Triangle,
                    )
                } else {
                    im
                };
                send(
                    &tx,
                    &ctx,
                    Event::Embedded {
                        id: job.id,
                        image: im,
                    },
                );
            }
            if job.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let t = Instant::now();
            let key = DecodeCache::key(&path, job.demosaic).ok();
            let cached = key
                .as_ref()
                .and_then(|key| DecodeCache::default().load(key, &raw.metadata));
            if let Some(image) = cached {
                send(
                    &tx,
                    &ctx,
                    Event::Ready {
                        id: job.id,
                        full: Arc::new(image),
                        status: format!("Opened from cache in {:.2}s", t.elapsed().as_secs_f32()),
                    },
                );
                if let Some(prefetch) = prefetch {
                    prefetcher.submit(prefetch);
                }
                return Ok(());
            }
            // Lightroom-style two stages: a half-size decode (about 0.2 s) makes
            // the photo editable at once; the full-resolution decode, which can
            // take seconds, then replaces it for 100% views and export.
            let quick = Arc::new(raw.develop(Decode::Half, &job.cancel)?);
            send(
                &tx,
                &ctx,
                Event::Ready {
                    id: job.id,
                    full: quick,
                    status: "Loading full resolution…".into(),
                },
            );
            full.submit(FullJob {
                id: job.id,
                path,
                cancel: job.cancel.clone(),
                started: t,
                demosaic: job.demosaic,
                key,
                prefetch,
            });
            Ok(())
        });
        if let Err(e) = result
            && !job.cancel.load(Ordering::Relaxed)
        {
            send(
                &tx,
                &ctx,
                Event::Failed {
                    id: job.id,
                    task: TaskKind::Load,
                    error: format!("{e:#}"),
                },
            );
        }
    })
}
