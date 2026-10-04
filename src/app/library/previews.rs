//! Bounded, asynchronous disk-cache work and its UI progress.
use crate::app::widgets::plural;
use crate::catalog::preview_cache::{PreviewCache, Stamp};
use eframe::egui;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
};

pub(super) struct PreviewResult {
    pub path: PathBuf,
    pub image: Option<image::RgbImage>,
    pub cache_error: Option<String>,
}

pub(super) fn spawn(
    cache_path: PathBuf,
    ctx: egui::Context,
) -> (SyncSender<PathBuf>, Receiver<PreviewResult>) {
    spawn_with(cache_path, ctx, super::thumbnail)
}
fn spawn_with(
    cache_path: PathBuf,
    ctx: egui::Context,
    thumbnail: fn(&Path) -> anyhow::Result<image::RgbImage>,
) -> (SyncSender<PathBuf>, Receiver<PreviewResult>) {
    let (tx, rx) = mpsc::sync_channel::<PathBuf>(24);
    let (result_tx, result_rx) = mpsc::sync_channel(24);
    std::thread::spawn(move || {
        let open = || match PreviewCache::open(&cache_path) {
            Ok(cache) => (Some(cache), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let (mut cache, mut open_error) = open();
        while let Ok(path) = rx.recv() {
            let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut cache_error = open_error.clone();
                let cached = cache.as_ref().and_then(|cache| match cache.load(&path) {
                    Ok(image) => image,
                    Err(error) => {
                        cache_error = Some(error.to_string());
                        None
                    }
                });
                let image = cached.or_else(|| {
                    let stamp = Stamp::read(&path).ok()?;
                    let image = thumbnail(&path).ok()?;
                    if let Some(cache) = &mut cache
                        && let Err(error) = cache.store(&path, &stamp, &image)
                    {
                        cache_error = Some(error.to_string());
                    }
                    Some(image)
                });
                (image, cache_error)
            }));
            // After a panic the preview is unavailable, and the cache, which it may
            // have left mid-write, is opened again for the next one.
            let (image, cache_error) = prepared.unwrap_or_else(|_| {
                (cache, open_error) = open();
                (None, None)
            });
            if result_tx
                .send(PreviewResult {
                    path,
                    image,
                    cache_error,
                })
                .is_err()
            {
                break;
            }
            ctx.request_repaint();
        }
    });
    (tx, result_rx)
}

