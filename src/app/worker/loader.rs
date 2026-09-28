use super::{Event, Latest, LoadJob, LoadedHeader, TaskKind, send};
use crate::{
    decode_cache::DecodeCache,
    develop::Recipe,
    export::ExportOptions,
    raw::{self, thumbnail},
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
    /// Decode cache key of `path`, when its identity could be read.
    key: Option<String>,
}
fn full_loader(tx: Sender<Event>, ctx: egui::Context) -> Latest<FullJob> {
    Latest::new(move |job: FullJob| {
        let result = (|| -> anyhow::Result<()> {
            if job.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            {
                let image = Arc::new(raw::Raw::open(&job.path)?.develop(false, &job.cancel)?);
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
            Ok(())
        })();
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
    let full = full_loader(tx.clone(), ctx.clone());
    Latest::new(move |job: LoadJob| {
        let result = (|| -> anyhow::Result<()> {
            let path = job.path.clone();
            let mut raw = raw::Raw::open(&path)?;
            let metadata = raw.metadata.clone();
            let (profiles, warnings) = crate::camera_profiles::installed(&metadata);
            // The catalog's edit replaces this once the header is installed.
            let recipe = Recipe::with_profiles(&metadata, &profiles);
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
            let key = DecodeCache::key(&path).ok();
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
                return Ok(());
            }
            // Lightroom-style two stages: a half-size decode (about 0.2 s) makes
            // the photo editable at once; the full-resolution decode, which can
            // take seconds, then replaces it for 100% views and export.
            let quick = Arc::new(raw.develop(true, &job.cancel)?);
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
                key,
            });
            Ok(())
        })();
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
