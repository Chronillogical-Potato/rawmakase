//! The EXIF directories an export writes: the camera's own when it is included,
//! with the export's size, orientation, resolution and software.
use super::{
    Embed, SOFTWARE,
    exif::{CameraExif, Field, tiff_block},
};
use crate::raw::Metadata;

pub(super) fn directories(m: &Metadata, embed: &Embed, width: u32, height: u32) -> CameraExif {
    let mut d = embed
        .camera
        .clone()
        .unwrap_or_else(|| CameraExif::from_libraw(m));
    if embed.camera_fallback && !d.main.iter().any(|f| f.tag == 0x010f) {
        d.main.extend(CameraExif::from_libraw(m).main);
    }
    let ppi = embed.ppi.clamp(1, 10_000);
    d.main
        .retain(|f| ![0x0112, 0x0131, 0x011a, 0x011b, 0x0128].contains(&f.tag));
    d.main.extend([
        Field::short(0x0112, 1),
        Field::rational(0x011a, ppi, 1),
        Field::rational(0x011b, ppi, 1),
        Field::short(0x0128, 2),
        Field::ascii(0x0131, SOFTWARE),
    ]);
    d.exif.retain(|f| f.tag != 0xa001);
    d.exif.extend([
        Field::short(0xa001, 1),
        Field::long(0xa002, width),
        Field::long(0xa003, height),
    ]);
    d
}

/// The APP1 EXIF payload of a JPEG export.
pub(super) fn jpeg_exif(mut d: CameraExif) -> Vec<u8> {
    // Required for JPEG by the EXIF standard: chroma samples are centered.
    d.main.push(Field::short(0x0213, 1));
    tiff_block(d)
}
