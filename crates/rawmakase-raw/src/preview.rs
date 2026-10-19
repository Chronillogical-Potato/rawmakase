//! Embedded JPEG previews, found without decoding the sensor data: each
//! container lists its candidates, and the largest is the one thumbnails use.
use crate::info::RawInfo;

/// An embedded JPEG: where it is and, when known, its size in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub offset: u64,
    pub len: u64,
    pub pixels: u64,
}

/// The candidate thumbnails use: the most pixels, then the most bytes.
pub(crate) fn largest(candidates: &[Candidate], bytes: &[u8]) -> Option<(u64, u64)> {
    candidates
        .iter()
        .filter(|c| c.len > 2 && is_jpeg(bytes, c.offset))
        .max_by_key(|c| (c.pixels, c.len))
        .map(|c| (c.offset, c.len))
}

/// Whether a JPEG starts at `at`: its start of image marker, or Epson's
/// (whose first byte is 0xEE) or Minolta's (whose first byte is another,
/// before the second marker's 0xFF), the first byte read as 0xFF (our
/// measurements).
pub(crate) fn is_jpeg(bytes: &[u8], at: u64) -> bool {
    usize::try_from(at)
        .ok()
        .and_then(|at| bytes.get(at..at.checked_add(3)?))
        .is_some_and(|b| b[..2] == [0xff, 0xd8] || b[..2] == [0xee, 0xd8] || b[1..] == [0xd8, 0xff])
}

/// The width and height of the JPEG `len` bytes at `at`, from its first
/// start-of-frame segment (ITU T.81 B.2.2); `None` when the markers before
/// it do not lead there within the JPEG.
pub(crate) fn jpeg_size(bytes: &[u8], at: u64, len: u64) -> Option<[u32; 2]> {
    let start = usize::try_from(at).ok()?;
    let end = start.checked_add(usize::try_from(len).ok()?)?;
    let jpeg = bytes.get(start..end.min(bytes.len()))?;
    let mut i = 2;
    while let [0xff, marker, ..] = *jpeg.get(i..)? {
        match marker {
            // Fill bytes before a marker.
            0xff => i += 1,
            // Start of frame, other than DHT, JPG and DAC: precision, then
            // height and width.
            0xc0..=0xcf if !matches!(marker, 0xc4 | 0xc8 | 0xcc) => {
                let f = jpeg.get(i + 4..i + 9)?;
                let (h, w) = (
                    u16::from_be_bytes([f[1], f[2]]),
                    u16::from_be_bytes([f[3], f[4]]),
                );
                return (w > 0 && h > 0).then_some([u32::from(w), u32::from(h)]);
            }
            // Start of scan or end of image before a frame.
            0xda | 0xd9 => return None,
            // Markers without a length.
            0x01 | 0xd0..=0xd7 => i += 2,
            _ => {
                let l = jpeg.get(i + 2..i + 4)?;
                i += 2 + usize::from(u16::from_be_bytes([l[0], l[1]]));
            }
        }
    }
    None
}

/// Why a file has no preview thumbnails can use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoPreview {
    /// The file has none.
    Missing,
    /// The file's previews are not JPEG (uncompressed thumbnails).
    NotJpeg,
}

impl std::fmt::Display for NoPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            NoPreview::Missing => "No thumbnail in file",
            NoPreview::NotJpeg => "Embedded preview is not JPEG",
        })
    }
}

impl std::error::Error for NoPreview {}

/// Where the preview thumbnails use is in the file: offset and length.
pub(crate) fn find(info: &RawInfo, bytes: &[u8]) -> Result<(u64, u64), NoPreview> {
    largest(&info.previews, bytes).ok_or(if info.uncompressed_preview {
        NoPreview::NotJpeg
    } else {
        NoPreview::Missing
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_jpeg_may_start_with_another_first_byte() {
        let bytes = [0xff, 0xd8, 0xff, 0x02, 0xd8, 0xff, 0x02, 0xd8, 0x00];
        assert!(super::is_jpeg(&bytes, 0));
        assert!(super::is_jpeg(&bytes, 3));
        // Without the second marker's 0xFF, not a JPEG; nor past the end.
        assert!(!super::is_jpeg(&bytes, 6));
        assert!(!super::is_jpeg(&bytes, 8));
        assert!(!super::is_jpeg(&bytes, u64::MAX));
    }
}