/// What an edited preview is rendered from.
#[derive(Clone)]
pub(super) enum EditSource {
    /// A saved RAWmakase recipe, as JSON.
    Recipe(String),
    /// Lightroom develop settings from an imported catalog.
    Lightroom(String),
    /// No edit: the raw defaults, as Develop would open the photo.
    Defaults(Arc<crate::develop::defaults::DevelopDefaults>),
}
impl EditSource {
    /// The recipe a photo is rendered with: its edit, or the defaults
    /// Develop would open it with (Adobe Default unless given).
    pub fn recipe(
        edit: Option<&Self>,
        raw: &crate::raw::Raw,
    ) -> anyhow::Result<crate::develop::Recipe> {
        let m = &raw.metadata;
        let (profiles, _) = crate::camera_profiles::installed(m);
        Ok(match edit {
            Some(Self::Recipe(json)) => serde_json::from_str(json)?,
            Some(Self::Lightroom(text)) => {
                crate::catalog::convert_develop(text, m, &profiles, None)?.0
            }
            Some(Self::Defaults(defaults)) => defaults.resolve(m, &profiles).recipe,
            None => crate::develop::Recipe::with_profiles(m, &profiles),
        })
    }
    /// Identifies this edit in the preview cache.
    pub fn tag(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match self {
            Self::Recipe(text) => ("recipe", text).hash(&mut h),
            Self::Lightroom(text) => ("lightroom", text).hash(&mut h),
            Self::Defaults(defaults) => ("defaults", format!("{defaults:?}")).hash(&mut h),
        }
        format!("edit-{:016x}", h.finish())
    }
}
pub(super) enum EditJob {
    /// Render (or load from cache) catalog photo `id` with its edit. Virtual
    /// copies share a file but not an edit, so results go by photo.
    Render {
        id: i64,
        /// Matches the result to this request, not to a later photo that
        /// reused a removed copy's id.
        ticket: u64,
        path: PathBuf,
        source: EditSource,
    },
    /// Keep an already rendered preview, e.g. from Develop.
    Store {
        path: PathBuf,
        tag: String,
        image: image::RgbImage,
    },
}
/// Photos shown in the grid or filmstrip in the last frame. Edited previews
/// are rendered only for these, so scrolling past photos leaves no backlog.
pub(super) type Wanted = Arc<Mutex<HashSet<i64>>>;
/// What an edited preview job came to.
pub(super) enum EditResult {
    Ready(i64, u64, image::RgbImage),
    /// The photo scrolled out of view first; request it again when shown.
    Skipped(i64, u64),
    Failed,
    /// A preview could not be kept in the cache; it comes besides any result.
    CacheError(String),
}
/// Edited previews on their own worker, so slow renders never delay the
/// embedded previews that fill the grid first. The latest request goes
/// first, and renders run on two threads so browsing stays responsive.
pub(super) fn spawn_edited(
    cache_path: PathBuf,
    wanted: Wanted,
    ctx: egui::Context,
) -> (mpsc::Sender<EditJob>, Receiver<EditResult>) {
    spawn_edited_with(cache_path, wanted, ctx, render_edited)
}
fn spawn_edited_with(
    cache_path: PathBuf,
    wanted: Wanted,
    ctx: egui::Context,
    render_edited: fn(&Path, &EditSource) -> anyhow::Result<image::RgbImage>,
) -> (mpsc::Sender<EditJob>, Receiver<EditResult>) {
    let (tx, rx) = mpsc::channel::<EditJob>();
    let (result_tx, result_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .thread_name(|i| format!("edited-preview-{i}"))
            .build()
            .ok();
        let mut cache = PreviewCache::open(&cache_path).ok();
        let mut queue = Vec::new();
        loop {
            if queue.is_empty() {
                match rx.recv() {
                    Ok(job) => queue.push(job),
                    Err(_) => break,
                }
            }
            queue.extend(rx.try_iter());
            // Stores first: they are cheap and keep Develop's renders.
            let job = match queue
                .iter()
                .position(|j| matches!(j, EditJob::Store { .. }))
            {
                Some(i) => queue.remove(i),
                None => queue.pop().unwrap(),
            };
            let store = matches!(job, EditJob::Store { .. });
            let mut cache_error = None;
            let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
                EditJob::Store { path, tag, image } => {
                    if let (Some(cache), Ok(stamp)) = (&mut cache, Stamp::read(&path))
                        && let Err(error) = cache.store_tagged(&path, &tag, &stamp, &image)
                    {
                        cache_error = Some(error.to_string());
                    }
                    None
                }
                EditJob::Render { id, ticket, .. } if !wanted.lock().unwrap().contains(&id) => {
                    Some(EditResult::Skipped(id, ticket))
                }
                EditJob::Render {
                    id,
                    ticket,
                    path,
                    source,
                } => {
                    let tag = source.tag();
                    let cached = cache
                        .as_ref()
                        .and_then(|c| c.load_tagged(&path, &tag).ok().flatten());
                    let image = cached.or_else(|| {
                        let stamp = Stamp::read(&path).ok()?;
                        let render = || render_edited(&path, &source);
                        let image = match &pool {
                            Some(pool) => pool.install(render),
                            None => render(),
                        }
                        .ok()?;
                        if let Some(cache) = &mut cache
                            && let Err(error) = cache.store_tagged(&path, &tag, &stamp, &image)
                        {
                            cache_error = Some(error.to_string());
                        }
                        Some(image)
                    });
                    Some(match image {
                        Some(image) => EditResult::Ready(id, ticket, image),
                        None => EditResult::Failed,
                    })
                }
            }));
            // After a panic the cache, which it may have left mid-write, is opened again.
            let result = done.unwrap_or_else(|_| {
                cache = PreviewCache::open(&cache_path).ok();
                (!store).then_some(EditResult::Failed)
            });
            let results = cache_error
                .map(EditResult::CacheError)
                .into_iter()
                .chain(result);
            if results.map(|r| result_tx.send(r)).any(|sent| sent.is_err()) {
                break;
            }
            ctx.request_repaint();
        }
    });
    (tx, result_rx)
}
/// A 640 px preview of `path` developed with `source`, from the fast
/// half-size decode.
fn render_edited(path: &Path, source: &EditSource) -> anyhow::Result<image::RgbImage> {
    let raw = crate::raw::Raw::open(path)?;
    let recipe = EditSource::recipe(Some(source), &raw)?;
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let image = raw.develop(true, &cancel)?;
    let out = crate::develop::render(&image, &recipe, 640)?;
    image::RgbImage::from_raw(out.width, out.height, out.rgb8())
        .ok_or_else(|| anyhow::anyhow!("Invalid preview size"))
}

