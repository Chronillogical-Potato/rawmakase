//! Where a photo falls in a sequence its camera shot (a burst in continuous
//! drive, a bracket), as the camera's maker note says: for Auto-Stack's
//! Group Camera Bursts. Read from public tag documentation (ExifTool's tag
//! names): Sony's SequenceNumber (0xb04a), Fujifilm's SequenceNumber
//! (0x1101), Panasonic's SequenceNumber (0x002b) and Apple's BurstUUID
//! (0x000b). Sony, Fujifilm and Panasonic number the frames of each press
//! from 1, one taken alone in continuous drive included, and write 0 for a
//! photo taken in single drive; an iPhone gives a burst's photos one id.
//! Other cameras' notes are not read: Canon's sequence counter is 0 in most
//! files, Nikon writes none, and Olympus's drive mode differs between
//! generations.
use super::{
    CameraExif, Field, Source, Tiff, camera_exif, exif_in_jpeg, open_tiff,
    tag::{BODY_SERIAL_NUMBER, EXIF_IFD, MAKE, MAKER_NOTE, MODEL},
};
use crate::tiff::{
    Entry,
    kind::{ASCII, LONG, SHORT},
};
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::Path;

/// A photo's place in its camera's sequences.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Sequence {
    /// The camera that numbered it: make, model and body serial number when
    /// the EXIF has one. Two bodies' frame numbers are never compared.
    pub camera: String,
    pub mark: Mark,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Mark {
    /// One of the photos of a burst, which share this id.
    Burst(String),
    /// The `n`th frame (from 1) of a press of the shutter in continuous
    /// drive or bracketing.
    Frame(u32),
    /// Taken in single drive: of no sequence.
    Single,
}

/// The sequence the photo at `path` is in, as its maker note says; `None`
/// when the file can't be read, has no maker note this reads, or the note
/// doesn't say.
pub fn sequence(path: &Path) -> Option<Sequence> {
    match open_tiff(path)? {
        Source::File(t) => sequence_of(t).or_else(|| in_jpg_from_raw(path)),
        Source::Memory(t) => sequence_of(t),
    }
}

/// [`sequence`] of a Panasonic RW2, which keeps its maker note in the EXIF
/// of the JPEG its first directory holds (JpgFromRaw, 0x002e), not in its
/// own EXIF directory.
fn in_jpg_from_raw(path: &Path) -> Option<Sequence> {
    let mut t = Tiff::open(File::open(path).ok()?, 0)?;
    let main = t.ifd(t.first)?;
    let e = main.get(&JPG_FROM_RAW)?;
    let jpeg = t.u32(&e.value) as u64;
    let (at, len) = exif_in_jpeg(&mut File::open(path).ok()?, jpeg, e.count as usize)?;
    sequence_of(Tiff::open_within(File::open(path).ok()?, jpeg + at, len)?)
}

/// A Panasonic RW2's preview JPEG, with the camera's whole EXIF.
const JPG_FROM_RAW: u16 = 0x002e;

/// [`sequence`] of the EXIF in `t`.
fn sequence_of<R: Read + Seek>(mut t: Tiff<R>) -> Option<Sequence> {
    let main = t.ifd(t.first)?;
    let exif_ifd = main
        .get(&EXIF_IFD)
        .and_then(|e| t.offset(e))
        .and_then(|at| t.ifd(at))?;
    let note = t.raw(exif_ifd.get(&MAKER_NOTE)?)?;
    let exif = camera_exif(t, |tag| tag == MAKE || tag == MODEL)?;
    let text = |exif: &CameraExif, tag| exif.get(tag).and_then(Field::text).unwrap_or_default();
    let make = text(&exif, MAKE);
    let mark = maker_mark(&make, &note)?;
    Some(Sequence {
        camera: [make, text(&exif, MODEL), text(&exif, BODY_SERIAL_NUMBER)].join("\n"),
        mark,
    })
}

/// The mark in maker note `note` of a camera by `make`.
fn maker_mark(make: &str, note: &[u8]) -> Option<Mark> {
    let make = make.trim().to_ascii_uppercase();
    if make.starts_with("SONY") {
        sony(note)
    } else if make.starts_with("FUJIFILM") {
        fujifilm(note)
    } else if make.starts_with("PANASONIC") {
        panasonic(note)
    } else if make.starts_with("APPLE") {
        apple(note)
    } else {
        None
    }
}

/// A frame number, 0 for single drive.
fn frame(n: u32) -> Mark {
    if n == 0 { Mark::Single } else { Mark::Frame(n) }
}

