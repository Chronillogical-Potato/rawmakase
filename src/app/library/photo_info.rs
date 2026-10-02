//! Photo info (camera, lens, exposure, size) for the Metadata panel and the
//! Loupe. Photos imported from Lightroom have it from their catalog; for
//! photos added from folders it is read from the files in the background and
//! kept in the catalog, a row of nothing for a file without any.
use super::Library;
use crate::catalog::PhotoInfo;
use eframe::egui;
use std::path::Path;

impl Library {
    /// The active photo's info, read from the catalog once per photo.
    pub(super) fn active_info(&mut self) -> Option<PhotoInfo> {
        let id = self.selection.active?;
        if self.info.as_ref().is_none_or(|(at, _)| *at != id) {
            let info = self.catalog.photo_info(id).ok().flatten();
            self.info = Some((id, info));
        }
        self.info.as_ref().and_then(|(_, info)| info.clone())
    }
    /// A grid cell's hover, as Lightroom's: file name, capture time and
    /// dimensions. The info is read from the catalog once per hovered photo.
    pub(super) fn hover_text(&mut self, photo: &crate::catalog::Photo) -> String {
        if self
            .hover_info
            .as_ref()
            .is_none_or(|(id, _)| *id != photo.id)
        {
            let info = self.catalog.photo_info(photo.id).ok().flatten();
            self.hover_info = Some((photo.id, info));
        }
        let info = self.hover_info.as_ref().and_then(|(_, info)| info.as_ref());
        [
            Some(format!(
                "{}{}",
                photo.filename,
                super::cell::copy_suffix(photo)
            )),
            Some(photo.capture_text()).filter(|t| !t.is_empty()),
            info.and_then(PhotoInfo::dimensions_text),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
    }
    /// An expanded grid cell's details: dimensions and capture date, e.g.
    /// "6000 × 4000 · 29/06/2016". The info is read once per photo.
    pub(super) fn cell_details(&mut self, photo: &crate::catalog::Photo) -> String {
        let catalog = &self.catalog;
        let info = self
            .cell_info
            .entry(photo.id)
            .or_insert_with(|| catalog.photo_info(photo.id).ok().flatten());
        let date = photo
            .capture_text()
            .get(..10)
            .unwrap_or_default()
            .to_string();
        [
            info.as_ref().and_then(PhotoInfo::dimensions_text),
            Some(date).filter(|d| !d.is_empty()),
            Some(photo.format.clone()).filter(|f| !f.is_empty()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }
    /// Reads the info of the photos that have none, unless already reading.
    pub(super) fn start_photo_info(&mut self) {
        // Asked again while reading (a volume came back): once it is done.
        if self.info_reader.is_some() {
            self.info_again = true;
            return;
        }
        let Ok(missing) = self.catalog.photos_without_info() else {
            return;
        };
        let missing: std::collections::HashSet<i64> = missing.into_iter().collect();
        let todo: Vec<_> = self
            .photos
            .iter()
            .filter(|p| missing.contains(&p.id) && self.is_available(&p.path))
            .map(|p| (p.id, p.path.clone()))
            .collect();
        if !todo.is_empty() {
            self.info_reader = Some(super::background::Reader::start(todo, &self.ctx, read));
        }
    }
    /// Saves the info read so far.
    pub(super) fn poll_photo_info(&mut self) {
        let Some(reader) = &self.info_reader else {
            return;
        };
        let (read, done) = reader.poll();
        if done {
            self.info_reader = None;
            if std::mem::take(&mut self.info_again) {
                self.start_photo_info();
            }
        }
        // A file that could not be read is left for the next online check.
        let infos: Vec<_> = read
            .into_iter()
            .filter_map(|(id, info)| Some((id, info?)))
            .collect();
        if !infos.is_empty() {
            match self.catalog.fill_photo_info(&infos) {
                Ok(()) => {
                    self.info = None;
                    self.hover_info = None;
                    self.cell_info.clear();
                    // Sorted by aspect ratio, the new sizes find their places.
                    if self.filters.sort == super::sort::Sort::AspectRatio {
                        self.resort_in_place(|_| {});
                    }
                }
                Err(e) => self.message = format!("Photo info could not be saved: {e}"),
            }
        }
    }
}

/// What the Loupe's Info overlay (I) shows, as Lightroom's Info 1 and 2.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum Overlay {
    #[default]
    Off,
    /// File name, capture time and dimensions.
    Info1,
    /// File name, exposure, camera and lens.
    Info2,
}
impl Overlay {
    pub(super) fn next(self) -> Self {
        match self {
            Self::Off => Self::Info1,
            Self::Info1 => Self::Info2,
            Self::Info2 => Self::Off,
        }
    }
}

impl Library {
    /// Draws the Loupe's Info overlay in the top left of `rect`.
    pub(in crate::app) fn loupe_overlay(&mut self, painter: &egui::Painter, rect: egui::Rect) {
        if self.loupe_info == Overlay::Off {
            return;
        }
        let Some(photo) = self.selection.active.and_then(|id| self.photo(id)).cloned() else {
            return;
        };
        let info = self.active_info().unwrap_or_default();
        let name = format!("{}{}", photo.filename, super::cell::copy_suffix(&photo));
        let lines: Vec<String> = match self.loupe_info {
            Overlay::Info1 => vec![
                Some(name),
                Some(photo.capture_text()).filter(|t| !t.is_empty()),
                info.dimensions_text(),
            ],
            _ => vec![
                Some(name),
                Some(
                    [info.exposure_text(), info.focal_text(), info.iso_text()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(", "),
                )
                .filter(|s| !s.is_empty()),
                Some(
                    [info.camera.clone(), info.lens.clone()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join(" · "),
                )
                .filter(|s| !s.is_empty()),
            ],
        }
        .into_iter()
        .flatten()
        .collect();
        let mut y = rect.top() + 12.;
        for (i, line) in lines.iter().enumerate() {
            let size = if i == 0 { 15. } else { 12. };
            let font = egui::FontId::proportional(size);
            let at = egui::pos2(rect.left() + 14., y);
            // A shadow keeps the text readable on any photo.
            painter.text(
                at + egui::vec2(1., 1.),
                egui::Align2::LEFT_TOP,
                line,
                font.clone(),
                egui::Color32::from_black_alpha(200),
            );
            painter.text(
                at,
                egui::Align2::LEFT_TOP,
                line,
                font,
                crate::app::theme::gray(235),
            );
            y += size + 5.;
        }
    }
}

/// The file's info: `None` when it cannot be read now, `Some(None)` when it
/// has none.
fn read(path: &Path) -> Option<Option<PhotoInfo>> {
    if !super::background::can_read(path) {
        return None;
    }
    Some(if crate::storage::is_raw(path) {
        // A RAW LibRaw cannot open now (still copying, a network error) is
        // tried again later.
        Some(PhotoInfo::from_metadata(
            &crate::raw::Raw::open(path).ok()?.metadata,
        ))
    } else {
        let mut info = crate::export::exif::photo_info(path).unwrap_or_default();
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
