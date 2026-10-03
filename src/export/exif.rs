//! The camera's own EXIF, read from the RAW so an export carries it as Lightroom's
//! do: capture settings, lens, dates, serial numbers and GPS. Maker notes, thumbnail
//! offsets and other pointers into the RAW are left out; they would not survive the
//! move to a new file.
use crate::tiff::{Entry, Tiff};
use std::{fs::File, io::Read, path::Path};

/// A TIFF field with its value in little-endian byte order.
#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub tag: u16,
    pub kind: u16,
    pub count: u32,
    pub bytes: Vec<u8>,
}
impl Field {
    pub fn ascii(tag: u16, value: &str) -> Self {
        let mut bytes = value.as_bytes().to_vec();
        bytes.push(0);
        Self {
            tag,
            kind: 2,
            count: bytes.len() as u32,
            bytes,
        }
    }
    pub fn short(tag: u16, value: u16) -> Self {
        Self {
            tag,
            kind: 3,
            count: 1,
            bytes: value.to_le_bytes().to_vec(),
        }
    }
    pub fn long(tag: u16, value: u32) -> Self {
        Self {
            tag,
            kind: 4,
            count: 1,
            bytes: value.to_le_bytes().to_vec(),
        }
    }
    pub fn rational(tag: u16, n: u32, d: u32) -> Self {
        let mut bytes = n.to_le_bytes().to_vec();
        bytes.extend(d.to_le_bytes());
        Self {
            tag,
            kind: 5,
            count: 1,
            bytes,
        }
    }
    /// The first value of a numeric field: SHORT, LONG or (S)RATIONAL.
    pub fn number(&self) -> Option<f64> {
        let word = |at: usize| {
            Some(u32::from_le_bytes(
                self.bytes.get(at..at + 4)?.try_into().ok()?,
            ))
        };
        match self.kind {
            3 => Some(u16::from_le_bytes(self.bytes.get(..2)?.try_into().ok()?) as f64),
            4 => word(0).map(f64::from),
            5 => Some(word(0)? as f64 / word(4).filter(|d| *d != 0)? as f64),
            10 => Some(word(0)? as i32 as f64 / (word(4).filter(|d| *d != 0)? as i32) as f64),
            _ => None,
        }
    }
    /// Text of an ASCII field, without the terminator.
    pub fn text(&self) -> Option<String> {
        (self.kind == 2).then(|| {
            String::from_utf8_lossy(&self.bytes)
                .trim_end_matches('\0')
                .trim()
                .to_string()
        })
    }
}

/// The directories an export copies from the camera.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CameraExif {
    /// Make, model, artist and copyright from the main directory.
    pub main: Vec<Field>,
    /// The EXIF directory: exposure, lens, dates, serial numbers…
    pub exif: Vec<Field>,
    pub gps: Vec<Field>,
}
impl CameraExif {
    pub fn get(&self, tag: u16) -> Option<&Field> {
        self.main
            .iter()
            .chain(&self.exif)
            .find(|field| field.tag == tag)
    }
    /// DateTimeOriginal as "2018:08:26 10:39:33".
    pub fn captured(&self) -> Option<String> {
        self.get(0x9003)
            .and_then(Field::text)
            .filter(|s| !s.is_empty())
    }
    /// Capture settings LibRaw reports, for a RAW whose EXIF could not be read.
    pub fn from_libraw(m: &crate::raw::Metadata) -> Self {
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
        Self {
            main,
            exif,
            gps: Vec::new(),
        }
    }
}

/// Main-directory tags an export keeps; it writes its own orientation, size and
/// software.
const MAIN: [u16; 5] = [0x010e, 0x010f, 0x0110, 0x013b, 0x8298];
/// EXIF tags an export replaces or drops: pixel dimensions (the export's own),
/// the interoperability pointer and the maker note, whose internal offsets point
/// into the RAW.
const SKIP: [u16; 4] = [0xa002, 0xa003, 0xa005, 0x927c];
/// A value this long is not capture metadata (and would crowd the 64 KB JPEG
/// segment the EXIF must fit in).
pub(super) const MAX_VALUE: usize = 4096;

/// Reads the camera EXIF of a TIFF-based RAW (ARW, NEF, CR2, DNG, ORF, RW2, PEF…),
/// a JPEG or TIFF, or a Fujifilm RAF, whose EXIF lives in its preview JPEG.
/// `None` when the file has none that can be read, e.g. a CR3.
pub fn read(path: &Path) -> Option<CameraExif> {
    directories(path, |tag| MAIN.contains(&tag))
}