/// Sony: a directory at the note's start (a RAW's), or after a 12-byte
/// "SONY DSC " or "SONY CAM " header (a JPEG's), in the EXIF's byte order,
/// which its entry count tells; another header isn't read.
/// SequenceNumber (0xb04a): 0 in single drive, 65535 when the camera doesn't
/// say.
fn sony(note: &[u8]) -> Option<Mark> {
    let at = if note.starts_with(b"SONY DSC \0\0\0") || note.starts_with(b"SONY CAM \0\0\0") {
        12
    } else if note.starts_with(b"SONY") {
        return None;
    } else {
        0
    };
    let n = int(&mut directory(note, at, None)?, 0xb04a)?;
    (n != 0xffff).then(|| frame(n))
}

/// Fujifilm: "FUJIFILM", then the little-endian offset of its directory from
/// the note's start. SequenceNumber (0x1101).
fn fujifilm(note: &[u8]) -> Option<Mark> {
    let header = note.strip_prefix(b"FUJIFILM")?;
    let at = u32::from_le_bytes(header.get(..4)?.try_into().ok()?) as u64;
    Some(frame(int(&mut directory(note, at, Some(true))?, 0x1101)?))
}

/// Panasonic: "Panasonic\0\0\0", then a directory in the EXIF's byte order.
/// SequenceNumber (0x002b).
fn panasonic(note: &[u8]) -> Option<Mark> {
    if !note.starts_with(b"Panasonic\0") {
        return None;
    }
    Some(frame(int(&mut directory(note, 12, None)?, 0x002b)?))
}

/// Apple: "Apple iOS\0", a version, "MM", then a big-endian directory
/// whose offsets are from the note's start. BurstUUID (0x000b); a photo
/// without one is no burst's.
fn apple(note: &[u8]) -> Option<Mark> {
    if !note.starts_with(b"Apple iOS\0") || note.get(12..14) != Some(b"MM") {
        return None;
    }
    let mut d = directory(note, 14, Some(false))?;
    let id = d
        .entries
        .get(&0x000b)
        .filter(|e| e.kind == ASCII)
        .and_then(|e| d.tiff.raw(e))
        .map(|b| {
            String::from_utf8_lossy(&b)
                .trim_end_matches('\0')
                .trim()
                .to_string()
        })
        .filter(|id| !id.is_empty());
    Some(id.map_or(Mark::Single, Mark::Burst))
}

/// A maker note's directory, its offsets from the note's start.
struct Directory<'a> {
    tiff: Tiff<Cursor<&'a [u8]>>,
    entries: std::collections::BTreeMap<u16, Entry>,
}

/// The directory `at` bytes into `note`, little-endian or not as `little`
/// says; `None` lets its entry count tell, as a count is under 256.
fn directory(note: &[u8], at: u64, little: Option<bool>) -> Option<Directory<'_>> {
    let little = little.unwrap_or_else(|| {
        let count = note.get(at as usize..at as usize + 2).unwrap_or(&[0, 0]);
        count[0] != 0 || count[1] == 0
    });
    let mut tiff = Tiff::headless(Cursor::new(note), 0, little);
    let entries = tiff.ifd(at)?;
    Some(Directory { tiff, entries })
}

/// The first value of SHORT or LONG entry `tag`.
fn int(d: &mut Directory<'_>, tag: u16) -> Option<u32> {
    let e = d.entries.get(&tag)?;
    let b = d.tiff.raw(e)?;
    match e.kind {
        SHORT => Some(d.tiff.u16(b.get(..2)?) as u32),
        LONG => Some(d.tiff.u32(b.get(..4)?)),
        _ => None,
    }
}

/// Files with maker notes, for tests.
#[cfg(any(test, feature = "test-support"))]
pub mod fixture {
    /// A TIFF with Make, Model and an EXIF directory holding `note` as its
    /// maker note (and a body serial number when given), in either byte
    /// order.
    pub fn tiff(little: bool, make: &str, model: &str, serial: &str, note: &[u8]) -> Vec<u8> {
        let u16b = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let u32b = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let text = |s: &str| [s.as_bytes(), b"\0"].concat();
        let (make, model, serial) = (text(make), text(model), text(serial));
        let main_at = 8u32;
        let exif_at = main_at + 2 + 3 * 12 + 4;
        let data_at = exif_at + 2 + 2 * 12 + 4;
        let make_at = data_at;
        let model_at = make_at + make.len() as u32;
        let serial_at = model_at + model.len() as u32;
        let note_at = serial_at + serial.len() as u32;
        let mut t = if little {
            b"II*\0".to_vec()
        } else {
            b"MM\0*".to_vec()
        };
        t.extend(u32b(main_at));
        let entry = |t: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32| {
            t.extend(u16b(tag));
            t.extend(u16b(kind));
            t.extend(u32b(count));
            t.extend(u32b(value));
        };
        t.extend(u16b(3));
        entry(&mut t, 0x010f, 2, make.len() as u32, make_at);
        entry(&mut t, 0x0110, 2, model.len() as u32, model_at);
        entry(&mut t, 0x8769, 4, 1, exif_at);
        t.extend([0; 4]);
        t.extend(u16b(2));
        entry(&mut t, 0x927c, 7, note.len() as u32, note_at);
        // A value of four bytes or less is held in the entry itself.
        if serial.len() <= 4 {
            t.extend(u16b(0xa431));
            t.extend(u16b(2));
            t.extend(u32b(serial.len() as u32));
            let mut inline = serial.clone();
            inline.resize(4, 0);
            t.extend(inline);
        } else {
            entry(&mut t, 0xa431, 2, serial.len() as u32, serial_at);
        }
        t.extend([0; 4]);
        t.extend(make.iter().chain(&model).chain(&serial).chain(note));
        t
    }

