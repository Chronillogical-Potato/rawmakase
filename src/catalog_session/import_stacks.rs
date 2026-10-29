//! Stack Camera Bursts when adding photos: the files of the batches added
//! with it, read in the background for what their cameras say of their
//! sequences and when they were taken, so their bursts can be stacked (see
//! `app::library::import_stacks`). The batches stay to be done in the catalog
//! (`Catalog::pending_burst_stacks`) until stacked: a batch with a photo
//! offline waits for it, across restarts. The read is one of the session's
//! background reads, stopped with them.
use super::CatalogSession;
use super::background::{Reader, can_read};
use crate::catalog::{PendingBurstStacks, PhotoId, SavedSequence};
use crate::exif::Sequence;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// What reading a photo's file found.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BurstRead {
    Read {
        /// What its camera says of its sequence.
        sequence: Option<Sequence>,
        /// When it was taken, read when the catalog had no time for it.
        captured: Option<String>,
        /// Read now from the file, not kept in the catalog yet: to save.
        save: Option<SavedSequence>,
    },
    /// Offline, unreadable now, or changed while read: its batch waits.
    Unreadable,
}

/// The batches read, once every photo of theirs has been.
pub(crate) struct BurstsRead {
    pub batches: Vec<PendingBurstStacks>,
    pub read: HashMap<PhotoId, BurstRead>,
    /// The session's structural generation the read started under: under
    /// another, photo ids may name other photos.
    pub generation: u64,
}

/// The read under way.
pub(super) struct Reading {
    reader: Reader<BurstRead>,
    batches: Vec<PendingBurstStacks>,
    read: HashMap<PhotoId, BurstRead>,
    /// The photos not read yet: those left when the reader stops were never
    /// read (it panicked).
    left: HashSet<PhotoId>,
    generation: u64,
}

impl CatalogSession {
    /// Reads the files of the batches whose bursts are still to be stacked,
    /// each file's sequence by `read` (`exif::sequence`); asked while
    /// reading, once done (see [`Self::import_stacks_again`]). A batch with a
    /// photo `available` says is offline is left for later. Nothing is read when no batch is to be
    /// done, or when reading the catalog fails (tried again on the next
    /// start).
    pub(crate) fn start_import_stacks_by(
        &mut self,
        available: impl Fn(&Path) -> bool,
        read: fn(&Path) -> Option<Sequence>,
    ) {
        if self.backfill.bursts.is_some() {
            self.backfill.bursts_again = true;
            return;
        }
        let Ok(pending) = self.catalog.pending_burst_stacks() else {
            return;
        };
        if pending.is_empty() {
            return;
        }
        let Ok(mut stored) = self.catalog.sequences() else {
            return;
        };
        let photos: HashMap<PhotoId, &crate::catalog::Photo> = self
            .photos
            .iter()
            .filter(|p| p.master.is_none())
            .map(|p| (p.id, p))
            .collect();
        let mut batches = Vec::new();
        let mut todo: Vec<(PhotoId, PathBuf)> = Vec::new();
        // By the file each was asked for: what was kept of a photo is only
        // checked against its own file.
        let mut asked: HashMap<PathBuf, (Option<SavedSequence>, bool)> = HashMap::new();
        for batch in pending {
            let members: Vec<&crate::catalog::Photo> = batch
                .photos
                .iter()
                .filter_map(|id| photos.get(id).copied())
                .collect();
            if members.iter().any(|p| !available(&p.path)) {
                continue;
            }
            for p in members {
                todo.push((p.id, p.path.clone()));
                asked.insert(
                    p.path.clone(),
                    (stored.remove(&p.id), p.captured.is_empty()),
                );
            }
            batches.push(batch);
        }
        if batches.is_empty() {
            return;
        }
        let left = todo.iter().map(|(id, _)| *id).collect();
        let reader = Reader::start(todo, self.wake.clone(), move |path| {
            let (stored, untimed) = asked
                .get(path)
                .map_or((None, true), |(s, t)| (s.as_ref(), *t));
            read_for_stacking(path, stored, untimed, read)
        });
        self.backfill.bursts = Some(Reading {
            reader,
            batches,
            read: HashMap::new(),
            left,
            generation: self.generation(),
        });
    }
    /// Whether the files of batches to stack are being read.
    #[cfg(test)]
    pub(crate) fn reading_import_stacks(&self) -> bool {
        self.backfill.bursts.is_some()
    }
    /// Whether they were asked to be read while being read, as more files
    /// came online: true once, after the read is done.
    pub(crate) fn import_stacks_again(&mut self) -> bool {
        self.backfill.bursts.is_none() && std::mem::take(&mut self.backfill.bursts_again)
    }
    /// Collects what was read; once every photo has been, returns it. A read
    /// that stopped before every photo (its thread panicked) returns nothing:
    /// its batches stay to be done, read again on the next start.
    pub(crate) fn poll_import_stacks(&mut self) -> Option<BurstsRead> {
        let reading = self.backfill.bursts.as_mut()?;
        let (read, done) = reading.reader.poll();
        for (id, got) in read {
            reading.left.remove(&id);
            reading.read.insert(id, got);
        }
        if !done {
            return None;
        }
        let reading = self.backfill.bursts.take()?;
        reading.left.is_empty().then_some(BurstsRead {
            batches: reading.batches,
            read: reading.read,
            generation: reading.generation,
        })
    }
}

/// Reads the file at `path` for stacking its burst: its sequence (what the
/// catalog kept of it, `stored`, when it is the same file, else as `read`
/// reads it) and, when `untimed`, its capture time; both of one file, not
/// changed while read.
pub(crate) fn read_for_stacking(
    path: &Path,
    stored: Option<&SavedSequence>,
    untimed: bool,
    read: fn(&Path) -> Option<Sequence>,
) -> BurstRead {
    let Ok(stamp) = crate::storage::Stamp::read(path) else {
        return BurstRead::Unreadable;
    };
    if !can_read(path) {
        return BurstRead::Unreadable;
    }
    let (sequence, save) = match stored {
        Some(saved) if saved.stamp == stamp => (saved.sequence.clone(), None),
        _ => {
            let sequence = read(path);
            let save = SavedSequence {
                stamp,
                sequence: sequence.clone(),
            };
            (sequence, Some(save))
        }
    };
    let captured = if untimed {
        super::file_capture_time(path)
    } else {
        None
    };
    if crate::storage::Stamp::read(path).ok() != Some(stamp) {
        return BurstRead::Unreadable;
    }
    BurstRead::Read {
        sequence,
        captured,
        save,
    }
}
