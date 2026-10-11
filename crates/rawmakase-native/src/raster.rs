//! Opening a JPEG, PNG or TIFF, the files RAWmakase shows without developing.
use anyhow::{Result, anyhow};
use image::{DynamicImage, RgbImage};
use moxcms::{ColorProfile, DataColorSpace, Layout, TransformExecutor, TransformOptions};
use rayon::prelude::*;
use std::path::Path;

/// The image at `path` decoded, turned upright and converted from its embedded
/// ICC profile to 8-bit sRGB; refused above 150 MP. A file without a profile,
/// or with one that cannot be read or does not describe its pixels (RGB, or
/// grey for a grey image), is taken to be sRGB already.
pub fn decode(path: &Path) -> Result<RgbImage> {
    use image::ImageDecoder;
    let mut decoder = image::ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()?;
    let (w, h) = decoder.dimensions();
    anyhow::ensure!(
        (w as u64) * (h as u64) <= 150_000_000,
        "Image too large to preview"
    );
    let orientation = decoder.orientation()?;
    let transform = decoder
        .icc_profile()
        .ok()
        .flatten()
        .and_then(|icc| ColorProfile::new_from_slice(&icc).ok())
        .and_then(|profile| ToSrgb::new(&profile, decoder.color_type()));
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    match transform {
        Some(transform) => transform.apply(image),
        None => Ok(image.into_rgb8()),
    }
}

/// A conversion from a file's profile to sRGB, at the depth its samples need.
enum ToSrgb {
    Eight(std::sync::Arc<moxcms::Transform8BitExecutor>, Layout),
    Sixteen(std::sync::Arc<moxcms::Transform16BitExecutor>, Layout),
}
impl ToSrgb {
    /// The conversion for pixels of type `color` in `profile`. Samples of
    /// more than 8 bits are converted at 16 and only then rounded to 8.
    fn new(profile: &ColorProfile, color: image::ColorType) -> Option<Self> {
        let layout = match profile.color_space {
            DataColorSpace::Rgb => Layout::Rgb,
            DataColorSpace::Gray if !color.has_color() => Layout::Gray,
            _ => return None,
        };
        let deep = color.bytes_per_pixel() > color.channel_count();
        let srgb = ColorProfile::new_srgb();
        let options = TransformOptions::default();
        Some(if deep {
            let t = profile.create_transform_16bit(layout, &srgb, Layout::Rgb, options);
            Self::Sixteen(t.ok()?, layout)
        } else {
            let t = profile.create_transform_8bit(layout, &srgb, Layout::Rgb, options);
            Self::Eight(t.ok()?, layout)
        })
    }
    fn apply(&self, image: DynamicImage) -> Result<RgbImage> {
        let mut out = RgbImage::new(image.width(), image.height());
        let gray = |layout: &Layout| *layout == Layout::Gray;
        match self {
            Self::Eight(t, l) if gray(l) => rows(&image.into_luma8(), &**t, &mut out, |v| v),
            Self::Eight(t, _) => rows(&image.into_rgb8(), &**t, &mut out, |v| v),
            Self::Sixteen(t, l) if gray(l) => rows(&image.into_luma16(), &**t, &mut out, to_8),
            Self::Sixteen(t, _) => rows(&image.into_rgb16(), &**t, &mut out, to_8),
        }?;
        Ok(out)
    }
}

/// A 16-bit sample rounded to 8 bits.
fn to_8(v: u16) -> u8 {
    ((u32::from(v) + 128) / 257) as u8
}

/// Converts `source` into `out`, which has as many rows, a row at a time, so a
/// 16-bit image never needs a second full-size 16-bit copy.
fn rows<T: Copy + Default + Send + Sync>(
    source: &[T],
    transform: &(dyn TransformExecutor<T> + Send + Sync),
    out: &mut RgbImage,
    quantise: impl Fn(T) -> u8 + Sync,
) -> Result<()> {
    let height = out.height() as usize;
    if height == 0 || out.is_empty() {
        return Ok(());
    }
    let out_row = out.len() / height;
    source
        .par_chunks(source.len() / height)
        .zip(out.par_chunks_mut(out_row))
        .try_for_each(|(from, to)| {
            let mut row = vec![T::default(); to.len()];
            transform.transform(from, &mut row)?;
            for (to, v) in to.iter_mut().zip(row) {
                *to = quantise(v);
            }
            Ok(())
        })
        .map_err(|e: moxcms::CmsError| anyhow!("Could not convert the image to sRGB: {e}"))
}

#[cfg(test)]
mod tests {
    use super::decode;
    use image::{ExtendedColorType, ImageEncoder};
    use std::path::Path;

