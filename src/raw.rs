use anyhow::{Result, bail, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
#[repr(C)]
struct NativeMetadata {
    width: u32,
    height: u32,
    raw_width: u32,
    raw_height: u32,
    crop_width: u32,
    crop_height: u32,
    crop_left: u32,
    crop_top: u32,
    flip: i32,
    xtrans: i32,
    fuji_dynamic_range: u32,
    iso: f32,
    shutter: f32,
    aperture: f32,
    focal: f32,
    wb: [f32; 3],
    daylight_wb: [f32; 3],
    matrix: [f32; 9],
    make: [c_char; 64],
    model: [c_char; 64],
    cam_xyz: [f32; 9],
    lens: [c_char; 128],
}
unsafe extern "C" {
    fn ora_version() -> *const c_char;
    fn ora_open(path: *const c_char, m: *mut NativeMetadata, err: *mut c_char) -> *mut c_void;
    fn ora_close(h: *mut c_void);
    fn ora_develop(
        h: *mut c_void,
        fast: c_int,
        cancel: extern "C" fn(*mut c_void) -> c_int,
        ctx: *mut c_void,
        w: *mut u32,
        height: *mut u32,
        gain: *mut f32,
        scale: *mut f32,
        clipped: *mut u32,
        err: *mut c_char,
    ) -> c_int;
    fn ora_copy(h: *mut c_void, out: *mut f32);
    fn ora_cfa_open(
        h: *mut c_void,
        w: *mut u32,
        height: *mut u32,
        pattern: *mut u8,
        err: *mut c_char,
    ) -> c_int;
    fn ora_cfa_copy(h: *mut c_void, out: *mut f32);
    fn ora_thumbnail(h: *mut c_void, data: *mut *mut u8, size: *mut u32, err: *mut c_char)
    -> c_int;
    fn ora_srgb_profile(data: *mut u8, size: u32) -> u32;
    fn ora_display(path: *const c_char, data: *mut u8, count: u32) -> c_int;
}
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
    pub iso: f32,
    pub shutter: f32,
    pub aperture: f32,
    pub focal: f32,
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
    /// DNG BaselineExposure, when the file is a DNG that records one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_exposure: Option<f32>,
    /// Correction from an imported Adobe lens profile matching this lens; rebuilt on open.
    #[serde(skip)]
    pub profile_lens: Option<crate::lens::LensCorrection>,
    /// Camera profile embedded in a DNG; rebuilt from the file on open.
    #[serde(skip)]
    pub embedded_profile: Option<std::sync::Arc<crate::camera_profiles::CameraProfile>>,
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
    handle: *mut c_void,
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
impl Drop for Raw {
    fn drop(&mut self) {
        unsafe { ora_close(self.handle) }
    }
}
#[cfg(unix)]
fn path_string(p: &Path) -> Result<CString> {
    use std::os::unix::ffi::OsStrExt;
    Ok(CString::new(p.as_os_str().as_bytes())?)
}
/// UTF-8, which the native side widens for LibRaw's wide-character open.
#[cfg(windows)]
fn path_string(p: &Path) -> Result<CString> {
    let utf8 = p
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Path is not valid Unicode: {}", p.display()))?;
    Ok(CString::new(utf8)?)
}
fn error(buf: &[c_char]) -> String {
    unsafe { CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned() }
}
extern "C" fn cancelled(ctx: *mut c_void) -> c_int {
    unsafe { (&*(ctx as *const AtomicBool)).load(Ordering::Relaxed) as c_int }
}
pub fn version() -> String {
    unsafe { CStr::from_ptr(ora_version()).to_string_lossy().into_owned() }
}
impl Raw {
    pub fn open(path: &Path) -> Result<Self> {
        let path_ref = path;
        let path = path_string(path)?;
        let mut err = [0; 512];
        let mut m: NativeMetadata = unsafe { std::mem::zeroed() };
        let handle = unsafe { ora_open(path.as_ptr(), &mut m, err.as_mut_ptr()) };
        ensure!(!handle.is_null(), "{}", error(&err));
        let metadata = Metadata {
            make: error(&m.make),
            model: error(&m.model),
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
            iso: m.iso,
            shutter: m.shutter,
            aperture: m.aperture,
            focal: m.focal,
            wb: m.wb,
            daylight_wb: m.daylight_wb,
            matrix: std::array::from_fn(|r| std::array::from_fn(|c| m.matrix[r * 3 + c])),
            cam_xyz: std::array::from_fn(|r| std::array::from_fn(|c| m.cam_xyz[r * 3 + c])),
            lens: crate::lens::embedded::read(path_ref),
            lens_model: error(&m.lens).trim().to_string(),
            baseline_exposure: None,
            profile_lens: None,
            embedded_profile: None,
        };
        let mut metadata = metadata;
        let dng = crate::dng::read(path_ref);
        let mut crop = fuji_crop(path_ref);
        if let Some(dng) = dng {
            metadata.baseline_exposure = dng.baseline_exposure;
            metadata.embedded_profile = dng
                .profile
                .filter(|p| p.ensure_camera(&metadata).is_ok())
                .map(std::sync::Arc::new);
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
        metadata.profile_lens = crate::lens::lcp::installed(&metadata);
        Ok(Self { handle, metadata })
    }
    pub fn thumbnail(&mut self) -> Result<Vec<u8>> {
        let mut data = std::ptr::null_mut();
        let mut size = 0;
        let mut err = [0; 512];
        let rc = unsafe { ora_thumbnail(self.handle, &mut data, &mut size, err.as_mut_ptr()) };
        ensure!(rc == 0, "{}", error(&err));
        ensure!(
            !data.is_null() && size > 0 && size < 100_000_000,
            "Invalid preview length"
        );
        Ok(unsafe { std::slice::from_raw_parts(data, size as usize).to_vec() })
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
        let (mut w, mut h) = (0u32, 0u32);
        let mut pattern = [0u8; crate::demosaic::PATTERN * crate::demosaic::PATTERN];
        let mut err = [0; 512];
        let rc = unsafe {
            ora_cfa_open(
                self.handle,
                &mut w,
                &mut h,
                pattern.as_mut_ptr(),
                err.as_mut_ptr(),
            )
        };
        if rc > 0 {
            return Ok(None);
        }
        ensure!(rc == 0, "{}", error(&err));
        ensure!(
            w > 0 && h > 0 && u64::from(w) * u64::from(h) <= 150_000_000,
            "Invalid RAW dimensions"
        );
        ensure!(pattern.iter().all(|c| *c < 3), "Unsupported colour filter");
        let mut data = vec![0f32; w as usize * h as usize];
        unsafe { ora_cfa_copy(self.handle, data.as_mut_ptr()) };
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
        let (mut w, mut h, mut gain, mut scale, mut clipped) = (0, 0, 0., 0., 0);
        let mut err = [0; 512];
        let rc = unsafe {
            ora_develop(
                self.handle,
                fast as c_int,
                cancelled,
                cancel as *const _ as *mut c_void,
                &mut w,
                &mut h,
                &mut gain,
                &mut scale,
                &mut clipped,
                err.as_mut_ptr(),
            )
        };
        ensure!(rc == 0, "{}", error(&err));
        ensure!(
            w > 0 && h > 0 && u64::from(w) * u64::from(h) <= 150_000_000,
            "Invalid RAW dimensions"
        );
        let mut pixels = vec![[0f32; 3]; w as usize * h as usize];
        unsafe { ora_copy(self.handle, pixels.as_mut_ptr().cast()) };
        ensure!(
            gain.is_finite() && pixels.iter().flatten().all(|v| v.is_finite()),
            "Non-finite RAW pixels"
        );
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
pub fn srgb_profile() -> Result<Vec<u8>> {
    let size = unsafe { ora_srgb_profile(std::ptr::null_mut(), 0) };
    ensure!(size > 0, "Cannot create sRGB profile");
    let mut data = vec![0; size as usize];
    ensure!(
        unsafe { ora_srgb_profile(data.as_mut_ptr(), size) } == size,
        "Cannot serialize sRGB profile"
    );
    Ok(data)
}
pub fn display_transform(path: &Path, data: &mut [u8]) -> Result<()> {
    ensure!(data.len().is_multiple_of(3), "Invalid RGB buffer");
    let p = path_string(path)?;
    if unsafe { ora_display(p.as_ptr(), data.as_mut_ptr(), (data.len() / 3).try_into()?) } != 0 {
        bail!("Cannot use monitor ICC profile {}", path.display());
    }
    Ok(())
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
    unsafe extern "C" {
        fn ora_scale_probe(wb: f32, error: *mut f32) -> i32;
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
    fn corrupt_raw_is_an_error() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), b"not a raw file").unwrap();
        assert!(super::Raw::open(f.path()).is_err());
    }
    #[test]
    fn integer_boundary_retains_near_saturation_ramps() {
        for wb in [1., 2.5, 8., 16.] {
            let mut error = 0.;
            let clipped = unsafe { ora_scale_probe(wb, &mut error) };
            assert_eq!(clipped, 0);
            assert!(error < wb / 59000., "WB {wb}: error {error}");
        }
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
