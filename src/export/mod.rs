//! Atomic JPEG and 16-bit TIFF export with sRGB ICC, the camera's EXIF and the
//! edit as Camera Raw XMP, as Lightroom embeds them.
pub mod assemble;
mod encode;
pub mod exif;
mod extended_xmp;
pub mod job;
mod metadata;
pub mod settings;
use crate::{
    develop::Rendered,
    raw::{self, Metadata},
    storage::is_raw,
};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
pub use settings::{Destination, Existing, ExportSettings, Format, Include};
use std::{fs, path::Path};
use tempfile::NamedTempFile;

/// The Software tag of an export and the creator tool of its XMP.
pub(crate) const SOFTWARE: &str = concat!("RAWmakase ", env!("CARGO_PKG_VERSION"));

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

/// What an export embeds besides pixels.
#[derive(Clone, Debug)]
pub struct Embed {
    /// The camera's EXIF, read from the RAW; LibRaw's capture settings stand in
    /// when it could not be read.
    pub camera: Option<exif::CameraExif>,
    /// Make and model from LibRaw where the camera's EXIF has none.
    pub camera_fallback: bool,
    /// An XMP packet, e.g. the edit as Camera Raw settings.
    pub xmp: Option<String>,
    /// Pixels per inch recorded in the file.
    pub ppi: u32,
}
impl Default for Embed {
    fn default() -> Self {
        Self {
            camera: None,
            camera_fallback: true,
            xmp: None,
            ppi: 240,
        }
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
    export_with(
        path,
        source,
        image,
        m,
        options,
        &Embed::default(),
        overwrite,
    )
}

pub fn export_with(
    path: &Path,
    source: &Path,
    image: &Rendered,
    m: &Metadata,
    options: &ExportOptions,
    embed: &Embed,
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
    let directories = metadata::directories(m, embed, image.width, image.height);
    match path
        .extension()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => encode::jpeg(
            &mut temp,
            image,
            options.quality,
            profile,
            directories,
            embed,
        )?,
        "tif" | "tiff" => encode::tiff(&mut temp, image, profile, &directories, embed)?,
        _ => bail!("Export extension must be .jpg, .jpeg, .tif or .tiff"),
    }
    temp.as_file().sync_all()?;
    if overwrite {
        temp.persist(path).map_err(|e| e.error)?;
    } else {
        temp.persist_noclobber(path).map_err(|e| e.error)?;
    }
    crate::storage::sync_dir(parent)
}

#[cfg(test)]
mod tests;
