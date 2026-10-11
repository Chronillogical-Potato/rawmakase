//! Opening a JPEG, PNG or TIFF, the files RAWmakase shows without developing.
use anyhow::Result;
use std::path::Path;

/// The image at `path` decoded and turned upright; refused above 150 MP.
pub fn decode(path: &Path) -> Result<image::RgbImage> {
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
    let mut image = image::DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok(image.to_rgb8())
}
