//! Atomic JPEG and 16-bit TIFF export with sRGB ICC, the camera's EXIF and the
//! edit as Camera Raw XMP, as Lightroom embeds them.
pub mod exif;
mod metadata;
use crate::{
    develop::Rendered,
    raw::{self, Metadata},
    storage::is_raw,
};
use anyhow::{Result, bail, ensure};
use image::{ImageEncoder, codecs::jpeg::JpegEncoder};
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

/// What an export embeds besides pixels.
#[derive(Clone, Debug)]
pub struct Embed {
    /// The camera's EXIF, read from the RAW; LibRaw's capture settings stand in
    /// when it could not be read.
    pub camera: Option<exif::CameraExif>,
    /// Include capture metadata at all (camera, exposure, lens, dates).
    pub capture: bool,
    /// Keep the camera's GPS position.
    pub location: bool,
    /// An XMP packet, e.g. the edit as Camera Raw settings.
    pub xmp: Option<String>,
    /// Pixels per inch recorded in the file.
    pub ppi: u32,
}
impl Default for Embed {
    fn default() -> Self {
        Self {
            camera: None,
            capture: true,
            location: true,
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

/// XMP goes in its own APP1 segment, after the EXIF and ICC ones.
fn insert_xmp(jpeg: Vec<u8>, xmp: &str) -> Result<Vec<u8>> {
    const HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
    let length = 2 + HEADER.len() + xmp.len();
    ensure!(length <= 65_535, "XMP is too large for a JPEG segment");
    ensure!(jpeg.starts_with(&[0xff, 0xd8]), "Not a JPEG");
    let mut at = 2;
    while at + 4 <= jpeg.len() && jpeg[at] == 0xff && (0xe0..=0xef).contains(&jpeg[at + 1]) {
        at += 2 + u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
    }
    let mut out = Vec::with_capacity(jpeg.len() + length + 2);
    out.extend_from_slice(&jpeg[..at]);
    out.extend([0xff, 0xe1]);
    out.extend((length as u16).to_be_bytes());
    out.extend_from_slice(HEADER);
    out.extend_from_slice(xmp.as_bytes());
    out.extend_from_slice(&jpeg[at..]);
    Ok(out)
}

/// Writes one EXIF field into a TIFF directory with its own type.
fn write_field<W: std::io::Write + std::io::Seek, K: tiff::encoder::TiffKind>(
    dir: &mut tiff::encoder::DirectoryEncoder<'_, W, K>,
    f: &exif::Field,
) -> Result<()> {
    use tiff::{
        encoder::{Rational, SRational},
        tags::Tag,
    };
    let tag = Tag::Unknown(f.tag);
    let words = |n: usize| -> Vec<[u8; 4]> {
        f.bytes
            .chunks_exact(4)
            .take(n)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect()
    };
    match f.kind {
        2 => dir.write_tag(tag, f.text().unwrap_or_default().as_str())?,
        3 => {
            let v: Vec<u16> = f
                .bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            dir.write_tag(tag, v.as_slice())?
        }
        8 => {
            let v: Vec<i16> = f
                .bytes
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
            dir.write_tag(tag, v.as_slice())?
        }
        4 => {
            let v: Vec<u32> = words(usize::MAX)
                .into_iter()
                .map(u32::from_le_bytes)
                .collect();
            dir.write_tag(tag, v.as_slice())?
        }
        9 => {
            let v: Vec<i32> = words(usize::MAX)
                .into_iter()
                .map(i32::from_le_bytes)
                .collect();
            dir.write_tag(tag, v.as_slice())?
        }
        5 | 10 => {
            let w = words(usize::MAX);
            let n: Vec<u32> = w
                .iter()
                .step_by(2)
                .map(|b| u32::from_le_bytes(*b))
                .collect();
            let d: Vec<u32> = w
                .iter()
                .skip(1)
                .step_by(2)
                .map(|b| u32::from_le_bytes(*b))
                .collect();
            // The encoder takes rationals only as fixed-size arrays; EXIF uses
            // up to four (LensInfo), GPS three (latitude, longitude, time).
            macro_rules! put {
                ($t:ident, $cast:ty) => {{
                    let v: Vec<$t> = n
                        .iter()
                        .zip(&d)
                        .map(|(n, d)| $t {
                            n: *n as $cast,
                            d: *d as $cast,
                        })
                        .collect();
                    match v.len() {
                        1 => dir.write_tag(tag, <[$t; 1]>::try_from(v).ok().unwrap())?,
                        2 => dir.write_tag(tag, <[$t; 2]>::try_from(v).ok().unwrap())?,
                        3 => dir.write_tag(tag, <[$t; 3]>::try_from(v).ok().unwrap())?,
                        4 => dir.write_tag(tag, <[$t; 4]>::try_from(v).ok().unwrap())?,
                        _ => {}
                    }
                }};
            }
            if f.kind == 5 {
                put!(Rational, u32)
            } else {
                put!(SRational, i32)
            }
        }
        11 => {
            let v: Vec<f32> = words(usize::MAX)
                .into_iter()
                .map(f32::from_le_bytes)
                .collect();
            dir.write_tag(tag, v.as_slice())?
        }
        // BYTE, SBYTE and UNDEFINED are written as bytes.
        _ => dir.write_tag(tag, f.bytes.as_slice())?,
    }
    Ok(())
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
        "jpg" | "jpeg" => {
            let mut jpeg = Vec::new();
            let mut e = JpegEncoder::new_with_quality(&mut jpeg, options.quality);
            e.set_icc_profile(profile)?;
            e.set_exif_metadata(metadata::jpeg_exif(directories))?;
            e.write_image(
                &image.rgb8(),
                image.width,
                image.height,
                image::ExtendedColorType::Rgb8,
            )?;
            if let Some(xmp) = &embed.xmp {
                jpeg = insert_xmp(jpeg, xmp)?;
            }
            std::io::Write::write_all(&mut temp, &jpeg)?;
        }
        "tif" | "tiff" => {
            use tiff::{
                encoder::{TiffEncoder, colortype::RGB16},
                tags::Tag,
            };
            let mut e = TiffEncoder::new(&mut temp)?;
            let mut exif = e.extra_directory()?;
            for f in &directories.exif {
                write_field(&mut exif, f)?;
            }
            let exif_offset = exif.finish_with_offsets()?.offset;
            let gps_offset = if directories.gps.is_empty() {
                None
            } else {
                let mut gps = e.extra_directory()?;
                for f in &directories.gps {
                    write_field(&mut gps, f)?;
                }
                Some(gps.finish_with_offsets()?.offset)
            };
            let mut im = e.new_image::<RGB16>(image.width, image.height)?;
            im.encoder().write_tag(Tag::Unknown(0x8769), exif_offset)?;
            if let Some(offset) = gps_offset {
                im.encoder().write_tag(Tag::Unknown(0x8825), offset)?;
            }
            im.encoder()
                .write_tag(Tag::Unknown(34675), profile.as_slice())?;
            // The encoder writes its own resolution tags.
            for f in directories
                .main
                .iter()
                .filter(|f| ![0x011a, 0x011b, 0x0128].contains(&f.tag))
            {
                write_field(im.encoder(), f)?;
            }
            let ppi = embed.ppi.clamp(1, 10_000);
            im.resolution(
                tiff::tags::ResolutionUnit::Inch,
                tiff::encoder::Rational { n: ppi, d: 1 },
            );
            if let Some(xmp) = &embed.xmp {
                im.encoder().write_tag(Tag::Unknown(700), xmp.as_bytes())?;
            }
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
