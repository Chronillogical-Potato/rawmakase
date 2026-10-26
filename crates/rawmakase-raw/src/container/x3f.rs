//! Sigma X3F (Foveon): "FOVb" and a header, then sections, each starting
//! with its own mark, and the directory ("SECd": a version, a count, then
//! each section's offset, length and type) at the offset the file's last
//! four bytes give. Image sections ("SECi", types `IMAG` and `IMA2`) have a
//! 28-byte header before their data: the sensor data, thumbnails and, on
//! the SD14 and later bodies, a JPEG preview with the camera's EXIF. The
//! property list ("SECp", type `PROP`) is a count of name and value
//! offsets into UTF-16 text, among them TIME, when the photo was taken in
//! seconds since 1970 on the camera's clock; the sd Quattro and later bodies
//! write none. RAWmakase decodes no X3F sensor data: only the capture dates
//! are read here.
//!
//! Sources: Foveon's public description of the X3F format (the header, the
//! directory, the sections and the property list), ExifTool's SigmaRaw tag
//! names, and our own measurements on raw.pixls.us's X3F files from the SD9
//! to the sd Quattro H (TIME and the preview's DateTimeOriginal agree on
//! every file that has both).
use super::tiff::TiffFile;
use crate::captured::{CaptureDates, exif_date};
use crate::io::Pieces;

/// The most sections and properties read: files with more are malformed.
const MAX_SECTIONS: u64 = 64;
const MAX_PROPERTIES: u64 = 4096;
/// An image section's header, before its data.
const IMAGE_HEADER: u64 = 28;
/// How much of a JPEG preview is read for its EXIF, which an APP1 segment
/// of at most 64 KB holds, after a JFIF segment at most as long.
const JPEG_HEAD: u64 = 1 << 17;

