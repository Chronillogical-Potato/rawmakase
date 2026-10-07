//! The camera's own EXIF, read from a RAW, JPEG or TIFF: for an export to carry
//! as Lightroom's do (capture settings, lens, dates, serial numbers and GPS), and
//! for the catalog's capture time and photo info. Maker notes, thumbnail offsets
//! and other pointers into the RAW are left out; they would not survive the move
//! to a new file.
use crate::{
    jpeg::{APP1, Segments},
    tiff::{
        Entry, Tiff,
        kind::{ASCII, IFD, LONG, RATIONAL, SHORT, SRATIONAL},
    },
};
use std::{fs::File, io::Read, path::Path};
use tag::*;

/// The tags RAWmakase reads or writes, by their EXIF names: of the main
/// directory, the EXIF directory and the GPS directory.
pub mod tag {
    pub const IMAGE_DESCRIPTION: u16 = 0x010e;
    pub const MAKE: u16 = 0x010f;
    pub const MODEL: u16 = 0x0110;
    pub const ORIENTATION: u16 = 0x0112;
    pub const X_RESOLUTION: u16 = 0x011a;
    pub const Y_RESOLUTION: u16 = 0x011b;
    pub const RESOLUTION_UNIT: u16 = 0x0128;
    pub const SOFTWARE: u16 = 0x0131;
    pub const DATE_TIME: u16 = 0x0132;
    pub const ARTIST: u16 = 0x013b;
    pub const YCBCR_POSITIONING: u16 = 0x0213;
    /// The XMP packet, in a TIFF.
    pub const XMP: u16 = 0x02bc;
    pub const COPYRIGHT: u16 = 0x8298;
    pub const ICC_PROFILE: u16 = 0x8773;
    /// Pointers to the EXIF and GPS directories.
    pub const EXIF_IFD: u16 = 0x8769;
    pub const GPS_IFD: u16 = 0x8825;

    pub const EXPOSURE_TIME: u16 = 0x829a;
    pub const F_NUMBER: u16 = 0x829d;
    pub const ISO: u16 = 0x8827;
    pub const SENSITIVITY_TYPE: u16 = 0x8830;
    pub const STANDARD_OUTPUT_SENSITIVITY: u16 = 0x8831;
    pub const RECOMMENDED_EXPOSURE_INDEX: u16 = 0x8832;
    pub const ISO_SPEED: u16 = 0x8833;
    pub const DATE_TIME_ORIGINAL: u16 = 0x9003;
    pub const DATE_TIME_DIGITIZED: u16 = 0x9004;
    pub const OFFSET_TIME: u16 = 0x9010;
    pub const OFFSET_TIME_ORIGINAL: u16 = 0x9011;
    pub const OFFSET_TIME_DIGITIZED: u16 = 0x9012;
    /// APEX values of the exposure time and aperture.
    pub const SHUTTER_SPEED_VALUE: u16 = 0x9201;
    pub const APERTURE_VALUE: u16 = 0x9202;
    pub const FOCAL_LENGTH: u16 = 0x920a;
    pub const MAKER_NOTE: u16 = 0x927c;
    pub const SUBSEC_TIME: u16 = 0x9290;
    pub const SUBSEC_TIME_ORIGINAL: u16 = 0x9291;
    pub const SUBSEC_TIME_DIGITIZED: u16 = 0x9292;
    pub const COLOR_SPACE: u16 = 0xa001;
    pub const PIXEL_X_DIMENSION: u16 = 0xa002;
    pub const PIXEL_Y_DIMENSION: u16 = 0xa003;
    pub const INTEROPERABILITY_IFD: u16 = 0xa005;
    pub const LENS_MODEL: u16 = 0xa434;

