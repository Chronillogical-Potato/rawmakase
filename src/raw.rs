use anyhow::{Result, bail, ensure};
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
fn path_string(p: &Path) -> Result<CString> {
    use std::os::unix::ffi::OsStrExt;
    Ok(CString::new(p.as_os_str().as_bytes())?)
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
        };
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
