use crate::catalog::Photo;
use anyhow::Result;
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};
pub(super) fn available_paths(photos: &[Photo]) -> HashSet<PathBuf> {
    use rayon::prelude::*;
    let mut directories: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
    for photo in photos {
        if let Some(parent) = photo.path.parent() {
            directories
                .entry(parent.into())
                .or_default()
                .insert(photo.path.clone());
        }
    }
    directories
        .into_par_iter()
        .flat_map_iter(|(directory, wanted)| {
            // GVFS directory listings carry file types; avoid one remote stat per photo.
            std::fs::read_dir(directory)
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .filter_map(move |entry| {
                    let path = entry.path();
                    if wanted.contains(&path)
                        && entry
                            .file_type()
                            .is_ok_and(|t| t.is_file() || (t.is_symlink() && path.is_file()))
                    {
                        Some(path)
                    } else {
                        None
                    }
                })
        })
        .collect()
}

pub(super) fn thumbnail(path: &std::path::Path) -> Result<image::RgbImage> {
    let image = if crate::storage::is_raw(path) {
        let mut raw = crate::raw::Raw::open(path)?;
        crate::raw::thumbnail(&mut raw)?
    } else {
        raster(path)?
    };
    Ok(downscale(&image, 640))
}
/// A JPEG, TIFF or PNG decoded and turned upright; refused above 150 MP.
pub(super) fn raster(path: &std::path::Path) -> Result<image::RgbImage> {
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
/// `image` within `edge`×`edge`; never enlarged.
pub(super) fn downscale(image: &image::RgbImage, edge: u32) -> image::RgbImage {
    let (width, height) = fit(image.width(), image.height(), edge);
    if (width, height) == image.dimensions() {
        return image.clone();
    }
    image::imageops::thumbnail(image, width, height)
}
/// Largest size within `edge`×`edge` that keeps the source aspect ratio.
pub(super) fn fit(width: u32, height: u32, edge: u32) -> (u32, u32) {
    let scale = (edge as f32 / width.max(height).max(1) as f32).min(1.);
    (
        ((width as f32 * scale).round() as u32).max(1),
        ((height as f32 * scale).round() as u32).max(1),
    )
}
