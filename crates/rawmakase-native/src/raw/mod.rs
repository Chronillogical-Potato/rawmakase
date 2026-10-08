//! RAW files: opening them through LibRaw, reading their metadata (with the
//! DNG, RAF and lens details read on top), development into linear camera-space
//! pixels, the embedded preview, and the monitor colour transform. The values
//! they produce are [`crate::camera_data`]'s.
//! The native boundary itself is in [`ffi`].
use anyhow::{Result, ensure};
use rayon::prelude::*;
use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
mod ffi;
use crate::camera_data::{CameraImage, Decode, Demosaic, HighlightTonePriority, Metadata};
pub use ffi::{display_transform, srgb_profile, version};
/// Runs `work` on a thread of its own at background priority: a lower
/// scheduling priority, and LibRaw decodes on two OpenMP threads. Every worker
/// that reads or decodes photos behind the user's back starts here or in
/// [`background_pool`], so none competes with Develop for the CPU.
pub fn spawn_background(work: impl FnOnce() + Send + 'static) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        ffi::background_thread();
        work();
    })
}
/// A pool of `threads` named `name-0`, `name-1`… at background priority, as
/// [`spawn_background`].
pub fn background_pool(
    threads: usize,
    name: &'static str,
) -> Result<rayon::ThreadPool, rayon::ThreadPoolBuildError> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(move |i| format!("{name}-{i}"))
        .start_handler(|_| ffi::background_thread())
        .build()
}
pub struct Raw {
    handle: ffi::Handle,
    pub metadata: Metadata,
}
impl Raw {
    /// LibRaw's facts about the file at `path`, and the crop a RAF header
    /// recommends. What the file says beyond them (lens tables, a DNG's profile
    /// and hints) and the imported lens profiles are `crate::photo::open`'s.
    pub fn open_file(path: &Path) -> Result<Self> {
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
            sony_daylight_wb: m
                .sony_daylight_wb
                .iter()
                .all(|v| v.is_finite() && *v > 0.)
                .then_some(m.sony_daylight_wb),
            matrix: std::array::from_fn(|r| std::array::from_fn(|c| m.matrix[r * 3 + c])),
            cam_xyz: std::array::from_fn(|r| std::array::from_fn(|c| m.cam_xyz[r * 3 + c])),
            lens: None,
            lens_model: text(&m.lens).trim().to_string(),
            baseline_exposure: None,
            dng_neutral_calibration: None,
            dng_matrix_profile_signature: None,
            lens_profiles: Default::default(),
            lateral_ca: Default::default(),
            embedded_dcp: None,
        };
        let mut metadata = metadata;
        if let Some(crop) = fuji_crop(path_ref) {
            metadata.apply_default_crop(crop);
        }
        Ok(Self { handle, metadata })
    }
    /// The embedded JPEG preview, as stored.
    pub fn thumbnail(&mut self) -> Result<Vec<u8>> {
        self.handle.thumbnail()
    }
    pub fn develop(self, decode: Decode, cancel: &AtomicBool) -> Result<CameraImage> {
        match decode {
            Decode::Half => self.develop_libraw(true, cancel),
            Decode::Full(Demosaic::Libraw) => self.develop_libraw(false, cancel),
            // LibRaw's demosaic when the file is not Bayer or X-Trans data.
            Decode::Full(Demosaic::Rawmakase) => match self.develop_cfa(cancel)? {
                Some(image) => Ok(image),
                None => self.develop_libraw(false, cancel),
            },
        }
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
            metadata: self.metadata,
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
pub fn thumbnail(raw: &mut Raw) -> anyhow::Result<image::RgbImage> {
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
    #[cfg(target_os = "linux")]
    #[test]
    fn background_threads_are_ten_steps_nicer_than_their_parent() {
        // Field 19 of /proc/thread-self/stat, after the parenthesised name.
        fn nice() -> i32 {
            let stat = std::fs::read_to_string("/proc/thread-self/stat").unwrap();
            let fields = stat.rsplit(')').next().unwrap();
            fields.split_whitespace().nth(16).unwrap().parse().unwrap()
        }
        let parent = nice();
        let (tx, rx) = std::sync::mpsc::channel();
        super::spawn_background(move || tx.send(nice()).unwrap());
        assert_eq!(rx.recv().unwrap(), (parent + 10).min(19));
        assert_eq!(nice(), parent);
    }
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
    fn corrupt_raw_is_an_error() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), b"not a raw file").unwrap();
        assert!(super::Raw::open_file(f.path()).is_err());
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
