//! Screen previews: a photo rendered with its edit at the size a view shows
//! it, for the views that show more than one photo large (Compare). The
//! grid's 640 px previews stand in until they are ready.
//!
//! Renders run on two threads, newest request first, so both photos of a
//! comparison come in together; a request no longer shown is skipped. The
//! results are kept as textures, a few at a time, oldest dropped first.
use super::previews::EditSource;
use super::thumbnails;
use crate::catalog::Photo;
use eframe::egui;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        mpsc::{Receiver, Sender, channel},
    },
};

/// Previews are rendered in steps of this many pixels, so resizing the
/// window a little does not render them again.
const EDGE_STEP: u32 = 512;
/// Textures kept: two comparisons' worth.
const KEPT: usize = 4;
const THREADS: usize = 2;

/// A preview as asked for: photo, file (photo ids can be reused) and edge.
type Key = (i64, PathBuf, u32);
type Render = fn(&Path, u32, Option<&EditSource>) -> anyhow::Result<image::RgbImage>;

struct Job {
    key: Key,
    /// Matches the result to this request, not to one made before the
    /// photo's edit changed.
    ticket: u64,
    edit: Option<EditSource>,
}
struct Done {
    key: Key,
    ticket: u64,
    outcome: Outcome,
}
enum Outcome {
    Ready(image::RgbImage),
    Failed(String),
    /// No longer shown when its turn came; asked for again if shown.
    Skipped,
}
#[derive(Default)]
struct Queue {
    jobs: Vec<Job>,
    closed: bool,
}

/// What a view can draw for a photo.
pub(super) enum Shown<'a> {
    Ready(&'a egui::TextureHandle),
    Loading,
    Failed(&'a str),
}

pub(super) struct ScreenPreviews {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    results: Receiver<Done>,
    /// Previews shown this frame and last; the workers skip the rest.
    wanted: Arc<Mutex<HashSet<Key>>>,
    seen: HashSet<Key>,
    /// Requests on their way, by the ticket of the latest.
    pending: HashMap<Key, u64>,
    next_ticket: u64,
    failed: HashMap<Key, String>,
    textures: HashMap<Key, egui::TextureHandle>,
    order: VecDeque<Key>,
}
impl ScreenPreviews {
    pub(super) fn new(ctx: &egui::Context) -> Self {
        Self::with(ctx, render)
    }
    fn with(ctx: &egui::Context, render: Render) -> Self {
        let queue: Arc<(Mutex<Queue>, Condvar)> = Default::default();
        let wanted: Arc<Mutex<HashSet<Key>>> = Default::default();
        let (tx, results) = channel();
        for _ in 0..THREADS {
            let (queue, wanted, tx, ctx) = (queue.clone(), wanted.clone(), tx.clone(), ctx.clone());
            std::thread::spawn(move || work(&queue, &wanted, &tx, &ctx, render));
        }
        Self {
            queue,
            results,
            wanted,
            seen: HashSet::new(),
            pending: HashMap::new(),
            next_ticket: 0,
            failed: HashMap::new(),
            textures: HashMap::new(),
            order: VecDeque::new(),
        }
    }
    /// `photo` at `edge` pixels, asked for unless it is ready, failed or on
    /// its way; `edit` is looked up only when it is asked for.
    pub(super) fn get(
        &mut self,
        photo: &Photo,
        edge: u32,
        edit: impl FnOnce() -> Option<EditSource>,
    ) -> Shown<'_> {
        let key = (
            photo.id,
            photo.path.clone(),
            edge.div_ceil(EDGE_STEP).max(1) * EDGE_STEP,
        );
        self.seen.insert(key.clone());
        if !self.textures.contains_key(&key)
            && !self.failed.contains_key(&key)
            && !self.pending.contains_key(&key)
        {
            self.next_ticket += 1;
            self.pending.insert(key.clone(), self.next_ticket);
            self.wanted.lock().unwrap().insert(key.clone());
            let (queue, ready) = &*self.queue;
            queue.lock().unwrap().jobs.push(Job {
                key: key.clone(),
                ticket: self.next_ticket,
                edit: edit(),
            });
            ready.notify_one();
        }
        if let Some(texture) = self.textures.get(&key) {
            // Kept longest: the previews in use.
            self.order.retain(|k| *k != key);
            self.order.push_back(key);
            Shown::Ready(texture)
        } else if let Some(error) = self.failed.get(&key) {
            Shown::Failed(error)
        } else {
            Shown::Loading
        }
    }
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(Done {
            key,
            ticket,
            outcome,
        }) = self.results.try_recv()
        {
            // Asked for before its edit changed: a newer request is coming.
            if self.pending.get(&key) != Some(&ticket) {
                continue;
            }
            self.pending.remove(&key);
            match outcome {
                Outcome::Ready(image) => {
                    let size = [image.width() as usize, image.height() as usize];
                    let texture = ctx.load_texture(
                        "library-screen-preview",
                        egui::ColorImage::from_rgb(size, image.as_raw()),
                        egui::TextureOptions::LINEAR,
                    );
                    while self.textures.len() >= KEPT {
                        let Some(old) = self.order.pop_front() else {
                            break;
                        };
                        self.textures.remove(&old);
                    }
                    self.order.push_back(key.clone());
                    self.textures.insert(key, texture);
                }
                Outcome::Failed(error) => {
                    self.failed.insert(key, error);
                }
                Outcome::Skipped => {}
            }
        }
    }
    /// Hands the workers the previews shown this frame. Call once per frame.
    pub(super) fn publish_shown(&mut self) {
        *self.wanted.lock().unwrap() = std::mem::take(&mut self.seen);
    }
    /// Lets previews that failed be asked for again, e.g. once their
    /// originals are back online.
    pub(super) fn retry_failed(&mut self) {
        self.failed.clear();
    }
    /// Forgets `id`'s previews, as its edit has changed or it was removed.
    pub(super) fn forget(&mut self, id: i64) {
        self.textures.retain(|key, _| key.0 != id);
        self.order.retain(|key| key.0 != id);
        self.failed.retain(|key, _| key.0 != id);
        self.pending.retain(|key, _| key.0 != id);
    }
    #[cfg(test)]
    pub(super) fn wait(&mut self, ctx: &egui::Context) {
        let started = std::time::Instant::now();
        while !self.pending.is_empty() && started.elapsed() < std::time::Duration::from_secs(20) {
            std::thread::sleep(std::time::Duration::from_millis(5));
            self.poll(ctx);
        }
    }
}
impl Drop for ScreenPreviews {
    fn drop(&mut self) {
        let (queue, ready) = &*self.queue;
        queue.lock().unwrap().closed = true;
        ready.notify_all();
    }
}

