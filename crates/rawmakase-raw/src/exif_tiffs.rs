//! Where a RAW file keeps its camera's EXIF, for the containers whose EXIF
//! is not a TIFF at the file's start: Canon's CR3 (`CMT1`, `CMT2` and
//! `CMT4` in Canon's box in `moov`), Minolta's MRW (the TIFF in its TTW
//! block) and Sigma's X3F (its JPEG preview's EXIF). Only these parts of the
//! file are read, never the sensor data. A CRW's CIFF records hold no TIFF.
//! Reading the TIFFs' fields is the EXIF reader's (`rawmakase-interop`).
use crate::container::{cr3, mrw, x3f};
use crate::io::Pieces;

/// The TIFFs a RAW container keeps its camera's EXIF in, each on its own:
/// offsets in it are from its start.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExifTiffs {
    /// A TIFF whose first directory is IFD0 (Make, Model…): a CR3's
    /// `CMT1`, or an MRW's or X3F's whole TIFF, whose IFD0 points to its
    /// EXIF directory.
    pub main: Option<Vec<u8>>,
    /// A TIFF whose first directory is the EXIF directory: a CR3's `CMT2`.
    pub exif: Option<Vec<u8>>,
    /// A TIFF whose first directory is the GPS directory: a CR3's `CMT4`.
    pub gps: Option<Vec<u8>>,
}

/// The camera's EXIF of the CR3, MRW or X3F `f`, whose first bytes are
/// `head`; none for another file.
pub(crate) fn read(f: &mut Pieces, head: &[u8]) -> ExifTiffs {
    if cr3::is_cr3(head) {
        cr3::exif_tiffs(f)
    } else if mrw::is_mrw(head) {
        mrw::exif_tiffs(f)
    } else if x3f::is_x3f(head) {
        x3f::exif_tiffs(f)
    } else {
        ExifTiffs::default()
    }
}
