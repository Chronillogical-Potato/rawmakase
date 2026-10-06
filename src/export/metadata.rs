//! The EXIF directories an export writes: the camera's own when it is included,
//! with the export's size, orientation, resolution and software.
use super::{Embed, exif::tiff_block};
use crate::build_info::SOFTWARE;
use crate::{
    exif::{
        CameraExif, Field,
        tag::{
            self, COLOR_SPACE, MAKE, ORIENTATION, PIXEL_X_DIMENSION, PIXEL_Y_DIMENSION,
            RESOLUTION_UNIT, X_RESOLUTION, Y_RESOLUTION, YCBCR_POSITIONING,
        },
    },
    raw::Metadata,
};

pub(super) fn directories(m: &Metadata, embed: &Embed, width: u32, height: u32) -> CameraExif {
    let mut d = embed
        .camera
        .clone()
        .unwrap_or_else(|| CameraExif::from_libraw(m));
    if embed.camera_fallback && !d.main.iter().any(|f| f.tag == MAKE) {
        d.main.extend(CameraExif::from_libraw(m).main);
    }
    let ppi = embed.ppi.clamp(1, 10_000);
    d.main.retain(|f| {
        ![
            ORIENTATION,
            tag::SOFTWARE,
            X_RESOLUTION,
            Y_RESOLUTION,
            RESOLUTION_UNIT,
        ]
        .contains(&f.tag)
    });
    d.main.extend([
        Field::short(ORIENTATION, 1),
        Field::rational(X_RESOLUTION, ppi, 1),
        Field::rational(Y_RESOLUTION, ppi, 1),
        Field::short(RESOLUTION_UNIT, 2),
        Field::ascii(tag::SOFTWARE, SOFTWARE),
    ]);
    d.exif.retain(|f| f.tag != COLOR_SPACE);
    d.exif.extend([
        Field::short(COLOR_SPACE, 1),
        Field::long(PIXEL_X_DIMENSION, width),
        Field::long(PIXEL_Y_DIMENSION, height),
    ]);
    d
}

/// The APP1 EXIF payload of a JPEG export.
pub(super) fn jpeg_exif(mut d: CameraExif) -> Vec<u8> {
    // Required for JPEG by the EXIF standard: chroma samples are centered.
    d.main.push(Field::short(YCBCR_POSITIONING, 1));
    tiff_block(d)
}
