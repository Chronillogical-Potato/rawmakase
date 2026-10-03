//! The EXIF directories an export writes: the camera's own when it is included,
//! with the export's size, orientation, resolution and software.
use super::{
    Embed,
    exif::{Field, tiff_block},
};
use crate::raw::Metadata;

pub(super) struct Directories {
    pub main: Vec<Field>,
    pub exif: Vec<Field>,
    pub gps: Vec<Field>,
}

/// Capture settings LibRaw reports, for a RAW whose EXIF could not be read.
pub(super) fn from_metadata(m: &Metadata) -> (Vec<Field>, Vec<Field>) {
    let rational =
        |tag, v: f32| Field::rational(tag, (v.max(0.) * 1_000_000.).round() as u32, 1_000_000);
    let main = vec![
        Field::ascii(0x010f, &m.make),
        Field::ascii(0x0110, &m.model),
    ];
    let mut exif = vec![
        rational(0x829a, m.shutter),
        rational(0x829d, m.aperture),
        Field::short(0x8827, m.iso.min(65535.) as u16),
        rational(0x920a, m.focal),
    ];
    if !m.lens_model.is_empty() {
        exif.push(Field::ascii(0xa434, &m.lens_model));
    }
    (main, exif)
}

pub(super) fn directories(m: &Metadata, embed: &Embed, width: u32, height: u32) -> Directories {
    let (mut main, mut exif, mut gps) = match (&embed.camera, embed.capture) {
        (_, false) => (Vec::new(), Vec::new(), Vec::new()),
        (Some(camera), true) => (camera.main.clone(), camera.exif.clone(), camera.gps.clone()),
        (None, true) => {
            let (main, exif) = from_metadata(m);
            (main, exif, Vec::new())
        }
    };
    if embed.capture && embed.camera_fallback && !main.iter().any(|f| f.tag == 0x010f) {
        main.extend(from_metadata(m).0);
    }
    if !embed.location {
        gps.clear();
    }
    let ppi = embed.ppi.clamp(1, 10_000);
    main.retain(|f| ![0x0112, 0x0131, 0x011a, 0x011b, 0x0128].contains(&f.tag));
    main.extend([
        Field::short(0x0112, 1),
        Field::rational(0x011a, ppi, 1),
        Field::rational(0x011b, ppi, 1),
        Field::short(0x0128, 2),
        Field::ascii(0x0131, &format!("RAWmakase {}", env!("CARGO_PKG_VERSION"))),
    ]);
    exif.retain(|f| f.tag != 0xa001);
    exif.extend([
        Field::short(0xa001, 1),
        Field::long(0xa002, width),
        Field::long(0xa003, height),
    ]);
    Directories { main, exif, gps }
}

/// The APP1 EXIF payload of a JPEG export.
pub(super) fn jpeg_exif(mut d: Directories) -> Vec<u8> {
    // Required for JPEG by the EXIF standard: chroma samples are centered.
    d.main.push(Field::short(0x0213, 1));
    tiff_block(d.main, d.exif, d.gps)
}
