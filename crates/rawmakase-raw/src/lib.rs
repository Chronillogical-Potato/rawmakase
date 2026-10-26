//! RAWmakase's own RAW decoders, in pure Rust: containers, maker notes,
//! sensor codecs, camera facts and embedded previews.
//!
//! Three operations, cheapest first, each on the file's bytes in memory:
//! [`open`] reads the header facts ([`RawInfo`]) without decoding the
//! sensor data, [`preview`] finds the embedded JPEG, and [`decode`] unpacks
//! the sensor data ([`Sensor`]), checking `cancel` as it goes. One selector
//! ([`Variant`]) decides which reader and codec handle a file; a file of a
//! known container in a variant not decoded here is [`Error::Unsupported`].
//! [`capture_dates`] reads, from the file itself, the few parts that date a
//! photo in the containers whose EXIF is not a TIFF at the file's start.
#![forbid(unsafe_code)]
mod captured;
mod codec;
mod color;
mod container;
mod error;
mod facts;
mod geometry;
mod info;
mod io;
mod levels;
mod names;
mod preview;
mod sensor;
mod variant;
mod vendor;

pub use captured::{CaptureDates, Stamp};
pub use error::{Error, Result};
pub use info::{Cfa, Presets, RawInfo, SuperCcd, UNSET};
pub use preview::{NoPreview, is_jpeg_xl};
pub use sensor::Sensor;
use std::sync::atomic::AtomicBool;
pub use variant::Variant;

/// Nikon High Efficiency (JPEG XS) codestream decoding, for the LibRaw
/// bridge's unpack until RAWmakase's own decoders replace it.
pub use codec::nikon_he::decode as decode_nikon_high_efficiency;
/// Synthetic High Efficiency codestreams, for other crates' tests.
#[cfg(feature = "test-support")]
pub use codec::nikon_he::tests as nikon_high_efficiency_test_support;

/// The decoders' version, for the About window and fingerprints.
pub const VERSION: &str = concat!("RAWmakase ", env!("CARGO_PKG_VERSION"));

/// The header facts of the RAW file in `bytes`.
pub fn open(bytes: &[u8]) -> Result<RawInfo> {
    variant::open(bytes)
}

/// A camera's make and model as its file writes them, before RAWmakase's
/// naming: "Canon" and "Canon EOS R6m2" where [`RawInfo`] says "Canon" and
/// "EOS R6 Mark II".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileNames {
    pub make: String,
    pub model: String,
}

/// The make and model the RAW file in `bytes` writes, from its header.
pub fn file_camera_names(bytes: &[u8]) -> Result<FileNames> {
    let info = open(bytes)?;
    Ok(FileNames {
        make: info.file_make,
        model: info.file_model,
    })
}

/// The maker a file's make names, as RAWmakase spells it ("SONY" is
/// "Sony", "NIKON CORPORATION" "Nikon"); another maker's name, trimmed.
pub fn maker_name(file_make: &str) -> String {
    names::make(file_make)
}

/// When the CR3, CRW, MRW or X3F `file` was taken, from the parts of it
/// that say so, nothing else read; none for another file or one that can't
/// be read. Holds for files whose sensor data is not decoded here (X3F).
pub fn capture_dates(file: &mut (impl std::io::Read + std::io::Seek)) -> CaptureDates {
    captured::read(file)
}

/// The embedded preview of the file `info` was read from, JPEG or JPEG XL
/// ([`is_jpeg_xl`] tells which; a JPEG is copied when its start of image
/// marker needs mending).
pub fn preview<'a>(
    info: &RawInfo,
    bytes: &'a [u8],
) -> std::result::Result<std::borrow::Cow<'a, [u8]>, NoPreview> {
    let (at, len) = preview::find(info, bytes)?;
    let range = usize::try_from(at)
        .ok()
        .zip(usize::try_from(at.saturating_add(len)).ok());
    let jpeg = range
        .and_then(|(start, end)| bytes.get(start..end))
        .ok_or(NoPreview::Missing)?;
    Ok(match jpeg {
        [first, rest @ ..] if *first != 0xff && !preview::is_jpeg_xl(jpeg, 0) => {
            let mut mended = vec![0xff];
            mended.extend_from_slice(rest);
            std::borrow::Cow::Owned(mended)
        }
        _ => std::borrow::Cow::Borrowed(jpeg),
    })
}

/// The sensor data of the file `info` was read from; stops with
/// [`Error::Cancelled`] soon after `cancel` is set.
pub fn decode(info: &RawInfo, bytes: &[u8], cancel: &AtomicBool) -> Result<Sensor> {
    variant::decode(info, bytes, cancel)
}