/// Whether `bytes` is an X3F file.
pub(crate) fn is_x3f(bytes: &[u8]) -> bool {
    bytes.starts_with(b"FOVb")
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// The capture dates of the first JPEG preview with EXIF, the property
/// list's TIME standing in for a DateTimeOriginal it lacks; the directory
/// and these sections' starts read alone.
pub(crate) fn capture_dates(f: &mut Pieces) -> CaptureDates {
    let mut dates = CaptureDates::default();
    let sections = directory(f).unwrap_or_default();
    for &(at, len, kind) in &sections {
        if !matches!(&kind, b"IMAG" | b"IMA2") || len <= IMAGE_HEADER {
            continue;
        }
        // The section's start, all in the file: longer than its header.
        let take = (IMAGE_HEADER + JPEG_HEAD).min(len);
        let Some(head) = f.read(at, take) else {
            continue;
        };
        if head.starts_with(b"SECi")
            && let Some(exif) = super::tiff_exif::in_jpeg(&head, IMAGE_HEADER, take - IMAGE_HEADER)
            && let Some(t) = TiffFile::parse(&head, exif)
        {
            super::tiff_exif::dates(&t, &mut dates);
            break;
        }
    }
    if dates.original.date.is_none() {
        dates.original.date = sections
            .iter()
            .filter(|s| &s.2 == b"PROP")
            .find_map(|&(at, len, _)| time(f, at, len))
            .and_then(exif_date);
    }
    dates
}

/// The directory's sections: offset, length and type.
fn directory(f: &mut Pieces) -> Option<Vec<(u64, u64, [u8; 4])>> {
    let at = u64::from(u32_at(&f.read(f.len.checked_sub(4)?, 4)?, 0)?);
    let head = f.read(at, 12)?;
    if !head.starts_with(b"SECd") {
        return None;
    }
    let count = u64::from(u32_at(&head, 8)?).min(MAX_SECTIONS);
    let entries = f.read(at + 12, 12 * count)?;
    Some(
        entries
            .as_chunks::<12>()
            .0
            .iter()
            .filter_map(|e| {
                let kind = e[8..12].try_into().ok()?;
                Some((u64::from(u32_at(e, 0)?), u64::from(u32_at(e, 4)?), kind))
            })
            .collect(),
    )
}

/// The TIME property of the property list `len` bytes at `at`: the mark,
/// a version, the count, the text's format (0, UTF-16), a reserved word and
/// the text's length in characters, then each property's name and value
/// offsets in characters, then the text.
fn time(f: &mut Pieces, at: u64, len: u64) -> Option<u64> {
    let p = f.read(at, len)?;
    let count = u64::from(u32_at(&p, 8)?);
    if !p.starts_with(b"SECp") || u32_at(&p, 12)? != 0 || count > MAX_PROPERTIES {
        return None;
    }
    let text: Vec<u16> = p
        .get(usize::try_from(24 + 8 * count).ok()?..)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    let string = |offset: u32| {
        let s = text.get(usize::try_from(offset).ok()?..)?;
        let end = s.iter().position(|&c| c == 0)?;
        String::from_utf16(&s[..end]).ok()
    };
    (0..count as usize).find_map(|i| {
        let entry = 24 + 8 * i;
        if string(u32_at(&p, entry)?)? != "TIME" {
            return None;
        }
        string(u32_at(&p, entry + 4)?)?.trim().parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use crate::Stamp;
    use std::io::Cursor;

    /// A property list of `properties`.
    fn prop(properties: &[(&str, &str)]) -> Vec<u8> {
        let mut text: Vec<u16> = Vec::new();
        let mut entries = Vec::new();
        for (name, value) in properties {
            for s in [name, value] {
                entries.extend((text.len() as u32).to_le_bytes());
                text.extend(s.encode_utf16());
                text.push(0);
            }
        }
        let mut out = b"SECp".to_vec();
        for v in [
            0x0001_0000,
            properties.len() as u32,
            0,
            0,
            text.len() as u32,
        ] {
            out.extend(v.to_le_bytes());
        }
        out.extend(entries);
        out.extend(text.iter().flat_map(|c| c.to_le_bytes()));
        out
    }

    /// An image section of `data`.
    fn image(data: &[u8]) -> Vec<u8> {
        let mut out = b"SECi".to_vec();
        for v in [0x0001_0000u32, 2, 18, 4, 4, 0] {
            out.extend(v.to_le_bytes());
        }
        out.extend(data);
        out
    }

    /// A JPEG of a JFIF segment and the EXIF `tiff`.
    fn jpeg(tiff: &[u8]) -> Vec<u8> {
        let mut out = vec![0xff, 0xd8, 0xff, 0xe0, 0, 16];
        out.extend(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        out.extend([0xff, 0xe1]);
        out.extend(((tiff.len() + 8) as u16).to_be_bytes());
        out.extend(b"Exif\0\0");
        out.extend(tiff);
        out.extend([0xff, 0xd9]);
        out
    }

    /// An X3F of `sections`, typed, then the directory.
    fn x3f(sections: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut file = b"FOVb".to_vec();
        file.extend(0x0002_0003u32.to_le_bytes());
        file.resize(232, 0);
        let mut entries = Vec::new();
        for (kind, body) in sections {
            entries.extend((file.len() as u32).to_le_bytes());
            entries.extend((body.len() as u32).to_le_bytes());
            entries.extend(*kind);
            file.extend(body);
        }
        let dir = file.len() as u32;
        file.extend(b"SECd");
        file.extend(0x0002_0000u32.to_le_bytes());
        file.extend((sections.len() as u32).to_le_bytes());
        file.extend(entries);
        file.extend(dir.to_le_bytes());
        file
    }

    fn stamp(date: &str, subseconds: Option<&str>) -> Stamp {
        Stamp {
            date: Some(date.into()),
            subseconds: subseconds.map(Into::into),
        }
    }

    #[test]
    fn the_preview_s_exif_dates_an_x3f_and_time_stands_in_for_its_original() {
        use super::super::test_tiff::{E, ascii, tiff};
        use crate::io::Order;
        let exif = tiff(
            b"II*\0",
            Order::Little,
            vec![ascii(271, "SIGMA"), ascii(306, "2021:09:04 17:20:46")],
            vec![
                ascii(36867, "2021:09:04 17:20:45"),
                ascii(36868, "2021:09:04 17:20:44"),
                ascii(37521, "5"),
                E(37520, 2, b"7\0".to_vec()),
            ],
            &[],
        );
        let raw = image(&[0; 64]);
        // An SD1 Merrill's: the preview, then the properties. Without a
        // date in the EXIF, TIME is the original.
        let file = x3f(&[
            (b"IMA2", raw.clone()),
            (b"IMA2", image(&jpeg(&exif))),
            (
                b"PROP",
                prop(&[("CAMMODEL", "SD1"), ("TIME", "1630776000")]),
            ),
        ]);
        let dates = crate::capture_dates(&mut Cursor::new(&file));
        assert_eq!(dates.original, stamp("2021:09:04 17:20:45", Some("5")));
        assert_eq!(dates.digitized, stamp("2021:09:04 17:20:44", None));
        assert_eq!(dates.modified, stamp("2021:09:04 17:20:46", Some("7")));
        // An SD9's: no preview, TIME alone.
        let file = x3f(&[
            (b"IMAG", raw),
            (
                b"PROP",
                prop(&[("CAMMANUF", "SIGMA"), ("TIME", "1050151205")]),
            ),
        ]);
        let dates = crate::capture_dates(&mut Cursor::new(&file));
        assert_eq!(dates.original, stamp("2003:04:12 12:40:05", None));
        assert_eq!(dates.digitized, Stamp::default());
    }

    #[test]
    fn damaged_x3f_files_have_no_dates_and_do_not_panic() {
        // An image section said to be 100 bytes, 4 bytes from the end.
        let short: Vec<u8> = [
            0x6256_4f46u32,
            0x6443_4553,
            0x0002_0000,
            1,
            28,
            100,
            u32::from_le_bytes(*b"IMAG"),
            4,
        ]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
        assert_eq!(
            crate::capture_dates(&mut Cursor::new(&short)),
            crate::CaptureDates::default()
        );
        let exif = super::super::test_tiff::tiff(
            b"II*\0",
            crate::io::Order::Little,
            vec![super::super::test_tiff::ascii(306, "2021:09:04 17:20:46")],
            Vec::new(),
            &[],
        );
        let file = x3f(&[
            (b"IMA2", image(&jpeg(&exif))),
            (b"PROP", prop(&[("TIME", "1050151205")])),
        ]);
        let good = crate::capture_dates(&mut Cursor::new(&file));
        assert!(good.original.date.is_some() && good.modified.date.is_some());
        for end in 0..file.len() {
            let _ = crate::capture_dates(&mut Cursor::new(&file[..end]));
        }
        for i in 0..file.len() {
            for v in [0, 0x7f, 0xff] {
                let mut d = file.clone();
                d[i] = v;
                let _ = crate::capture_dates(&mut Cursor::new(&d));
            }
        }
        // A TIME that is not a number is none.
        let file = x3f(&[(b"PROP", prop(&[("TIME", "noon")]))]);
        assert_eq!(
            crate::capture_dates(&mut Cursor::new(&file)),
            crate::CaptureDates::default()
        );
    }
}
