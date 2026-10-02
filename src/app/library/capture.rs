//! Capture times for photos added from folders. Adding a folder records only
//! file names, so the dates are read from the files afterwards, in the
//! background, a batch at a time. Photos imported from Lightroom already have
//! theirs.
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, TryRecvError, channel},
    },
};

/// Photos read between two updates of the catalog.
const BATCH: usize = 32;

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Read {
    Dated(String),
    /// The file has no date; it keeps sorting before dated photos.
    Undated,
    /// The file could not be opened, e.g. it is offline; tried again later.
    Unreadable,
}

pub(super) struct Backfill {
    rx: Receiver<Vec<(i64, Read)>>,
    cancel: Arc<AtomicBool>,
}
impl Backfill {
    pub(super) fn start(photos: Vec<(i64, PathBuf)>, ctx: &egui::Context) -> Self {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for batch in photos.chunks(BATCH) {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let read = batch.iter().map(|(id, path)| (*id, read(path))).collect();
                if tx.send(read).is_err() {
                    return;
                }
                ctx.request_repaint();
            }
        });
        Self { rx, cancel }
    }
    /// The batches read since the last call, and whether the backfill is done.
    pub(super) fn poll(&self) -> (Vec<(i64, Read)>, bool) {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(batch) => out.extend(batch),
                Err(TryRecvError::Empty) => return (out, false),
                Err(TryRecvError::Disconnected) => return (out, true),
            }
        }
    }
}
impl Drop for Backfill {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Whether a capture time can be read from this kind of file at all, so
/// files that never yield one are not opened on every launch: a PNG rarely
/// has one, and the EXIF reader handles TIFF-based RAWs and RAF but not
/// Canon's CR3 and CRW, Sigma's X3F or Minolta's MRW.
pub(super) fn readable(path: &Path) -> bool {
    let extension = path
        .extension()
        .map(|x| x.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "jpg" | "jpeg" | "tif" | "tiff" => true,
        "cr3" | "crw" | "x3f" | "mrw" => false,
        _ => crate::storage::is_raw(path),
    }
}

fn read(path: &Path) -> Read {
    // A file that cannot be read now (offline, no permission, a network
    // error) is tried again later rather than taken for undated.
    use std::io::Read as _;
    let readable = std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut [0; 1]));
    if readable.is_err() {
        return Read::Unreadable;
    }
    match crate::export::exif::capture_time(path) {
        Some(time) => Read::Dated(time),
        None => Read::Undated,
    }
}