/// When the photo was taken, in the form Lightroom stores it: the camera's
/// local time with no zone and three-digit subseconds,
/// "2018-08-26T10:39:33.120". Falls back from DateTimeOriginal to
/// DateTimeDigitized, then DateTime, each with its own subseconds.
pub fn capture_time(path: &Path) -> Option<String> {
    const DATE_TIME: u16 = 0x0132;
    let exif = directories(path, |tag| tag == DATE_TIME)?;
    [(0x9003, 0x9291), (0x9004, 0x9292), (DATE_TIME, 0x9290)]
        .into_iter()
        .find_map(|(date, subseconds)| {
            let date = exif.get(date).and_then(Field::text)?;
            lightroom_time(&date, exif.get(subseconds).and_then(Field::text).as_deref())
        })
}

/// Camera, lens and exposure settings of a JPEG, TIFF or TIFF-based RAW
/// from its EXIF; `None` when it has none. Dimensions are not read here.
pub fn photo_info(path: &Path) -> Option<crate::catalog::PhotoInfo> {
    const MAKE: u16 = 0x010f;
    const MODEL: u16 = 0x0110;
    let exif = directories(path, |tag| tag == MAKE || tag == MODEL)?;
    let text = |tag| {
        exif.get(tag)
            .and_then(Field::text)
            .filter(|t| !t.is_empty())
    };
    let number = |tag| exif.get(tag).and_then(Field::number).filter(|n| *n > 0.);
    // Lightroom shows the model, which usually names the make too.
    let camera = match (text(MAKE), text(MODEL)) {
        (Some(make), Some(model)) if !model.starts_with(make.split(' ').next().unwrap_or("")) => {
            Some(format!("{make} {model}"))
        }
        (make, model) => model.or(make),
    };
    let info = crate::catalog::PhotoInfo {
        camera,
        lens: text(0xa434),
        focal: number(0x920a),
        // FNumber and ExposureTime, else their APEX values.
        aperture: number(0x829d).or_else(|| {
            let av = exif.get(0x9202).and_then(Field::number)?;
            Some(2f64.powf(av / 2.))
        }),
        exposure: number(0x829a).or_else(|| {
            let tv = exif.get(0x9201).and_then(Field::number)?;
            Some(2f64.powf(-tv))
        }),
        iso: iso(&exif),
        dimensions: None,
    };
    (info != Default::default()).then_some(info)
}

/// The ISO speed. Above 65535 the EXIF 2.3 tag that SensitivityType
/// (0x8830) names holds it: standard output sensitivity, recommended
/// exposure index or ISO speed.
fn iso(exif: &CameraExif) -> Option<f64> {
    let number = |tag| exif.get(tag).and_then(Field::number).filter(|n| *n > 0.);
    let iso = number(0x8827);
    if iso.is_some_and(|iso| iso < 65535.) {
        return iso;
    }
    let extended = match number(0x8830).map(|t| t as u32) {
        Some(1 | 4 | 5 | 7) => 0x8831,
        Some(2 | 6) => 0x8832,
        Some(3) => 0x8833,
        _ => {
            return [0x8833, 0x8832, 0x8831]
                .into_iter()
                .find_map(number)
                .or(iso);
        }
    };
    number(extended).or(iso)
}

/// "YYYY:MM:DD HH:MM:SS" and optional subsecond digits as
/// "YYYY-MM-DDTHH:MM:SS.fff"; `None` for a blank or zeroed date.
pub(crate) fn lightroom_time(date: &str, subseconds: Option<&str>) -> Option<String> {
    let digits: Vec<u8> = date.bytes().filter(u8::is_ascii_digit).collect();
    let shape = date.trim().len() >= 19
        && date
            .trim()
            .bytes()
            .take(19)
            .enumerate()
            .all(|(i, b)| match i {
                4 | 7 => matches!(b, b':' | b'-'),
                10 => matches!(b, b' ' | b'T'),
                13 | 16 => b == b':',
                _ => b.is_ascii_digit(),
            });
    if !shape || digits[..8].iter().all(|d| *d == b'0') {
        return None;
    }
    let d = |range: std::ops::Range<usize>| std::str::from_utf8(&digits[range]).unwrap_or("");
    let mut fraction: String = subseconds
        .unwrap_or("")
        .trim()
        .chars()
        .take_while(char::is_ascii_digit)
        .take(3)
        .collect();
    while fraction.len() < 3 {
        fraction.push('0');
    }
    Some(format!(
        "{}-{}-{}T{}:{}:{}.{fraction}",
        d(0..4),
        d(4..6),
        d(6..8),
        d(8..10),
        d(10..12),
        d(12..14)
    ))
}

