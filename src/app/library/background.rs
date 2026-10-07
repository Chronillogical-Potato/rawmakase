//! Reading something from many photos' files in the background, a batch at a
//! time, for the catalog: capture times, camera settings. Dropping the
//! reader stops it.
use crate::catalog::PhotoId;
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

pub(super) struct Reader<T> {
    rx: Receiver<Vec<(PhotoId, T)>>,
    cancel: Arc<AtomicBool>,
}
impl<T: Send + 'static> Reader<T> {
    /// Reads `photos` with `read` on a thread of its own.
    pub(super) fn start(
        photos: Vec<(PhotoId, PathBuf)>,
        ctx: &egui::Context,
        read: fn(&Path) -> T,
    ) -> Self {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        let ctx = ctx.clone();
        crate::raw::spawn_background(move || {
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
    /// The batches read since the last call, and whether reading is done.
    pub(super) fn poll(&self) -> (Vec<(PhotoId, T)>, bool) {
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
impl<T> Drop for Reader<T> {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Whether the file can be read now: an offline file, one without
/// permission or a network error is tried again later.
pub(super) fn can_read(path: &Path) -> bool {
    use std::io::Read as _;
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut [0; 1]))
        .is_ok()
}