    pub const GPS_VERSION_ID: u16 = 0x0000;
    pub const GPS_LATITUDE_REF: u16 = 0x0001;
    pub const GPS_LATITUDE: u16 = 0x0002;
    pub const GPS_LONGITUDE_REF: u16 = 0x0003;
    pub const GPS_LONGITUDE: u16 = 0x0004;
    pub const GPS_ALTITUDE_REF: u16 = 0x0005;
    pub const GPS_ALTITUDE: u16 = 0x0006;
}

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
            kind: ASCII,
            count: bytes.len() as u32,
            bytes,
        }
    }
    pub fn short(tag: u16, value: u16) -> Self {
        Self {
            tag,
            kind: SHORT,
            count: 1,
            bytes: value.to_le_bytes().to_vec(),
        }
    }
    pub fn long(tag: u16, value: u32) -> Self {
        Self {
            tag,
            kind: LONG,
            count: 1,
            bytes: value.to_le_bytes().to_vec(),
        }
    }
    pub fn rational(tag: u16, n: u32, d: u32) -> Self {
        let mut bytes = n.to_le_bytes().to_vec();
        bytes.extend(d.to_le_bytes());
        Self {
            tag,
            kind: RATIONAL,
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
            SHORT => Some(u16::from_le_bytes(self.bytes.get(..2)?.try_into().ok()?) as f64),
            LONG => word(0).map(f64::from),
            RATIONAL => Some(word(0)? as f64 / word(4).filter(|d| *d != 0)? as f64),
            SRATIONAL => {
                Some(word(0)? as i32 as f64 / (word(4).filter(|d| *d != 0)? as i32) as f64)
            }
            _ => None,
        }
    }
    /// Text of an ASCII field, without the terminator.
    pub fn text(&self) -> Option<String> {
        (self.kind == ASCII).then(|| {
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
        self.get(DATE_TIME_ORIGINAL)
            .and_then(Field::text)
            .filter(|s| !s.is_empty())
    }
    /// Capture settings LibRaw reports, for a RAW whose EXIF could not be read.
    pub fn from_libraw(m: &crate::camera_data::Metadata) -> Self {
        let rational =
            |tag, v: f32| Field::rational(tag, (v.max(0.) * 1_000_000.).round() as u32, 1_000_000);
        let main = vec![Field::ascii(MAKE, &m.make), Field::ascii(MODEL, &m.model)];
        let mut exif = vec![
            rational(EXPOSURE_TIME, m.shutter),
            rational(F_NUMBER, m.aperture),
            Field::short(ISO, m.iso.min(65535.) as u16),
            rational(FOCAL_LENGTH, m.focal),
        ];
        if !m.lens_model.is_empty() {
            exif.push(Field::ascii(LENS_MODEL, &m.lens_model));
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
const MAIN: [u16; 5] = [IMAGE_DESCRIPTION, MAKE, MODEL, ARTIST, COPYRIGHT];
/// EXIF tags an export replaces or drops: pixel dimensions (the export's own),
/// the interoperability pointer and the maker note, whose internal offsets point
/// into the RAW.
const SKIP: [u16; 4] = [
    PIXEL_X_DIMENSION,
    PIXEL_Y_DIMENSION,
    INTEROPERABILITY_IFD,
    MAKER_NOTE,
];
/// A value this long is not capture metadata (and would crowd the 64 KB JPEG
/// segment the EXIF must fit in).
pub(crate) const MAX_VALUE: usize = 4096;

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
    let exif = directories(path, |tag| tag == DATE_TIME)?;
    [
        (DATE_TIME_ORIGINAL, SUBSEC_TIME_ORIGINAL),
        (DATE_TIME_DIGITIZED, SUBSEC_TIME_DIGITIZED),
        (DATE_TIME, SUBSEC_TIME),
    ]
    .into_iter()
    .find_map(|(date, subseconds)| {
        let date = exif.get(date).and_then(Field::text)?;
        lightroom_time(&date, exif.get(subseconds).and_then(Field::text).as_deref())
    })
}

/// Camera, lens and exposure settings of a JPEG, TIFF or TIFF-based RAW
/// from its EXIF; `None` when it has none. Dimensions are not read here.
pub fn photo_info(path: &Path) -> Option<crate::metadata::PhotoInfo> {
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
    let info = crate::metadata::PhotoInfo {
        camera,
        lens: text(LENS_MODEL),
        focal: number(FOCAL_LENGTH),
        // FNumber and ExposureTime, else their APEX values.
        aperture: number(F_NUMBER).or_else(|| {
            let av = exif.get(APERTURE_VALUE).and_then(Field::number)?;
            Some(2f64.powf(av / 2.))
        }),
        exposure: number(EXPOSURE_TIME).or_else(|| {
            let tv = exif.get(SHUTTER_SPEED_VALUE).and_then(Field::number)?;
            Some(2f64.powf(-tv))
        }),
        iso: iso(&exif),
        dimensions: None,
    };
    (info != Default::default()).then_some(info)
}

/// The ISO speed. Above 65535 the EXIF 2.3 tag that SensitivityType
/// names holds it: standard output sensitivity, recommended
/// exposure index or ISO speed.
fn iso(exif: &CameraExif) -> Option<f64> {
    let number = |tag| exif.get(tag).and_then(Field::number).filter(|n| *n > 0.);
    let iso = number(ISO);
    if iso.is_some_and(|iso| iso < 65535.) {
        return iso;
    }
    let extended = match number(SENSITIVITY_TYPE).map(|t| t as u32) {
        Some(1 | 4 | 5 | 7) => STANDARD_OUTPUT_SENSITIVITY,
        Some(2 | 6) => RECOMMENDED_EXPOSURE_INDEX,
        Some(3) => ISO_SPEED,
        _ => {
            return [
                ISO_SPEED,
                RECOMMENDED_EXPOSURE_INDEX,
                STANDARD_OUTPUT_SENSITIVITY,
            ]
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
        .get(&EXIF_IFD)
        .and_then(|e| t.offset(e))
        .and_then(|at| t.ifd(at))
    {
        out.exif = fields(&mut t, &ifd, |tag| !SKIP.contains(&tag) && tag < 0xc000);
    }
    if let Some(ifd) = main
        .get(&GPS_IFD)
        .and_then(|e| t.offset(e))
        .and_then(|at| t.ifd(at))
    {
        out.gps = fields(&mut t, &ifd, |tag| tag <= 0x1f);
    }
    (!out.exif.is_empty() || !out.main.is_empty()).then_some(out)
}

/// Offset of the TIFF header inside a JPEG's EXIF segment, relative to the JPEG.
/// Only the first `length` bytes, at most 64 KB, are read.
fn exif_in_jpeg(f: &mut File, jpeg: u64, length: usize) -> Option<u64> {
    use std::io::{Cursor, Seek, SeekFrom};
    f.seek(SeekFrom::Start(jpeg)).ok()?;
    let mut head = vec![0u8; length.min(1 << 16)];
    f.read_exact(&mut head).ok()?;
    let mut segments = Segments::new(Cursor::new(head)).ok()??;
    while let Some(segment) = segments.next().ok()? {
        let mut signature = [0u8; 6];
        if segment.marker == APP1
            && segments.reader().read_exact(&mut signature).is_ok()
            && signature == *b"Exif\0\0"
        {
            return Some(segment.offset + 6);
        }
    }
    None
}

fn fields(
    t: &mut Tiff,
    ifd: &std::collections::BTreeMap<u16, Entry>,
    keep: impl Fn(u16) -> bool,
) -> Vec<Field> {
    ifd.iter()
        .filter(|(tag, e)| keep(**tag) && e.kind != IFD)
        .filter_map(|(tag, e)| {
            let size = e.type_size()?;
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