#[derive(Default)]
pub(super) struct Progress {
    total: usize,
    completed: usize,
    failed: usize,
    cache_error: Option<String>,
}

impl Progress {
    pub fn queued(&mut self) {
        if self.completed == self.total {
            *self = Self::default();
        }
        self.total += 1;
    }

    pub fn finish(&mut self, result: &PreviewResult) {
        self.completed += 1;
        self.failed += usize::from(result.image.is_none());
        if let Some(error) = &result.cache_error {
            self.cache_error = Some(error.clone());
        }
    }

    /// A preview could not be kept in the cache.
    pub fn cache_failed(&mut self, error: String) {
        self.cache_error = Some(error);
    }

    /// Worth a status line: still working, or something could not be prepared.
    pub fn active(&self) -> bool {
        self.completed < self.total || self.failed > 0 || self.cache_error.is_some()
    }
    /// One line of small text, the same height as the status row it sits in.
    /// In a right-to-left layout, so the spinner comes first and sits right.
    pub fn show(&self, ui: &mut egui::Ui, edits_pending: usize) {
        let building = self.completed < self.total || edits_pending > 0;
        if building {
            ui.add(egui::Spinner::new().size(11.));
        }
        if edits_pending > 0 {
            ui.small(format!(
                "Rendering {}",
                plural(edits_pending, "edited preview", "edited previews")
            ))
            .on_hover_text(
                "Photos with Lightroom or RAWmakase edits are rendered with them, \
                 visible ones first. Each is kept, so this happens once.",
            );
        }
        if self.completed < self.total {
            ui.small(format!("Preparing previews {} / {}", self.completed, self.total))
                .on_hover_text("Cached previews load first; missing ones are built in the background as you browse.");
        }
        if self.failed > 0 {
            ui.small(format!("{} previews unavailable", self.failed))
                .on_hover_text(
                    "The original may be offline, damaged, or have no usable embedded preview.",
                );
        }
        if let Some(error) = &self.cache_error {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                egui::RichText::new("Preview cache could not be updated").small(),
            )
            .on_hover_text(error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn worker_persists_previews_and_reuses_them_when_original_is_offline() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("photo.png");
        image::RgbImage::new(720, 480).save(&source)?;
        let cache_path = directory.path().join("previews.sqlite3");
        let (tx, rx) = spawn(cache_path.clone(), egui::Context::default());
        tx.try_send(source.clone())?;
        let result = rx.recv_timeout(Duration::from_secs(10))?;
        assert_eq!(result.image.unwrap().dimensions(), (640, 427));
        assert!(result.cache_error.is_none());
        // Reopen through another worker: a memory-only result cannot pass this.
        std::fs::remove_file(&source)?;
        let (tx, rx) = spawn(cache_path, egui::Context::default());
        tx.try_send(source.clone())?;
        let result = rx.recv_timeout(Duration::from_secs(10))?;
        assert_eq!(result.path, source);
        assert!(result.image.is_some());
        assert!(result.cache_error.is_none());
        Ok(())
    }

    #[test]
    fn a_panicking_preview_is_unavailable_and_the_next_one_is_made() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let [panics, works] = ["panics.png", "works.png"].map(|name| directory.path().join(name));
        for path in [&panics, &works] {
            image::RgbImage::new(16, 16).save(path)?;
        }
        fn thumbnail(path: &Path) -> anyhow::Result<image::RgbImage> {
            assert!(!path.ends_with("panics.png"), "thumbnail panics");
            Ok(image::open(path)?.to_rgb8())
        }
        let (tx, rx) = spawn_with(
            directory.path().join("previews.sqlite3"),
            egui::Context::default(),
            thumbnail,
        );
        tx.try_send(panics.clone())?;
        tx.try_send(works.clone())?;
        let first = rx.recv_timeout(Duration::from_secs(10))?;
        assert_eq!(first.path, panics);
        assert!(first.image.is_none());
        let second = rx.recv_timeout(Duration::from_secs(10))?;
        assert_eq!(second.path, works);
        assert!(second.image.is_some());
        Ok(())
    }