    fn p3() -> Vec<u8> {
        moxcms::ColorProfile::new_display_p3().encode().unwrap()
    }
    /// A one-row PNG of `pixels`, with `icc` embedded when given.
    fn png(path: &Path, pixels: &[u8], color: ExtendedColorType, icc: Option<Vec<u8>>) {
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = image::codecs::png::PngEncoder::new(file);
        if let Some(icc) = icc {
            encoder.set_icc_profile(icc).unwrap();
        }
        let bytes_per_pixel = match color {
            ExtendedColorType::L8 => 1,
            ExtendedColorType::Rgb8 => 3,
            ExtendedColorType::Rgb16 => 6,
            _ => unreachable!(),
        };
        let width = (pixels.len() / bytes_per_pixel) as u32;
        encoder.write_image(pixels, width, 1, color).unwrap();
    }
    /// Display P3 `rgb` (0–255) as sRGB, worked out apart from any CMS: both
    /// encode with the sRGB curve and share D65, so only the primaries differ.
    fn p3_as_srgb(rgb: [f64; 3]) -> [f64; 3] {
        let linear = |v: f64| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let encode = |v: f64| {
            let v = v.clamp(0., 1.);
            255. * if v <= 0.003_130_8 {
                v * 12.92
            } else {
                1.055 * v.powf(1. / 2.4) - 0.055
            }
        };
        let [r, g, b] = rgb.map(|v| linear(v / 255.));
        [
            encode(1.224_940_2 * r - 0.224_940_2 * g),
            encode(-0.042_056_95 * r + 1.042_056_95 * g),
            encode(-0.019_637_55 * r - 0.078_636_05 * g + 1.098_273_6 * b),
        ]
    }
    fn assert_near(actual: image::Rgb<u8>, expected: [f64; 3], tolerance: f64) {
        for (a, e) in actual.0.iter().zip(expected) {
            assert!(
                (*a as f64 - e).abs() <= tolerance,
                "{actual:?} is not {expected:?}"
            );
        }
    }

    #[test]
    fn a_display_p3_png_is_converted_to_srgb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p3.png");
        let pixels = [200, 100, 50, 40, 180, 90];
        png(&path, &pixels, ExtendedColorType::Rgb8, Some(p3()));
        let image = decode(&path).unwrap();
        assert_near(*image.get_pixel(0, 0), p3_as_srgb([200., 100., 50.]), 1.5);
        assert_near(*image.get_pixel(1, 0), p3_as_srgb([40., 180., 90.]), 1.5);
    }

    #[test]
    fn a_sixteen_bit_png_is_converted_to_srgb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p3-16.png");
        // Red and green half-way between 8-bit codes. These are 194.77, 6.01
        // and 130.93 in sRGB; rounded to 8 bits first, they would come out
        // 194.24, 4.86 and 130.96.
        let samples: [u16; 3] = [45_874, 9_894, 32_896];
        let bytes: Vec<u8> = samples.iter().flat_map(|v| v.to_ne_bytes()).collect();
        png(&path, &bytes, ExtendedColorType::Rgb16, Some(p3()));
        let image = decode(&path).unwrap();
        let expected = p3_as_srgb(samples.map(|v| v as f64 / 257.));
        assert_near(*image.get_pixel(0, 0), expected, 0.5);
    }

    #[test]
    fn a_display_p3_jpeg_is_converted_to_srgb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p3.jpg");
        let source = image::RgbImage::from_pixel(16, 16, image::Rgb([200, 100, 50]));
        let file = std::fs::File::create(&path).unwrap();
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 100);
        encoder.set_icc_profile(p3()).unwrap();
        encoder
            .write_image(source.as_raw(), 16, 16, ExtendedColorType::Rgb8)
            .unwrap();
        let image = decode(&path).unwrap();
        // JPEG's own rounding comes on top of the conversion's.
        assert_near(*image.get_pixel(8, 8), p3_as_srgb([200., 100., 50.]), 3.);
    }

    #[test]
    fn a_grey_png_is_converted_through_its_grey_profile() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("linear-grey.png");
        let linear = moxcms::ColorProfile::new_gray_with_gamma(1.)
            .encode()
            .unwrap();
        png(&path, &[128], ExtendedColorType::L8, Some(linear));
        // Linear 128/255 is 188 in sRGB.
        assert_near(*decode(&path).unwrap().get_pixel(0, 0), [188.; 3], 1.);
    }

    #[test]
    fn a_file_without_a_profile_is_taken_as_srgb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.png");
        let pixels = [200, 100, 50, 0, 255, 7];
        png(&path, &pixels, ExtendedColorType::Rgb8, None);
        assert_eq!(decode(&path).unwrap().as_raw(), &pixels);
    }

    #[test]
    fn an_unreadable_profile_is_taken_as_srgb() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.png");
        let pixels = [200, 100, 50];
        let icc = b"not a profile".to_vec();
        png(&path, &pixels, ExtendedColorType::Rgb8, Some(icc));
        assert_eq!(decode(&path).unwrap().as_raw(), &pixels);
    }
}
