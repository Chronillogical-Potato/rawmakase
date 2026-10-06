//! Atomic JPEG and 16-bit TIFF export with sRGB ICC, the camera's EXIF and the
//! edit as Camera Raw XMP, as Lightroom embeds them.
pub mod assemble;
pub mod batch;
mod encode;
pub(crate) mod exif;
mod extended_xmp;
pub mod job;
mod metadata;
pub mod queue;
pub mod settings;
pub use crate::storage::Replace;
use crate::{
    develop::Rendered,
    raw::{self, Metadata},
    storage::is_raw,
};
use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
pub use settings::{Destination, Existing, ExportSettings, Format, Include, NameContext, Naming};
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
    pub camera: Option<crate::exif::CameraExif>,
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
    replace: Replace,
) -> Result<()> {
    export_with(path, source, image, m, options, &Embed::default(), replace)
}

pub fn export_with(
    path: &Path,
    source: &Path,
    image: &Rendered,
    m: &Metadata,
    options: &ExportOptions,
    embed: &Embed,
    replace: Replace,
) -> Result<()> {
    ensure!(!is_raw(path), "An export cannot overwrite a RAW file");
    options.validate()?;
    if path.exists() {
        ensure!(
            fs::canonicalize(path)? != fs::canonicalize(source)?,
            "Cannot overwrite source"
        );
        ensure!(replace == Replace::Overwrite, "Destination already exists");
    }
    let staged = stage(path, image, m, options, embed)?;
    crate::storage::persist(staged, path, replace)
}

/// The export of `image` for `path`, encoded into a synced temporary file in
/// `path`'s folder: [`crate::storage::persist`] puts it in place, and dropping it
/// leaves nothing behind.
pub fn stage(
    path: &Path,
    image: &Rendered,
    m: &Metadata,
    options: &ExportOptions,
    embed: &Embed,
) -> Result<NamedTempFile> {
    ensure!(!is_raw(path), "An export cannot overwrite a RAW file");
    options.validate()?;
    let parent = crate::storage::parent_dir(path);
    let mut temp = NamedTempFile::new_in(parent)?;
    let profile = raw::srgb_profile()?;
    let directories = metadata::directories(m, embed, image.width, image.height);
    match Format::from_path(path) {
        Some(Format::Jpeg) => encode::jpeg(
            &mut temp,
            image,
            options.quality,
            profile,
            directories,
            embed,
        )?,
        Some(Format::Tiff) => encode::tiff(&mut temp, image, profile, &directories, embed)?,
        None => bail!("Export extension must be .jpg, .jpeg, .tif or .tiff"),
    }
    temp.as_file().sync_all()?;
    Ok(temp)
}

#[cfg(test)]
mod tests;
