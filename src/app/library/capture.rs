//! Capture times for photos added from folders. Adding a folder records only
//! file names, so the dates are read from the files afterwards, in the
//! background, a batch at a time. Photos imported from Lightroom already have
//! theirs.
use super::Library;
use std::collections::HashMap;
use std::path::Path;

impl Library {
    /// Once it is known which photos are online, filters again and reads the
    /// capture times still missing, including ones that were offline before.
    pub(super) fn availability_known(&mut self) {
        self.filter();
        // An original back online renders where it failed before.
        self.screen.retry_failed();
        self.capture_tried.clear();
        self.start_capture_times();
        self.start_photo_info();
    }
    pub(super) fn start_capture_times(&mut self) {
        if self.capture.is_some() {
            return;
        }
        let todo: Vec<_> = self
            .photos
            .iter()
            .filter(|p| {
                p.captured.is_empty()
                    && p.master.is_none()
                    && !self.capture_tried.contains(&p.id)
                    && readable(&p.path)
                    && self.is_available(&p.path)
            })
            .map(|p| (p.id, p.path.clone()))
            .collect();
        if !todo.is_empty() {
            self.capture_tried.extend(todo.iter().map(|(id, _)| *id));
            self.capture = Some(super::background::Reader::start(todo, &self.ctx, read));
        }
    }
    /// Saves the capture times read so far and sorts the photos again.
    pub(super) fn poll_capture_times(&mut self) {
        let Some(backfill) = &self.capture else {
            return;
        };
        let (read, done) = backfill.poll();
        if done {
            self.capture = None;
        }
        let dated: Vec<(i64, String)> = read
            .into_iter()
            .filter_map(|(id, read)| match read {
                Read::Dated(time) => Some((id, time)),
                _ => None,
            })
            .collect();
        if !dated.is_empty() {
            match self.catalog.fill_capture_times(&dated) {
                Ok(()) => self.apply_capture_times(&dated),
                // Read again after the next online check, which clears the
                // photos tried.
                Err(e) => self.message = format!("Capture times could not be saved: {e}"),
            }
        }
        if done {
            // Photos added while it ran.
            self.start_capture_times();
        }
    }
    /// Re-sorts after capture times were filled in, as the catalog orders
    /// photos, keeping the selected photo selected and where it was on screen.
    pub(super) fn apply_capture_times(&mut self, times: &[(i64, String)]) {
        let times: HashMap<i64, &String> = times.iter().map(|(id, t)| (*id, t)).collect();
        for photo in &mut self.photos {
            if photo.captured.is_empty()
                && let Some(time) = times
                    .get(&photo.id)
                    .or_else(|| photo.master.and_then(|m| times.get(&m)))
            {
                photo.captured = (*time).clone();
            }
        }
        self.resort_in_place(|library| {
            library.photos.sort_by(|a, b| {
                (&a.captured, &a.filename, a.id).cmp(&(&b.captured, &b.filename, b.id))
            });
        });
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Read {
    Dated(String),
    /// The file has no date; it keeps sorting before dated photos.
    Undated,
    /// The file could not be opened, e.g. it is offline; tried again later.
    Unreadable,
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
    if !super::background::can_read(path) {
        return Read::Unreadable;
    }
    match crate::export::exif::capture_time(path) {
        Some(time) => Read::Dated(time),
        None => Read::Undated,
    }
}