    /// A maker-note directory of `entries` (tag, kind, count, and value or
    /// offset from the directory's start), then `data`.
    pub fn note_ifd(little: bool, entries: &[(u16, u16, u32, u32)], data: &[u8]) -> Vec<u8> {
        let u16b = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let u32b = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let mut d = u16b(entries.len() as u16).to_vec();
        for (tag, kind, count, value) in entries {
            d.extend(u16b(*tag));
            d.extend(u16b(*kind));
            d.extend(u32b(*count));
            // A SHORT is left-justified in its value field.
            if *kind == 3 && *count == 1 {
                d.extend(u16b(*value as u16));
                d.extend([0, 0]);
            } else {
                d.extend(u32b(*value));
            }
        }
        d.extend([0; 4]);
        d.extend(data);
        d
    }

    /// A Sony maker note saying frame `n` of a continuous burst, in either
    /// byte order, with or without the "SONY DSC " header.
    pub fn sony_note(header: bool, little: bool, n: u16) -> Vec<u8> {
        let mut note = if header {
            b"SONY DSC \0\0\0".to_vec()
        } else {
            Vec::new()
        };
        note.extend(note_ifd(
            little,
            &[(0xb049, 3, 1, 2), (0xb04a, 3, 1, n as u32)],
            &[],
        ));
        note
    }