fn directories(path: &Path, keep_main: impl Fn(u16) -> bool) -> Option<CameraExif> {
    let mut f = File::open(path).ok()?;
    let mut head = [0u8; 108];
    f.read_exact(&mut head).ok()?;
    let base = if head.starts_with(b"FUJIFILMCCD-RAW") {
        let jpeg = u32::from_be_bytes(head[84..88].try_into().ok()?) as u64;
        let length = u32::from_be_bytes(head[88..92].try_into().ok()?) as usize;
        jpeg + exif_in_jpeg(&mut File::open(path).ok()?, jpeg, length)?
    } else if head.starts_with(&[0xff, 0xd8]) {
        let length = f.metadata().ok()?.len().min(1 << 16) as usize;
        exif_in_jpeg(&mut File::open(path).ok()?, 0, length)?
    } else {
        0
    };
    let mut t = Tiff::open(File::open(path).ok()?, base)?;
    let main = t.ifd(t.first)?;
    let mut out = CameraExif {
        main: fields(&mut t, &main, keep_main),
        ..Default::default()
    };
    if let Some(ifd) = main
        .get(&0x8769)
        .and_then(|e| t.offset(e))
        .and_then(|at| t.ifd(at))
    {
        out.exif = fields(&mut t, &ifd, |tag| !SKIP.contains(&tag) && tag < 0xc000);
    }
    if let Some(ifd) = main
        .get(&0x8825)
        .and_then(|e| t.offset(e))
        .and_then(|at| t.ifd(at))
    {
        out.gps = fields(&mut t, &ifd, |tag| tag <= 0x1f);
    }
    (!out.exif.is_empty() || !out.main.is_empty()).then_some(out)
}

/// Offset of the TIFF header inside a JPEG's EXIF segment, relative to the JPEG.
fn exif_in_jpeg(f: &mut File, jpeg: u64, length: usize) -> Option<u64> {
    use std::io::{Seek, SeekFrom};
    f.seek(SeekFrom::Start(jpeg)).ok()?;
    let mut head = vec![0u8; length.min(1 << 16)];
    f.read_exact(&mut head).ok()?;
    let mut at = 2;
    while at + 10 <= head.len() && head[at] == 0xff {
        let marker = head[at + 1];
        let size = u16::from_be_bytes([head[at + 2], head[at + 3]]) as usize;
        if marker == 0xe1 && head[at + 4..at + 10] == *b"Exif\0\0" {
            return Some(at as u64 + 10);
        }
        if marker == 0xda {
            return None;
        }
        at += 2 + size;
    }
    None
}

fn fields(
    t: &mut Tiff,
    ifd: &std::collections::BTreeMap<u16, Entry>,
    keep: impl Fn(u16) -> bool,
) -> Vec<Field> {
    ifd.iter()
        .filter(|(tag, e)| keep(**tag) && !matches!(e.kind, 13))
        .filter_map(|(tag, e)| {
            let size = match e.kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 => 4,
                5 | 10 | 12 => 8,
                _ => return None,
            };
            if e.count as usize * size > MAX_VALUE {
                return None;
            }
            let mut bytes = t.raw(e)?;
            if !t.little && size > 1 {
                // Rationals are two 4-byte numbers; every other type is one number.
                for chunk in bytes.chunks_mut(size.min(4)) {
                    chunk.reverse();
                }
            }
            Some(Field {
                tag: *tag,
                kind: e.kind,
                count: e.count,
                bytes,
            })
        })
        .collect()
}

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
    main.retain(|f| f.tag != 0x8769 && f.tag != 0x8825);
    if has_exif {
        main.push(Field::long(0x8769, 0));
    }
    if has_gps {
        main.push(Field::long(0x8825, 0));
    }
    // Directories follow each other; the pointers depend only on sizes.
    let exif_at = 8 + directory(main.clone(), 8).len() as u32;
    let exif_block = directory(exif, exif_at);
    let gps_at = exif_at + if has_exif { exif_block.len() as u32 } else { 0 };
    for f in &mut main {
        match f.tag {
            0x8769 => f.bytes = exif_at.to_le_bytes().to_vec(),
            0x8825 => f.bytes = gps_at.to_le_bytes().to_vec(),
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
#[cfg(test)]
pub(crate) fn dated_file(jpeg: bool, original: &str, subseconds: &str) -> Vec<u8> {
    let mut exif = vec![Field::ascii(0x9003, original)];
    if !subseconds.is_empty() {
        exif.push(Field::ascii(0x9291, subseconds));
    }
    let block = tiff_block(CameraExif {
        main: vec![Field::ascii(0x010f, "Test")],
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
