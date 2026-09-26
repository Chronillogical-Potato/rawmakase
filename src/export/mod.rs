//! Atomic JPEG and 16-bit TIFF export with sRGB ICC and selected capture metadata.
mod metadata;
use crate::{
    develop::Rendered,
    raw::{self, Metadata},
    storage::is_raw,
};
use anyhow::{Result, bail, ensure};
use image::{ImageEncoder, codecs::jpeg::JpegEncoder};
use metadata::{description, exif};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    path::Path,
};
use tempfile::NamedTempFile;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExportOptions {
    pub quality: u8,
    pub max_edge: u32,
}
impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            quality: 92,
            max_edge: 0,
        }
    }
}
impl ExportOptions {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=100).contains(&self.quality),
            "JPEG quality must be 1–100"
        );
        ensure!(
            self.max_edge <= 30_000,
            "Export edge must not exceed 30000 pixels"
        );
        Ok(())
    }
}

pub fn export(
    path: &Path,
    source: &Path,
    image: &Rendered,
    m: &Metadata,
    options: &ExportOptions,
    overwrite: bool,
) -> Result<()> {
    ensure!(!is_raw(path), "An export cannot overwrite a RAW file");
    options.validate()?;
    if path.exists() {
        ensure!(
            fs::canonicalize(path)? != fs::canonicalize(source)?,
            "Cannot overwrite source"
        );
        ensure!(overwrite, "Destination already exists");
    }
    let parent = crate::storage::parent_dir(path);
    let mut temp = NamedTempFile::new_in(parent)?;
    let profile = raw::srgb_profile()?;
    match path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => {
            let mut e = JpegEncoder::new_with_quality(&mut temp, options.quality);
            e.set_icc_profile(profile)?;
            e.set_exif_metadata(exif(m))?;
            e.write_image(
                &image.rgb8(),
                image.width,
                image.height,
                image::ExtendedColorType::Rgb8,
            )?;
        }
        "tif" | "tiff" => {
            use tiff::{
                encoder::{TiffEncoder, colortype::RGB16},
                tags::Tag,
            };
            let mut e = TiffEncoder::new(&mut temp)?;
            let mut exif = e.extra_directory()?;
            let rational = |v: f32| tiff::encoder::Rational {
                n: (v.max(0.) * 1_000_000.).round() as u32,
                d: 1_000_000,
            };
            exif.write_tag(Tag::Unknown(0x829a), rational(m.shutter))?;
            exif.write_tag(Tag::Unknown(0x829d), rational(m.aperture))?;
            exif.write_tag(Tag::Unknown(0x8827), m.iso.min(65535.) as u16)?;
            exif.write_tag(Tag::Unknown(0x920a), rational(m.focal))?;
            exif.write_tag(Tag::Unknown(0xa001), 1u16)?;
            let exif_offset = exif.finish_with_offsets()?.offset;
            let mut im = e.new_image::<RGB16>(image.width, image.height)?;
            im.encoder().write_tag(Tag::Unknown(0x8769), exif_offset)?;
            im.encoder()
                .write_tag(Tag::Unknown(34675), profile.as_slice())?;
            im.encoder().write_tag(Tag::Make, m.make.as_str())?;
            im.encoder().write_tag(Tag::Model, m.model.as_str())?;
            im.encoder().write_tag(Tag::Software, "RAWmakase 0.1")?;
            im.encoder()
                .write_tag(Tag::ImageDescription, description(m).as_str())?;
            im.encoder().write_tag(Tag::Orientation, 1u16)?;
            im.write_data(&image.rgb16())?;
        }
        _ => bail!("Export extension must be .jpg, .jpeg, .tif or .tiff"),
    }
    temp.as_file().sync_all()?;
    if overwrite {
        temp.persist(path).map_err(|e| e.error)?;
    } else {
        temp.persist_noclobber(path).map_err(|e| e.error)?;
    }
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests;
