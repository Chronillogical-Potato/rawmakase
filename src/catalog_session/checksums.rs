//! Each master photo's file checksum, read in the background and kept in the
//! catalog, so that a photo moved or renamed outside RAWmakase can be found
//! again. The quick hashes of every photo come first, so candidates exist
//! within seconds, then the full ones: one file at a time, in 1 MiB chunks,
//! at most [`RATE`] bytes a second, and not at all while paused (as while an
//! export runs). Dropping the reader stops it between chunks.
use super::CatalogSession;
use super::background::Wake;
use crate::catalog::{FolderId, PhotoId, ReadChecksum};
use crate::storage::{ChecksumRead, FileChecksum, Stamp};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

/// How fast files are read, in bytes a second.
pub(crate) const RATE: u64 = 64_000_000;
/// How often a paused reader looks again.
const PAUSED_POLL: Duration = Duration::from_millis(100);
/// How far reading can fall behind [`RATE`], as while a file is opened or
/// its metadata read, and catch up at full speed afterwards.
const MAX_CREDIT: Duration = Duration::from_secs(1);

/// The reader under way and what it was started for.
#[derive(Default)]
pub(super) struct Checksums {
    reader: Option<Reader>,
    /// Starting was asked for while a reader was under way: start again
    /// once it is done, for photos that came online meanwhile.
    again: bool,
    /// Reading waits while this is set: by the user, or for heavier work.
    paused: bool,
    /// Masters with a full checksum, and masters, in the catalog.
    pub(super) count: (usize, usize),
}
impl Checksums {
    /// These without their reader, which stops; still paused if they were.
    pub(super) fn stopped(self) -> Self {
        Self {
            reader: None,
            again: false,
            ..self
        }
    }
}

/// A master photo whose checksum may be missing or out of date.
struct Wanted {
    photo: PhotoId,
    folder: FolderId,
    filename: String,
    path: PathBuf,
    stored: Option<FileChecksum>,
}

struct Reader {
    rx: Receiver<ReadChecksum>,
    control: Arc<Control>,
    /// The session's structural generation it was started under; what it
    /// reads under another is dropped.
    generation: u64,
}
struct Control {
    cancel: AtomicBool,
    paused: AtomicBool,
}
impl Drop for Reader {
    fn drop(&mut self) {
        self.control.cancel.store(true, Ordering::Relaxed);
    }
}

impl CatalogSession {
    /// Reads the checksums missing or out of date of the masters `available`
    /// says are online, unless a reader is under way.
    pub(crate) fn start_checksums(&mut self, available: impl Fn(&Path) -> bool) {
        if self.backfill.checksums.reader.is_some() {
            self.backfill.checksums.again = true;
            return;
        }
        let Ok(mut stored) = self.catalog.checksums() else {
            return;
        };
        let wanted: Vec<Wanted> = self
            .photos
            .iter()
            .filter(|p| p.master.is_none() && !p.path.as_os_str().is_empty() && available(&p.path))
            .map(|p| Wanted {
                photo: p.id,
                folder: p.folder,
                filename: p.filename.clone(),
                path: p.path.clone(),
                stored: stored.remove(&p.id),
            })
            .collect();
        if wanted.is_empty() {
            return;
        }
        let control = Arc::new(Control {
            cancel: AtomicBool::new(false),
            paused: AtomicBool::new(self.backfill.checksums.paused),
        });
        let (tx, rx) = channel();
        let (thread, wake) = (control.clone(), self.wake.clone());
        crate::raw::spawn_background(move || read(wanted, &thread, &tx, &wake));
        self.backfill.checksums.reader = Some(Reader {
            rx,
            control,
            generation: self.generation,
        });
    }
    /// Pauses reading checksums, or lets it go on.
    pub(crate) fn pause_checksums(&mut self, paused: bool) {
        self.backfill.checksums.paused = paused;
        if let Some(reader) = &self.backfill.checksums.reader {
            reader.control.paused.store(paused, Ordering::Relaxed);
        }
    }
    /// Whether checksums are being read.
    #[cfg(test)]
    pub(crate) fn reading_checksums(&self) -> bool {
        self.backfill.checksums.reader.is_some()
    }
    /// Masters with a full checksum, of how many.
    pub(crate) fn checksum_count(&self) -> (usize, usize) {
        self.backfill.checksums.count
    }
    /// Saves the checksums read so far. One read before the catalog changed
    /// structurally is dropped, with its reader, and reading starts again.
    pub(crate) fn poll_checksums(&mut self) -> super::backfill::Saved<usize> {
        let mut out = super::backfill::Saved::default();
        let Some(reader) = &self.backfill.checksums.reader else {
            return out;
        };
        if reader.generation != self.generation {
            self.backfill.checksums.reader = None;
            out.start_again = true;
            return out;
        }
        let mut read = Vec::new();
        let done = loop {
            match reader.rx.try_recv() {
                Ok(checksum) => read.push(checksum),
                Err(TryRecvError::Empty) => break false,
                Err(TryRecvError::Disconnected) => break true,
            }
        };
        if done {
            self.backfill.checksums.reader = None;
            out.start_again = std::mem::take(&mut self.backfill.checksums.again);
        }
        if !read.is_empty() {
            match self.catalog.save_checksums(&read) {
                Ok(saved) => {
                    out.saved = saved.len();
                    self.count_checksums();
                }
                Err(e) => out.error = Some(e),
            }
        }
        out
    }
    /// Counts the checksums on record again.
    pub(super) fn count_checksums(&mut self) {
        if let Ok(count) = self.catalog.checksum_count() {
            self.backfill.checksums.count = count;
        }
    }
}

