//! What adding a folder leaves out, read from the photos' files in the
//! background and kept in the catalog: capture times, so the photos sort as
//! they were taken, and photo info (camera, lens, exposure, size). Photos
//! imported from Lightroom already have both.
use super::CatalogSession;
use super::background::{Reader, can_read};
use crate::catalog::PhotoId;
use crate::metadata::PhotoInfo;

use std::collections::{HashMap, HashSet};
use std::path::Path;

/// The readers under way, and what they have tried.
#[derive(Default)]
pub(super) struct Backfill {
    capture: Option<Reader<CaptureRead>>,
    /// Photos the capture-time backfill tried since the last online check.
    capture_tried: HashSet<PhotoId>,
    info: Option<Reader<Option<Option<PhotoInfo>>>>,
    /// Photo info was asked for while it was being read.
    info_again: bool,
}

/// What one poll of a backfill saved.
#[derive(Debug, Default)]
pub(crate) struct Saved<T> {
    /// What was saved in the catalog and is now in the session's lists.
    pub saved: T,
    /// Saving failed; what was read is read again after the next online check.
    pub error: Option<anyhow::Error>,
    /// The reader finished, and photos it did not cover may want reading:
    /// start it again.
    pub start_again: bool,
}

impl CatalogSession {
    /// Reads the capture times still missing, of the photos `available` says
    /// are online and not tried since [`Self::retry_capture_times`].
    pub(crate) fn start_capture_times(&mut self, available: impl Fn(&Path) -> bool) {
        if self.backfill.capture.is_some() {
            return;
        }
        let tried = &self.backfill.capture_tried;
        let todo: Vec<_> = self
            .photos
            .iter()
            .filter(|p| {
                p.captured.is_empty()
                    && p.master.is_none()
                    && !tried.contains(&p.id)
                    && has_capture_time(&p.path)
                    && available(&p.path)
            })
            .map(|p| (p.id, p.path.clone()))
            .collect();
        if !todo.is_empty() {
            self.backfill
                .capture_tried
                .extend(todo.iter().map(|(id, _)| *id));
            self.backfill.capture = Some(Reader::start(todo, self.wake.clone(), read_capture));
        }
    }
    /// Lets the next [`Self::start_capture_times`] try every photo again, as
    /// once more photos are online.
    pub(crate) fn retry_capture_times(&mut self) {
        self.backfill.capture_tried.clear();
    }
    /// Whether capture times are being read.
    #[cfg(test)]
    pub(crate) fn reading_capture_times(&self) -> bool {
        self.backfill.capture.is_some()
    }
    /// Saves the capture times read so far, a virtual copy taking its
    /// master's. Returns the times saved; the photos keep their order.
    pub(crate) fn poll_capture_times(&mut self) -> Saved<Vec<(PhotoId, String)>> {
        let Some(reader) = &self.backfill.capture else {
            return Saved::default();
        };
        let (read, done) = reader.poll();
        if done {
            self.backfill.capture = None;
        }
        let dated: Vec<(PhotoId, String)> = read
            .into_iter()
            .filter_map(|(id, read)| match read {
                CaptureRead::Dated(time) => Some((id, time)),
                _ => None,
            })
            .collect();
        let mut out = Saved {
            start_again: done,
            ..Default::default()
        };
        if !dated.is_empty() {
            match self.catalog.fill_capture_times(&dated) {
                Ok(()) => {
                    self.apply_capture_times(&dated);
                    out.saved = dated;
                }
                Err(e) => out.error = Some(e),
            }
        }
        out
    }
    fn apply_capture_times(&mut self, times: &[(PhotoId, String)]) {
        let times: HashMap<PhotoId, &String> = times.iter().map(|(id, t)| (*id, t)).collect();
        for photo in &mut self.photos {
            if photo.captured.is_empty()
                && let Some(time) = times
                    .get(&photo.id)
                    .or_else(|| photo.master.and_then(|m| times.get(&m)))
            {
                photo.captured = (*time).clone();
            }
        }
    }
    /// Reads the info of the photos that have none, of those `available` says
    /// are online; asked while reading, it reads again once done.
    pub(crate) fn start_photo_info(&mut self, available: impl Fn(&Path) -> bool) {
        if self.backfill.info.is_some() {
            self.backfill.info_again = true;
            return;
        }
        let Ok(missing) = self.catalog.photos_without_info() else {
            return;
        };
        let missing: HashSet<PhotoId> = missing.into_iter().collect();
        let todo: Vec<_> = self
            .photos
            .iter()
            .filter(|p| missing.contains(&p.id) && available(&p.path))
            .map(|p| (p.id, p.path.clone()))
            .collect();
        if !todo.is_empty() {
            self.backfill.info = Some(Reader::start(todo, self.wake.clone(), read_info));
        }
    }
    /// Whether photo info is being read.
    pub(crate) fn reading_photo_info(&self) -> bool {
        self.backfill.info.is_some()
    }
    /// Saves the photo info read so far. Returns how many photos' info was
    /// saved; a file that could not be read is left for the next online check.
    pub(crate) fn poll_photo_info(&mut self) -> Saved<usize> {
        let Some(reader) = &self.backfill.info else {
            return Saved::default();
        };
        let (read, done) = reader.poll();
        let mut out = Saved::default();
        if done {
            self.backfill.info = None;
            out.start_again = std::mem::take(&mut self.backfill.info_again);
        }
        let infos: Vec<_> = read
            .into_iter()
            .filter_map(|(id, info)| Some((id, info?)))
            .collect();
        if !infos.is_empty() {
            match self.catalog.fill_photo_info(&infos) {
                Ok(()) => out.saved = infos.len(),
                Err(e) => out.error = Some(e),
            }
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq)]
enum CaptureRead {
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
fn has_capture_time(path: &Path) -> bool {
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

fn read_capture(path: &Path) -> CaptureRead {
    if !can_read(path) {
        return CaptureRead::Unreadable;
    }
    match crate::exif::capture_time(path) {
        Some(time) => CaptureRead::Dated(time),
        None => CaptureRead::Undated,
    }
}

/// The file's info: `None` when it cannot be read now, `Some(None)` when it
/// has none.
fn read_info(path: &Path) -> Option<Option<PhotoInfo>> {
    if !can_read(path) {
        return None;
    }
    Some(if crate::storage::is_raw(path) {
        // A RAW LibRaw cannot open now (still copying, a network error) is
        // tried again later.
        Some(PhotoInfo::from_metadata(
            &crate::photo::open(path).ok()?.metadata,
        ))
    } else {
        let mut info = crate::exif::photo_info(path).unwrap_or_default();
        // A header that cannot be read yet (a file still being copied) is
        // tried again later.
        info.dimensions = Some(raster_dimensions(path)?);
        (info != PhotoInfo::default()).then_some(info)
    })
}

/// A JPEG, TIFF or PNG's size as shown, after its EXIF orientation, from its
/// header alone.
fn raster_dimensions(path: &Path) -> Option<(u32, u32)> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let (w, h) = decoder.dimensions();
    let turned = decoder.orientation().is_ok_and(|o| {
        use image::metadata::Orientation::*;
        matches!(o, Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH)
    });
    Some(if turned { (h, w) } else { (w, h) })
}