/// A worker thread: renders the newest job still wanted, until closed.
fn work(
    queue: &(Mutex<Queue>, Condvar),
    wanted: &Mutex<HashSet<Key>>,
    results: &Sender<Done>,
    ctx: &egui::Context,
    render: Render,
) {
    let (queue, ready) = queue;
    loop {
        let job = {
            let mut queue = queue.lock().unwrap();
            while queue.jobs.is_empty() && !queue.closed {
                queue = ready.wait(queue).unwrap();
            }
            if queue.closed {
                return;
            }
            queue.jobs.pop().unwrap()
        };
        let Job { key, ticket, edit } = job;
        let outcome = if !wanted.lock().unwrap().contains(&key) {
            Outcome::Skipped
        } else {
            let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                render(&key.1, key.2, edit.as_ref())
            }));
            match rendered {
                Ok(Ok(image)) => Outcome::Ready(image),
                Ok(Err(e)) => Outcome::Failed(format!("{e:#}")),
                Err(_) => Outcome::Failed("The preview could not be rendered".into()),
            }
        };
        let done = Done {
            key,
            ticket,
            outcome,
        };
        if results.send(done).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// `path` within `edge` pixels: a RAW developed with its edit (or the
/// defaults Develop opens it with) from the fast half-size decode; a JPEG,
/// TIFF or PNG as it is.
fn render(path: &Path, edge: u32, edit: Option<&EditSource>) -> anyhow::Result<image::RgbImage> {
    if !crate::storage::is_raw(path) {
        return Ok(thumbnails::downscale(&thumbnails::raster(path)?, edge));
    }
    let raw = crate::raw::Raw::open(path)?;
    let recipe = EditSource::recipe(edit, &raw)?;
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let image = raw.develop(true, &cancel)?;
    let out = crate::develop::render(&image, &recipe, edge)?;
    image::RgbImage::from_raw(out.width, out.height, out.rgb8())
        .ok_or_else(|| anyhow::anyhow!("Invalid preview size"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(id: i64, path: &Path) -> Photo {
        Photo {
            id,
            path: path.into(),
            ..Default::default()
        }
    }

    #[test]
    fn previews_render_at_the_size_shown_and_are_kept() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("a.png");
        image::RgbImage::new(3000, 2000).save(&path)?;
        let ctx = egui::Context::default();
        let mut screen = ScreenPreviews::new(&ctx);
        let a = photo(1, &path);
        assert!(matches!(screen.get(&a, 900, || None), Shown::Loading));
        screen.wait(&ctx);
        // Rendered at the next step up from the size asked for.
        match screen.get(&a, 900, || None) {
            Shown::Ready(texture) => assert_eq!(texture.size(), [1024, 683]),
            _ => panic!("not rendered"),
        }
        // Forgetting the photo renders it again.
        screen.forget(1);
        assert!(matches!(screen.get(&a, 900, || None), Shown::Loading));
        Ok(())
    }

    #[test]
    fn a_failed_or_panicking_render_is_reported_and_the_rest_go_on() {
        fn render(path: &Path, _: u32, _: Option<&EditSource>) -> anyhow::Result<image::RgbImage> {
            match path.to_str() {
                Some("panics") => panic!("render panics"),
                Some("fails") => anyhow::bail!("no such photo"),
                _ => Ok(image::RgbImage::new(4, 4)),
            }
        }
        let ctx = egui::Context::default();
        let mut screen = ScreenPreviews::with(&ctx, render);
        let photos = [(1, "panics"), (2, "fails"), (3, "works")]
            .map(|(id, path)| photo(id, Path::new(path)));
        for p in &photos {
            let _ = screen.get(p, 100, || None);
        }
        screen.wait(&ctx);
        assert!(matches!(
            screen.get(&photos[0], 100, || None),
            Shown::Failed("The preview could not be rendered")
        ));
        assert!(matches!(
            screen.get(&photos[1], 100, || None),
            Shown::Failed("no such photo")
        ));
        assert!(matches!(
            screen.get(&photos[2], 100, || None),
            Shown::Ready(_)
        ));
    }

    #[test]
    fn a_preview_no_longer_shown_is_skipped_and_asked_for_again() {
        let ctx = egui::Context::default();
        let mut screen = ScreenPreviews::with(&ctx, |_, _, _| Ok(image::RgbImage::new(4, 4)));
        let a = photo(1, Path::new("a"));
        // Queued, then not shown for a frame before a worker takes it.
        let key = (1, PathBuf::from("a"), EDGE_STEP);
        screen.pending.insert(key.clone(), 0);
        screen.queue.0.lock().unwrap().jobs.push(Job {
            key: key.clone(),
            ticket: 0,
            edit: None,
        });
        screen.queue.1.notify_one();
        screen.wait(&ctx);
        assert!(screen.textures.is_empty());
        assert!(matches!(screen.get(&a, 100, || None), Shown::Loading));
        screen.wait(&ctx);
        assert!(matches!(screen.get(&a, 100, || None), Shown::Ready(_)));
    }

    #[test]
    fn a_render_asked_for_before_an_edit_changed_is_dropped() {
        let ctx = egui::Context::default();
        let mut screen = ScreenPreviews::with(&ctx, |_, _, _| Ok(image::RgbImage::new(4, 4)));
        let key = (1, PathBuf::from("a"), EDGE_STEP);
        // The photo was asked for again (ticket 2) after the edit changed,
        // and the render asked for before it (ticket 1) comes in first.
        screen.pending.insert(key.clone(), 2);
        screen.wanted.lock().unwrap().insert(key.clone());
        screen.queue.0.lock().unwrap().jobs.push(Job {
            key: key.clone(),
            ticket: 1,
            edit: None,
        });
        screen.queue.1.notify_one();
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_millis(300) {
            std::thread::sleep(std::time::Duration::from_millis(5));
            screen.poll(&ctx);
        }
        assert!(screen.textures.is_empty());
        assert_eq!(screen.pending.get(&key), Some(&2));
    }

    #[test]
    fn a_failed_preview_is_asked_for_again_once_retried() {
        let ctx = egui::Context::default();
        let mut screen = ScreenPreviews::with(&ctx, |_, _, _| anyhow::bail!("offline"));
        let a = photo(1, Path::new("a"));
        let _ = screen.get(&a, 100, || None);
        screen.wait(&ctx);
        assert!(matches!(screen.get(&a, 100, || None), Shown::Failed(_)));
        screen.retry_failed();
        assert!(matches!(screen.get(&a, 100, || None), Shown::Loading));
    }
}