    #[test]
    fn a_panicking_edited_preview_fails_and_the_next_one_is_rendered() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let [panics, works] = ["panics.png", "works.png"].map(|name| directory.path().join(name));
        for path in [&panics, &works] {
            image::RgbImage::new(16, 16).save(path)?;
        }
        fn render(path: &Path, _: &EditSource) -> anyhow::Result<image::RgbImage> {
            assert!(!path.ends_with("panics.png"), "render panics");
            Ok(image::open(path)?.to_rgb8())
        }
        let wanted = Wanted::default();
        wanted.lock().unwrap().extend([1, 2]);
        let (tx, rx) = spawn_edited_with(
            directory.path().join("previews.sqlite3"),
            wanted,
            egui::Context::default(),
            render,
        );
        let source = EditSource::Recipe("{}".into());
        let job = |id, path: &PathBuf| EditJob::Render {
            id,
            ticket: 0,
            path: path.clone(),
            source: source.clone(),
        };
        tx.send(job(1, &panics))?;
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10))?,
            EditResult::Failed
        ));
        tx.send(job(2, &works))?;
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10))?,
            EditResult::Ready(2, 0, _)
        ));
        Ok(())
    }

    #[test]
    fn edited_preview_cache_failures_are_reported() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("photo.png");
        image::RgbImage::new(16, 16).save(&source)?;
        let (tx, rx) = spawn_edited(
            directory.path().join("previews.sqlite3"),
            Wanted::default(),
            egui::Context::default(),
        );
        // Larger than the cache keeps.
        tx.send(EditJob::Store {
            path: source,
            tag: "edit".into(),
            image: image::RgbImage::new(2048, 8),
        })?;
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10))?,
            EditResult::CacheError(_)
        ));
        Ok(())
    }

    #[test]
    fn cache_failure_does_not_stop_previews_or_completion() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("photo.png");
        image::RgbImage::new(16, 16).save(&source)?;
        // A directory cannot be opened as a SQLite database.
        let (tx, rx) = spawn(directory.path().into(), egui::Context::default());
        let mut progress = Progress::default();
        for path in [source, directory.path().join("missing.ARW")] {
            tx.try_send(path)?;
            progress.queued();
        }
        let result = rx.recv_timeout(Duration::from_secs(10))?;
        assert!(result.image.is_some());
        assert!(result.cache_error.is_some());
        progress.finish(&result);
        assert_eq!((progress.completed, progress.total), (1, 2));
        progress.finish(&rx.recv_timeout(Duration::from_secs(10))?);
        assert_eq!(
            (progress.completed, progress.total, progress.failed),
            (2, 2, 1)
        );
        assert!(progress.cache_error.is_some());
        progress.queued();
        assert_eq!(
            (progress.completed, progress.total, progress.failed),
            (0, 1, 0)
        );
        Ok(())
    }
}
