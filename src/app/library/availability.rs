//! Which originals are online. Listing every folder can take seconds on a
//! network share, so the Library opens without waiting and treats photos as
//! online until it is known.
use crate::catalog::Photo;
use eframe::egui;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, TryRecvError, channel},
};

#[derive(Default)]
pub(super) struct Availability {
    /// The check still running, if any.
    checking: Option<Receiver<HashSet<PathBuf>>>,
    available: HashSet<PathBuf>,
}
impl Availability {
    /// Finds out which of `photos` are online, in the background. Until the
    /// answer is in, every photo counts as online.
    pub(super) fn start(&mut self, photos: &[Photo], ctx: &egui::Context) {
        let (tx, rx) = channel();
        let photos = photos.to_vec();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if tx.send(super::thumbnails::available_paths(&photos)).is_ok() {
                ctx.request_repaint();
            }
        });
        self.checking = Some(rx);
    }
    /// Takes the result of the check once it is there (or waits for it).
    /// Returns whether the answer arrived, so the caller can filter again.
    pub(super) fn poll(&mut self, wait: bool, photos: &[Photo]) -> bool {
        let Some(rx) = &self.checking else {
            return false;
        };
        let result = if wait {
            rx.recv().map_err(|_| TryRecvError::Disconnected)
        } else {
            rx.try_recv()
        };
        match result {
            Ok(available) => self.available = available,
            Err(TryRecvError::Empty) => return false,
            // The check failed: claim nothing is offline.
            Err(TryRecvError::Disconnected) => {
                self.available = photos.iter().map(|p| p.path.clone()).collect()
            }
        }
        self.checking = None;
        true
    }
    pub(super) fn is_available(&self, path: &Path) -> bool {
        self.checking.is_some() || self.available.contains(path)
    }
    pub(super) fn count(&self, photos: &[Photo]) -> usize {
        photos.iter().filter(|p| self.is_available(&p.path)).count()
    }
}