/// Whether `stored` was read from the file as it is now: the same size and
/// modification time.
fn current(stored: &FileChecksum, path: &Path) -> bool {
    Stamp::read(path).is_ok_and(|now| {
        now.size == stored.identity.size && now.modified_ns == stored.identity.modified_ns
    })
}

/// The reader's thread: quick hashes first, then full ones. A file that
/// can't be read now is left for the next start.
fn read(mut wanted: Vec<Wanted>, control: &Control, tx: &Sender<ReadChecksum>, wake: &Wake) {
    let mut pace = Pace::new(control);
    // A checksum read from the file as it is stays.
    for w in &mut wanted {
        if !pace.go_on(0) {
            return;
        }
        if w.stored.as_ref().is_some_and(|s| !current(s, &w.path)) {
            w.stored = None;
        }
    }
    wanted.retain(|w| w.stored.as_ref().is_none_or(|s| s.full.is_none()));
    let send = |w: &Wanted, checksum: FileChecksum| {
        let read = ReadChecksum {
            photo: w.photo,
            folder: w.folder,
            filename: w.filename.clone(),
            path: w.path.clone(),
            checksum,
        };
        let sent = tx.send(read).is_ok();
        wake();
        sent
    };
    for w in wanted.iter_mut().filter(|w| w.stored.is_none()) {
        if !pace.go_on(0) {
            return;
        }
        match FileChecksum::read_quick(&w.path, |n| pace.go_on(n)) {
            Ok(ChecksumRead::Done(quick)) => {
                if !send(w, quick.clone()) {
                    return;
                }
                w.stored = Some(quick);
            }
            Ok(ChecksumRead::Cancelled) => return,
            Ok(ChecksumRead::Changed) | Err(_) => {}
        }
    }
    for w in wanted.iter().filter(|w| w.stored.is_some()) {
        if !pace.go_on(0) {
            return;
        }
        match FileChecksum::read_full(&w.path, |n| pace.go_on(n)) {
            Ok(ChecksumRead::Done(full)) => {
                if !send(w, full) {
                    return;
                }
            }
            Ok(ChecksumRead::Cancelled) => return,
            // Changed while read, or not readable now: next time.
            Ok(ChecksumRead::Changed) | Err(_) => {}
        }
    }
}

/// Keeps reading under [`RATE`] and waits while paused.
struct Pace<'a> {
    control: &'a Control,
    since: Instant,
    bytes: u64,
}
impl<'a> Pace<'a> {
    fn new(control: &'a Control) -> Self {
        Self {
            control,
            since: Instant::now(),
            bytes: 0,
        }
    }
    /// After `bytes` were read: waits as long as the rate or a pause asks,
    /// and says whether to go on.
    fn go_on(&mut self, bytes: usize) -> bool {
        self.bytes += bytes as u64;
        let due = Duration::from_secs_f64(self.bytes as f64 / RATE as f64);
        let elapsed = self.since.elapsed();
        if let Some(wait) = due.checked_sub(elapsed) {
            std::thread::sleep(wait);
        } else if elapsed > due + MAX_CREDIT {
            // Time spent not reading is no licence to read faster later.
            self.since = Instant::now();
            self.bytes = 0;
        }
        if self.control.paused.load(Ordering::Relaxed) {
            while self.control.paused.load(Ordering::Relaxed) {
                if self.control.cancel.load(Ordering::Relaxed) {
                    return false;
                }
                std::thread::sleep(PAUSED_POLL);
            }
            // Not made up for after the pause.
            *self = Self::new(self.control);
        }
        !self.control.cancel.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Catalog, CatalogLocation};
    use anyhow::Result;

