//! The EXIF an export writes into a JPEG: the camera's directories (see
//! `crate::exif`) as one little-endian TIFF block.
use crate::exif::{
    CameraExif, Field,
    tag::{EXIF_IFD, GPS_IFD},
};

/// A little-endian TIFF block for a JPEG APP1 segment: the main directory, then
/// the EXIF and GPS directories it points to.
pub(super) fn tiff_block(d: CameraExif) -> Vec<u8> {
    fn directory(mut fields: Vec<Field>, offset: u32) -> Vec<u8> {
        fields.sort_by_key(|f| f.tag);
        let mut out = (fields.len() as u16).to_le_bytes().to_vec();
        let mut payload = Vec::new();
        let start = offset + 2 + fields.len() as u32 * 12 + 4;
        for f in fields {
            out.extend(f.tag.to_le_bytes());
            out.extend(f.kind.to_le_bytes());
            out.extend(f.count.to_le_bytes());
            let mut bytes = f.bytes;
            if bytes.len() <= 4 {
                bytes.resize(4, 0);
                out.extend(bytes);
            } else {
                out.extend((start + payload.len() as u32).to_le_bytes());
                payload.extend(bytes);
                if payload.len() % 2 != 0 {
                    payload.push(0);
                }
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(payload);
        out
    }
    let CameraExif {
        mut main,
        exif,
        gps,
    } = d;
    let has_exif = !exif.is_empty();
    let has_gps = !gps.is_empty();
    main.retain(|f| f.tag != EXIF_IFD && f.tag != GPS_IFD);
    if has_exif {
        main.push(Field::long(EXIF_IFD, 0));
    }
    if has_gps {
        main.push(Field::long(GPS_IFD, 0));
    }
    // Directories follow each other; the pointers depend only on sizes.
    let exif_at = 8 + directory(main.clone(), 8).len() as u32;
    let exif_block = directory(exif, exif_at);
    let gps_at = exif_at + if has_exif { exif_block.len() as u32 } else { 0 };
    for f in &mut main {
        match f.tag {
            EXIF_IFD => f.bytes = exif_at.to_le_bytes().to_vec(),
            GPS_IFD => f.bytes = gps_at.to_le_bytes().to_vec(),
            _ => {}
        }
    }
    let mut out = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
    out.extend(directory(main, 8));
    if has_exif {
        out.extend(exif_block);
    }
    if has_gps {
        out.extend(directory(gps, gps_at));
    }
    out
}

/// A small TIFF, or a JPEG when `jpeg`, taken at `original` with these
/// subseconds, for tests.
#[cfg(any(test, feature = "test-support"))]
pub fn dated_file(jpeg: bool, original: &str, subseconds: &str) -> Vec<u8> {
    use crate::exif::tag::{DATE_TIME_ORIGINAL, MAKE, SUBSEC_TIME_ORIGINAL};
    let mut exif = vec![Field::ascii(DATE_TIME_ORIGINAL, original)];
    if !subseconds.is_empty() {
        exif.push(Field::ascii(SUBSEC_TIME_ORIGINAL, subseconds));
    }
    let block = tiff_block(CameraExif {
        main: vec![Field::ascii(MAKE, "Test")],
        exif,
        gps: Vec::new(),
    });
    let mut out = if jpeg {
        let mut out = vec![0xff, 0xd8, 0xff, 0xe1];
        out.extend(((block.len() + 8) as u16).to_be_bytes());
        out.extend(b"Exif\0\0");
        out.extend(block);
        out.extend([0xff, 0xd9]);
        out
    } else {
        block
    };
    // Real files are never shorter than the header the reader starts with.
    out.resize(out.len().max(512), 0);
    out
}
