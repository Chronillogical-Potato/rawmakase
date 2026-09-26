use super::{Event, Latest, LoadJob, LoadedHeader, TaskKind, send};
use crate::{
    develop::{self, Recipe},
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
    files: Vec<std::path::PathBuf>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    started: Instant,
}
fn full_loader(tx: Sender<Event>, ctx: egui::Context) -> Latest<FullJob> {
    Latest::new(move |job: FullJob| {
        let result = (|| -> anyhow::Result<()> {
            if job.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let image = Arc::new(raw::Raw::open(&job.path)?.develop(false, &job.cancel)?);
            if job.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let preview = Arc::new(develop::preview(&image, super::DRAFT_EDGE));
            send(
                &tx,
                &ctx,
                Event::Ready {
                    id: job.id,
                    full: image,
                    draft: preview,
                    status: format!("Developed in {:.2}s", job.started.elapsed().as_secs_f32()),
                },
            );
            let pos = job.files.iter().position(|p| p == &job.path).unwrap_or(0);
            let start = pos.saturating_sub(16);
            for file in job.files.iter().skip(start).take(32) {
                if job.cancel.load(Ordering::Relaxed) {
                    break;
                }
                if let Ok(im) = raw::Raw::open(file).and_then(|mut r| thumbnail(&mut r)) {
                    let small = image::imageops::thumbnail(&im, 120, 72);
                    send(
                        &tx,
                        &ctx,
                        Event::Thumbnail {
                            id: job.id,
                            path: file.clone(),
                            image: small,
                        },
                    );
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
            let files = if job.catalog {
                vec![job.path.clone()]
            } else {
                crate::storage::list_raws(&job.path)?
            };
            let path = if job.path.is_dir() {
                files
                    .first()
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("No RAW photos in this folder"))?
            } else {
                job.path.clone()
            };
            let mut raw = raw::Raw::open(&path)?;
            let metadata = raw.metadata.clone();
            let (profiles, warnings) = crate::camera_profiles::installed(&metadata);
            let (recipe, export, protected, status) = match if job.catalog {
                Ok(None)
            } else {
                crate::storage::load(&path)
            } {
                Ok(Some(s)) => (s.recipe, s.export, false, "Edits restored".into()),
                Ok(None) => (
                    Recipe::with_profiles(&metadata, &profiles),
                    ExportOptions::default(),
                    false,
                    "Original".into(),
                ),
                Err(e) => (
                    Recipe::with_profiles(&metadata, &profiles),
                    ExportOptions::default(),
                    true,
                    format!("Sidecar protected: {e}"),
                ),
            };
            send(
                &tx,
                &ctx,
                Event::Header(Box::new(LoadedHeader {
                    id: job.id,
                    path: path.clone(),
                    metadata,
                    recipe,
                    export,
                    protected,
                    status,
                    files: files.clone(),
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
            // Lightroom-style two stages: a half-size decode (about 0.2 s) makes
            // the photo editable at once; the full-resolution decode, which can
            // take seconds, then replaces it for 100% views and export.
            let t = Instant::now();
            let quick = Arc::new(raw.develop(true, &job.cancel)?);
            send(
                &tx,
                &ctx,
                Event::Ready {
                    id: job.id,
                    draft: Arc::new(develop::preview(&quick, super::DRAFT_EDGE)),
                    full: quick,
                    status: "Loading full resolution…".into(),
                },
            );
            full.submit(FullJob {
                id: job.id,
                path,
                files,
                cancel: job.cancel.clone(),
                started: t,
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