    /// A session of a catalog with two photos, one with a virtual copy.
    fn session() -> Result<(tempfile::TempDir, CatalogSession)> {
        let dir = tempfile::tempdir()?;
        let folder = dir.path().join("photos");
        std::fs::create_dir(&folder)?;
        std::fs::write(folder.join("a.dng"), vec![1u8; 3 * 1024 * 1024])?;
        std::fs::write(folder.join("b.dng"), b"synthetic raw b")?;
        let path = dir.path().join("library.rawmakase");
        let mut catalog = Catalog::create(&path)?;
        catalog.add_folder(&folder)?;
        let a = catalog
            .photos()?
            .into_iter()
            .find(|p| p.filename == "a.dng")
            .unwrap();
        catalog.create_virtual_copy(a.id)?;
        let session = CatalogSession::open(&CatalogLocation::from(&path))?.session;
        Ok((dir, session))
    }
    /// Polls until the reader is done; the photos saved.
    fn read_all(session: &mut CatalogSession) -> usize {
        let started = Instant::now();
        let mut saved = 0;
        while session.reading_checksums() {
            assert!(started.elapsed() < Duration::from_secs(10));
            let polled = session.poll_checksums();
            assert!(polled.error.is_none());
            saved += polled.saved;
            std::thread::sleep(Duration::from_millis(5));
        }
        saved
    }

    #[test]
    fn every_master_gets_a_quick_then_a_full_checksum_once() -> Result<()> {
        let (_dir, mut session) = session()?;
        assert_eq!(session.checksum_count(), (0, 2));
        session.start_checksums(|_| true);
        // Two quick, then two full.
        assert_eq!(read_all(&mut session), 4);
        assert_eq!(session.checksum_count(), (2, 2));
        let stored = session.catalog.checksums()?;
        assert_eq!(stored.len(), 2);
        assert!(stored.values().all(|c| c.full.is_some()));
        // Current: read again only once a file changes.
        session.start_checksums(|_| true);
        assert_eq!(read_all(&mut session), 0);
        let b = session
            .photos
            .iter()
            .find(|p| p.filename == "b.dng")
            .unwrap();
        std::fs::write(&b.path, b"synthetic raw b, changed")?;
        session.start_checksums(|_| true);
        assert_eq!(read_all(&mut session), 2);
        Ok(())
    }

    #[test]
    fn a_start_asked_for_while_reading_starts_again_once_done() -> Result<()> {
        let (_dir, mut session) = session()?;
        session.start_checksums(|path| path.ends_with("b.dng"));
        // a.dng came online meanwhile.
        session.start_checksums(|_| true);
        let started = Instant::now();
        let mut again = false;
        while session.reading_checksums() {
            assert!(started.elapsed() < Duration::from_secs(10));
            again |= session.poll_checksums().start_again;
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(again);
        session.start_checksums(|_| true);
        read_all(&mut session);
        assert_eq!(session.checksum_count(), (2, 2));
        Ok(())
    }

    #[test]
    fn offline_photos_are_left_for_later() -> Result<()> {
        let (_dir, mut session) = session()?;
        session.start_checksums(|path| path.ends_with("b.dng"));
        assert_eq!(read_all(&mut session), 2);
        assert_eq!(session.checksum_count(), (1, 2));
        Ok(())
    }

    #[test]
    fn what_was_read_before_a_structural_change_is_dropped_and_read_again() -> Result<()> {
        let (_dir, mut session) = session()?;
        session.pause_checksums(true);
        session.start_checksums(|_| true);
        // Read under the old generation, saved under none.
        session.pause_checksums(false);
        std::thread::sleep(Duration::from_millis(200));
        session.reload()?;
        let polled = session.poll_checksums();
        assert_eq!(polled.saved, 0);
        assert!(polled.start_again && !session.reading_checksums());
        assert_eq!(session.checksum_count(), (0, 2));
        session.start_checksums(|_| true);
        assert_eq!(read_all(&mut session), 4);
        Ok(())
    }

    #[test]
    fn a_paused_reader_reads_nothing_until_it_goes_on() -> Result<()> {
        let (_dir, mut session) = session()?;
        session.pause_checksums(true);
        session.start_checksums(|_| true);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(session.poll_checksums().saved, 0);
        assert!(session.reading_checksums());
        // Stopping keeps the pause for the next reader.
        session.stop_backfill();
        session.start_checksums(|_| true);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(session.poll_checksums().saved, 0);
        session.pause_checksums(false);
        assert_eq!(read_all(&mut session), 4);
        Ok(())
    }

    #[test]
    fn pace_keeps_under_the_rate_and_stops_when_cancelled_while_paused() {
        let control = Control {
            cancel: AtomicBool::new(false),
            paused: AtomicBool::new(false),
        };
        let mut pace = Pace::new(&control);
        let started = Instant::now();
        assert!(pace.go_on((RATE / 10) as usize));
        assert!(started.elapsed() >= Duration::from_millis(95));
        // Idle time beyond a second is not banked.
        pace.since -= Duration::from_secs(10);
        pace.go_on(0);
        let started = Instant::now();
        assert!(pace.go_on((RATE / 10) as usize));
        assert!(started.elapsed() >= Duration::from_millis(95));
        control.paused.store(true, Ordering::Relaxed);
        control.cancel.store(true, Ordering::Relaxed);
        assert!(!pace.go_on(0));
    }
}