    /// A Sony RAW (a TIFF) of `serial`'s body, frame `n` of its burst; 0 for
    /// a single shot.
    pub fn sony_raw(serial: &str, n: u16) -> Vec<u8> {
        tiff(true, "SONY", "ILCE-7M4", serial, &sony_note(false, true, n))
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::{note_ifd, sony_note, tiff};
    use super::{Mark, Sequence, sequence};

    /// `tiff` in a JPEG's APP1 segment.
    fn jpeg(tiff: &[u8]) -> Vec<u8> {
        let mut j = vec![0xff, 0xd8, 0xff, 0xe1];
        j.extend(((tiff.len() + 8) as u16).to_be_bytes());
        j.extend(b"Exif\0\0");
        j.extend(tiff);
        j.extend([0xff, 0xd9]);
        j
    }

    fn read(file: &[u8], name: &str) -> Option<Sequence> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, file).unwrap();
        sequence(&path)
    }

    #[test]
    fn sony_frames_are_read_in_either_byte_order_with_or_without_a_header() {
        for (little, header) in [(true, false), (true, true), (false, true), (false, false)] {
            let file = tiff(
                little,
                "SONY",
                "ILCE-7M4",
                "",
                &sony_note(header, little, 3),
            );
            let read = read(&file, "a.arw").unwrap();
            assert_eq!(read.mark, Mark::Frame(3), "little {little} header {header}");
            assert_eq!(read.camera, "SONY\nILCE-7M4\n");
        }
        let single = tiff(true, "SONY", "ILCE-7M4", "", &sony_note(false, true, 0));
        assert_eq!(read(&single, "a.arw").unwrap().mark, Mark::Single);
        let unknown = tiff(
            true,
            "SONY",
            "ILCE-7M4",
            "",
            &sony_note(false, true, 0xffff),
        );
        assert_eq!(read(&unknown, "a.arw"), None);
        let mut other = b"SONY MOBILE\0".to_vec();
        other.extend(&sony_note(false, true, 3));
        assert_eq!(read(&tiff(true, "SONY", "X", "", &other), "a.jpg"), None);
    }

    #[test]
    fn fujifilm_frames_are_read_from_a_jpeg_and_a_raf_preview() {
        let mut note = b"FUJIFILM".to_vec();
        note.extend(12u32.to_le_bytes());
        note.extend(note_ifd(true, &[(0x1100, 3, 1, 1), (0x1101, 3, 1, 2)], &[]));
        // Fujifilm's EXIF is big-endian in a RAF's preview, its note not.
        let exif = jpeg(&tiff(false, "FUJIFILM", "X-T5", "", &note));
        assert_eq!(read(&exif, "a.jpg").unwrap().mark, Mark::Frame(2));
        let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        raf.resize(84, 0);
        raf.extend(108u32.to_be_bytes());
        raf.extend((exif.len() as u32).to_be_bytes());
        raf.resize(108, 0);
        raf.extend(&exif);
        assert_eq!(read(&raf, "a.raf").unwrap().mark, Mark::Frame(2));
    }

    #[test]
    fn panasonic_frames_and_single_drive() {
        let note = |n| {
            let mut note = b"Panasonic\0\0\0".to_vec();
            note.extend(note_ifd(true, &[(0x002a, 3, 1, 1), (0x002b, 4, 1, n)], &[]));
            note
        };
        let file = tiff(true, "Panasonic", "DC-S5", "", &note(4));
        assert_eq!(read(&file, "a.rw2").unwrap().mark, Mark::Frame(4));
        // An RW2: its own EXIF without the note, which is in its JpgFromRaw's.
        let preview = jpeg(&file);
        let mut rw2 = b"IIU\0\x08\0\0\0".to_vec();
        rw2.extend(1u16.to_le_bytes());
        rw2.extend(0x002eu16.to_le_bytes());
        rw2.extend(7u16.to_le_bytes());
        rw2.extend((preview.len() as u32).to_le_bytes());
        rw2.extend(26u32.to_le_bytes());
        rw2.extend([0; 4]);
        rw2.extend(&preview);
        assert_eq!(read(&rw2, "a.rw2").unwrap().mark, Mark::Frame(4));
        let file = tiff(true, "Panasonic", "DC-S5", "", &note(0));
        assert_eq!(read(&file, "a.rw2").unwrap().mark, Mark::Single);
    }

    #[test]
    fn an_iphone_burst_is_its_uuid_and_other_photos_are_single() {
        let note = |uuid: Option<&str>| {
            let mut note = b"Apple iOS\0\0\x01MM".to_vec();
            let entries_len = 2 + 2 * 12 + 4;
            let at = (14 + entries_len) as u32;
            let mut entries = vec![(0x0001, 9, 1, 14)];
            let mut data = Vec::new();
            if let Some(uuid) = uuid {
                data = [uuid.as_bytes(), b"\0"].concat();
                entries.push((0x000b, 2, data.len() as u32, at));
            }
            note.extend(note_ifd(false, &entries, &data));
            note
        };
        let uuid = "D1E2C3B4-0000-4A4A-8B8B-123456789ABC";
        let burst = jpeg(&tiff(
            false,
            "Apple",
            "iPhone 15 Pro",
            "",
            &note(Some(uuid)),
        ));
        assert_eq!(
            read(&burst, "a.jpg").unwrap().mark,
            Mark::Burst(uuid.into())
        );
        let single = jpeg(&tiff(false, "Apple", "iPhone 15 Pro", "", &note(None)));
        assert_eq!(read(&single, "a.jpg").unwrap().mark, Mark::Single);
    }

    #[test]
    fn the_body_serial_number_tells_two_bodies_apart() {
        let a = tiff(true, "SONY", "ILCE-1", "111", &sony_note(false, true, 1));
        let b = tiff(true, "SONY", "ILCE-1", "222", &sony_note(false, true, 1));
        assert_ne!(
            read(&a, "a.arw").unwrap().camera,
            read(&b, "b.arw").unwrap().camera
        );
    }

    #[test]
    fn other_makes_and_broken_notes_read_as_none() {
        let note = sony_note(false, true, 2);
        assert_eq!(
            read(&tiff(true, "Canon", "EOS R5", "", &note), "a.cr2"),
            None
        );
        for broken in [
            Vec::new(),
            b"SONY DSC \0\0\0".to_vec(),
            note[..note.len() / 2].to_vec(),
            b"FUJIFILM\xff\xff\xff\xff".to_vec(),
            b"Apple iOS\0\0\x01II".to_vec(),
        ] {
            for make in ["SONY", "FUJIFILM", "Apple"] {
                assert_eq!(read(&tiff(true, make, "X", "", &broken), "a.tif"), None);
            }
        }
        // A directory said to hold more entries than the note does.
        let mut huge = vec![0xe7, 0x03];
        huge.extend([0; 20]);
        assert_eq!(read(&tiff(true, "SONY", "X", "", &huge), "a.arw"), None);
    }

    #[test]
    fn exports_still_leave_the_maker_note_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.arw");
        std::fs::write(
            &path,
            tiff(true, "SONY", "ILCE-7M4", "", &sony_note(false, true, 2)),
        )
        .unwrap();
        let exif = super::super::read(&path).unwrap();
        assert!(exif.get(super::MAKER_NOTE).is_none());
        assert!(sequence(&path).is_some());
    }
}
