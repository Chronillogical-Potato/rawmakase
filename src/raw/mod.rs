//! RAW files: opening them through LibRaw, their metadata as RAWmakase keeps it
//! (with the DNG, RAF and lens details read on top), development into linear
//! camera-space pixels, the embedded preview, and the monitor colour transform.
//! The native boundary itself is in [`ffi`].
use anyhow::{Result, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
mod ffi;
pub use ffi::{display_transform, srgb_profile, version};
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metadata {
    pub make: String,
    pub model: String,
    pub width: u32,
    pub height: u32,
    pub raw_width: u32,
    pub raw_height: u32,
    pub crop_width: u32,
    pub crop_height: u32,
    pub crop_left: u32,
    pub crop_top: u32,
    pub flip: i32,
    pub xtrans: bool,
    #[serde(default)]
    pub fuji_dynamic_range: u32,
    /// Canon Highlight Tone Priority, from the maker notes; `Off` for other makes.
    #[serde(default)]
    pub highlight_tone_priority: HighlightTonePriority,
    /// Fujifilm's exposure midpoint shift in EV (maker note ExpoMidPointShift): about
    /// −0.7 at DR100, a stop lower for each DR step up, a stop higher at extended
    /// low ISO. `None` when the raw has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fuji_exposure_shift: Option<f32>,
    pub iso: f32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal: f32,
    /// Focal length in 35mm equivalent (EXIF FocalLengthIn35mmFilm); 0 when unknown.
    #[serde(default)]
    pub focal_35mm: f32,
    pub wb: [f32; 3],
    pub daylight_wb: [f32; 3],
    pub matrix: [[f32; 3]; 3],
    /// LibRaw's XYZ(D65)-to-camera matrix, equivalent to a DNG ColorMatrix. Zero when unknown.
    #[serde(default)]
    pub cam_xyz: [[f32; 3]; 3],
    /// Built-in lens correction stored by the camera, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lens: Option<crate::lens::LensCorrection>,
    /// Lens model as recorded by the camera, e.g. "FE 55mm F1.8 ZA".
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub lens_model: String,
    /// DNG BaselineExposure (0 when the DNG has none); `None` for other formats.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_exposure: Option<f32>,
    /// Imported Adobe lens profiles that fit this camera, Enable Profile Corrections'
    /// choices; rebuilt on open.
    #[serde(skip)]
    pub lens_profiles: crate::lens::lcp::PhotoProfiles,
    /// Lateral chromatic aberration measured from the decoded image, shared by every
    /// image made from it (see `crate::lens::auto_ca::prime`).
    #[serde(skip)]
    pub lateral_ca: std::sync::Arc<std::sync::OnceLock<Option<[crate::lens::Radial; 2]>>>,
    /// Camera profile embedded in a DNG; rebuilt from the file on open.
    #[serde(skip)]
    pub embedded_profile: Option<std::sync::Arc<crate::camera_profiles::CameraProfile>>,
}
/// Canon Highlight Tone Priority: the camera exposes a stop darker to keep
/// highlights, and Camera Raw brightens the photo by that stop again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HighlightTonePriority {
    #[default]
    Off,
    On,
    /// "Enhanced" (D+2) on recent bodies. No sample has been measured yet.
    Enhanced,
}
impl HighlightTonePriority {
    /// From LibRaw's `makernotes.canon.HighlightTonePriority`.
    fn from_libraw(v: i32) -> Self {
        match v {
            1 => Self::On,
            2 => Self::Enhanced,
            _ => Self::Off,
        }
    }
}
/// Which demosaic full-size development uses. A process-wide preference: the app sets
/// it from its settings, and RAWMAKASE_LIBRAW_DEMOSAIC=1 forces LibRaw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Demosaic {
    /// RAWmakase's own demosaic of LibRaw-unpacked data (`crate::demosaic`): about
    /// 2–5× faster, with equal or better detail against Adobe renders.
    #[default]
    Rawmakase,
    /// LibRaw's AHD (Bayer) and 1-pass Markesteijn (X-Trans).
    Libraw,
}
static DEMOSAIC: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
pub fn set_demosaic(d: Demosaic) {
    DEMOSAIC.store(d as u8, Ordering::Relaxed);
}
pub fn demosaic() -> Demosaic {
    if std::env::var_os("RAWMAKASE_LIBRAW_DEMOSAIC").is_some_and(|v| v != "0")
        || DEMOSAIC.load(Ordering::Relaxed) == Demosaic::Libraw as u8
    {
        Demosaic::Libraw
    } else {
        Demosaic::Rawmakase
    }
}
pub struct Raw {
    handle: ffi::Handle,
    pub metadata: Metadata,
}
#[derive(Clone)]
pub struct CameraImage {
    pub width: u32,
    pub height: u32,
    pub recovered: std::sync::OnceLock<std::sync::Arc<CameraImage>>,
    pub pixels: Vec<[f32; 3]>,
    pub metadata: Metadata,
    pub fast: bool,
    pub scale_factor: f32,
    pub scale_clipped: u32,
}
impl Raw {
    pub fn open(path: &Path) -> Result<Self> {
        let path_ref = path;
        let (handle, m) = ffi::Handle::open(path)?;
        let text = |bytes: &[std::ffi::c_char]| {
            let bytes: Vec<u8> = bytes.iter().map(|&c| c as u8).collect();
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
            String::from_utf8_lossy(&bytes[..end]).into_owned()
        };
        let metadata = Metadata {
            make: text(&m.make),
            model: text(&m.model),
            width: m.width,
            height: m.height,
            raw_width: m.raw_width,
            raw_height: m.raw_height,
            crop_width: m.crop_width,
            crop_height: m.crop_height,
            crop_left: m.crop_left,
            crop_top: m.crop_top,
            flip: m.flip,
            xtrans: m.xtrans != 0,
            fuji_dynamic_range: m.fuji_dynamic_range,
            highlight_tone_priority: HighlightTonePriority::from_libraw(m.highlight_tone_priority),
            // LibRaw leaves -999 when the maker notes have no shift.
            fuji_exposure_shift: (m.fuji_exposure_shift > -100.).then_some(m.fuji_exposure_shift),
            iso: m.iso,
            shutter: m.shutter,
            aperture: m.aperture,
            focal: m.focal,
            focal_35mm: m.focal_35mm,
            wb: m.wb,
            daylight_wb: m.daylight_wb,
            matrix: std::array::from_fn(|r| std::array::from_fn(|c| m.matrix[r * 3 + c])),
            cam_xyz: std::array::from_fn(|r| std::array::from_fn(|c| m.cam_xyz[r * 3 + c])),
            lens: crate::lens::embedded::read(path_ref),
            lens_model: text(&m.lens).trim().to_string(),
            baseline_exposure: None,
            lens_profiles: Default::default(),
            lateral_ca: Default::default(),
            embedded_profile: None,
        };
        let mut metadata = metadata;
        let dng = crate::dng::read(path_ref);
        let mut crop = fuji_crop(path_ref);
        if let Some(dng) = dng {
            // 0 is the DNG default; the camera table is for other raw formats.
            metadata.baseline_exposure = Some(dng.baseline_exposure.unwrap_or(0.));
            metadata.embedded_profile = dng
                .profile
                .filter(|p| p.ensure_camera(&metadata).is_ok())
                .map(std::sync::Arc::new);
            // LibRaw has no XYZ-to-camera matrix for a DNG from a camera it does not
            // know, so one written with colour matrices but no profile would render
            // without a profile at all. Take the file's D65 matrix then: the same
            // matrix in the same direction, so nothing downstream has to know where
            // it came from. A camera LibRaw knows keeps LibRaw's matrix.
            if metadata.embedded_profile.is_none()
                && metadata.cam_xyz.iter().flatten().all(|v| *v == 0.)
                && let Some(matrix) = dng.color_matrix
                && matrix.iter().flatten().any(|v| *v != 0.)
            {
                metadata.cam_xyz = matrix;
            }
            if dng.lens.is_some() {
                metadata.lens = dng.lens;
            }
            crop = dng.crop.or(crop);
        }
        if let Some([left, top, width, height]) = crop
            && left.checked_add(width).is_some_and(|r| r <= metadata.width)
            && top
                .checked_add(height)
                .is_some_and(|b| b <= metadata.height)
        {
            // Adobe's default crop (DNG DefaultCrop, or the RAF header's crop, which is
            // 2 px larger per side than LibRaw's).
            metadata.crop_left = left;
            metadata.crop_top = top;
            metadata.crop_width = width;
            metadata.crop_height = height;
        }
        metadata.lens_profiles = crate::lens::lcp::library().for_photo(&metadata);
        Ok(Self { handle, metadata })
    }
    /// The embedded JPEG preview, as stored.
    pub fn thumbnail(&mut self) -> Result<Vec<u8>> {
        self.handle.thumbnail()
    }
    pub fn develop(self, fast: bool, cancel: &AtomicBool) -> Result<CameraImage> {
        // Half-size drafts always use LibRaw's fast half-size path.
        if !fast
            && demosaic() == Demosaic::Rawmakase
            && let Some(image) = self.develop_cfa(cancel)?
        {
            return Ok(image);
        }
        self.develop_libraw(fast, cancel)
    }
    /// Unpacked CFA data demosaiced by `crate::demosaic`; `None` when the file is not
    /// single-channel Bayer or X-Trans data.
    fn develop_cfa(&self, cancel: &AtomicBool) -> Result<Option<CameraImage>> {
        let Some(ffi::Cfa {
            width: w,
            height: h,
            pattern,
            mut data,
        }) = self.handle.cfa()?
        else {
            return Ok(None);
        };
        ensure!(!cancel.load(Ordering::Relaxed), "Development cancelled");
        let wb = self.metadata.wb;
        let (width, height) = (w as usize, h as usize);
        let clipped = data
            .par_chunks_mut(width)
            .enumerate()
            .map(|(y, row)| {
                let mut clipped = 0u32;
                for (x, v) in row.iter_mut().enumerate() {
                    clipped += u32::from(*v >= 0.999);
                    *v *= wb[pattern[(y % crate::demosaic::PATTERN) * crate::demosaic::PATTERN
                        + x % crate::demosaic::PATTERN] as usize];
                }
                clipped
            })
            .sum();
        let pixels = crate::demosaic::demosaic(&crate::demosaic::Cfa {
            data: &data,
            width,
            height,
            pattern: &pattern,
        });
        ensure!(!cancel.load(Ordering::Relaxed), "Development cancelled");
        Ok(Some(CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels,
            metadata: self.metadata.clone(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: clipped,
        }))
    }
    fn develop_libraw(self, fast: bool, cancel: &AtomicBool) -> Result<CameraImage> {
        let ffi::Developed {
            width: w,
            height: h,
            scale,
            clipped,
            pixels,
        } = self.handle.develop(fast, cancel)?;
        Ok(CameraImage {
            recovered: Default::default(),
            width: w,
            height: h,
            pixels,
            metadata: self.metadata.clone(),
            fast,
            scale_factor: scale,
            scale_clipped: clipped,
        })
    }
}
/// The camera's recommended crop from the RAF header directory: tags 0x110 (top, left)
/// and 0x111 (height, width), big-endian. Lightroom uses it as the default crop.
fn fuji_crop(path: &Path) -> Option<[u32; 4]> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 100];
    f.read_exact(&mut head).ok()?;
    if !head.starts_with(b"FUJIFILMCCD-RAW") {
        return None;
    }
    let dir = u32::from_be_bytes(head[92..96].try_into().ok()?) as u64;
    let len = u32::from_be_bytes(head[96..100].try_into().ok()?) as usize;
    if !(4..=1 << 20).contains(&len) {
        return None;
    }
    let mut b = vec![0; len];
    f.seek(SeekFrom::Start(dir)).ok()?;
    f.read_exact(&mut b).ok()?;
    let be16 = |o: usize| Some(u16::from_be_bytes(b.get(o..o + 2)?.try_into().ok()?) as u32);
    let count = u32::from_be_bytes(b[..4].try_into().ok()?) as usize;
    let (mut o, mut origin, mut size) = (4, None, None);
    for _ in 0..count.min(256) {
        let (tag, n) = (be16(o)?, be16(o + 2)? as usize);
        if n == 4 && tag == 0x110 {
            origin = Some([be16(o + 6)?, be16(o + 4)?]);
        } else if n == 4 && tag == 0x111 {
            size = Some([be16(o + 6)?, be16(o + 4)?]);
        }
        o += 4 + n;
    }
    let ([left, top], [width, height]) = (origin?, size?);
    (width > 0 && height > 0).then_some([left, top, width, height])
}
pub(crate) fn thumbnail(raw: &mut Raw) -> anyhow::Result<image::RgbImage> {
    use image::{ImageDecoder, metadata::Orientation};
    let bytes = raw.thumbnail()?;
    let mut decoder = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(bytes))?;
    let mut orientation = decoder.orientation()?;
    if orientation == Orientation::NoTransforms {
        orientation = match raw.metadata.flip {
            3 => Orientation::Rotate180,
            5 => Orientation::Rotate270,
            6 => Orientation::Rotate90,
            _ => Orientation::NoTransforms,
        };
    }
    let mut im = image::DynamicImage::from_decoder(decoder)?;
    im.apply_orientation(orientation);
    Ok(im.to_rgb8())
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_highlight_tone_priority_from_libraw() {
        use super::HighlightTonePriority as H;
        assert_eq!(H::from_libraw(0), H::Off);
        assert_eq!(H::from_libraw(1), H::On);
        assert_eq!(H::from_libraw(2), H::Enhanced);
        assert_eq!(H::from_libraw(-1), H::Off);
    }
    #[test]
    fn reads_fujifilm_default_crop() {
        let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        raf.resize(128, 0);
        let mut dir = 2u32.to_be_bytes().to_vec();
        for (tag, a, b) in [(0x110u16, 16u16, 16u16), (0x111, 4000, 6000)] {
            dir.extend(tag.to_be_bytes());
            dir.extend(4u16.to_be_bytes());
            dir.extend(a.to_be_bytes());
            dir.extend(b.to_be_bytes());
        }
        raf[92..96].copy_from_slice(&128u32.to_be_bytes());
        raf[96..100].copy_from_slice(&(dir.len() as u32).to_be_bytes());
        raf.extend(dir);
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), raf).unwrap();
        assert_eq!(super::fuji_crop(f.path()), Some([16, 16, 6000, 4000]));
        std::fs::write(f.path(), b"FUJIFILMCCD-RAW").unwrap();
        assert_eq!(super::fuji_crop(f.path()), None);
    }
    #[test]
    fn dng_without_baseline_exposure_uses_the_dng_default() {
        let chart = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/corpus/charts/synthetic-d65.dng"
        );
        let mut bytes = std::fs::read(chart).unwrap();
        // BaselineExposure, SRATIONAL, count 1: give it an invalid type so it is unread.
        let entry = [0x2a, 0xc6, 10, 0, 1, 0, 0, 0];
        let at = bytes.windows(8).position(|w| w == entry).unwrap();
        bytes[at + 2] = 0;
        // A closed file: Windows' LibRaw cannot open one a NamedTempFile holds open.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-baseline.dng");
        std::fs::write(&path, bytes).unwrap();
        let m = super::Raw::open(&path).unwrap().metadata;
        // A camera without a table row would otherwise take the table's median.
        assert_eq!(m.baseline_exposure, Some(0.));
        assert_eq!(crate::camera_profiles::reference::baseline_exposure(&m), 0.);
    }
    #[test]
    fn corrupt_raw_is_an_error() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), b"not a raw file").unwrap();
        assert!(super::Raw::open(f.path()).is_err());
    }
    #[test]
    fn monitor_srgb_roundtrip() -> anyhow::Result<()> {
        let d = tempfile::tempdir()?;
        let p = d.path().join("srgb.icc");
        std::fs::write(&p, super::srgb_profile()?)?;
        let mut rgb = vec![12, 128, 240, 255, 0, 100];
        let before = rgb.clone();
        super::display_transform(&p, &mut rgb)?;
        for (a, b) in rgb.iter().zip(before) {
            assert!((i16::from(*a) - i16::from(b)).abs() <= 1);
        }
        Ok(())
    }
}
