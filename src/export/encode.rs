//! JPEG and 16-bit TIFF encoding with an ICC profile, EXIF directories and XMP.
use super::{
    Embed,
    exif::{CameraExif, Field},
};
use crate::develop::Rendered;
use anyhow::{Result, ensure};
use image::{ImageEncoder, codecs::jpeg::JpegEncoder};
use std::io::{Seek, Write};
use tiff::{
    encoder::{DirectoryEncoder, Rational, SRational, TiffEncoder, TiffKind, colortype::RGB16},
    tags::{ResolutionUnit, Tag},
};

pub(super) fn jpeg(
    out: &mut impl Write,
    image: &Rendered,
    quality: u8,
    profile: Vec<u8>,
    directories: CameraExif,
    embed: &Embed,
) -> Result<()> {
    let mut jpeg = Vec::new();
    let mut e = JpegEncoder::new_with_quality(&mut jpeg, quality);
    e.set_icc_profile(profile)?;
    e.set_exif_metadata(super::metadata::jpeg_exif(directories))?;
    e.write_image(
        &image.rgb8(),
        image.width,
        image.height,
        image::ExtendedColorType::Rgb8,
    )?;
    if let Some(xmp) = &embed.xmp {
        jpeg = insert_xmp(jpeg, xmp)?;
    }
    out.write_all(&jpeg)?;
    Ok(())
}

pub(super) fn tiff(
    out: &mut (impl Write + Seek),
    image: &Rendered,
    profile: Vec<u8>,
    directories: &CameraExif,
    embed: &Embed,
) -> Result<()> {
    let mut e = TiffEncoder::new(out)?;
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
    im.resolution(
        ResolutionUnit::Inch,
        Rational {
            n: embed.ppi.clamp(1, 10_000),
            d: 1,
        },
    );
    if let Some(xmp) = &embed.xmp {
        im.encoder().write_tag(Tag::Unknown(700), xmp.as_bytes())?;
    }
    im.write_data(&image.rgb16())?;
    Ok(())
}

/// XMP goes in its own APP1 segment, after the EXIF and ICC ones; what does
/// not fit one follows as ExtendedXMP.
pub(super) fn insert_xmp(jpeg: Vec<u8>, xmp: &str) -> Result<Vec<u8>> {
    use super::extended_xmp::{STANDARD_HEADER, segments, split};
    ensure!(jpeg.starts_with(&[0xff, 0xd8]), "Not a JPEG");
    let split = split(xmp)?;
    let mut payloads = vec![[STANDARD_HEADER, split.standard.as_bytes()].concat()];
    if let Some((guid, extended)) = &split.extended {
        payloads.extend(segments(guid, extended));
    }
    for payload in &payloads {
        ensure!(
            2 + payload.len() <= 65_535,
            "XMP is too large for a JPEG segment"
        );
    }
    let mut at = 2;
    while at + 4 <= jpeg.len() && jpeg[at] == 0xff && (0xe0..=0xef).contains(&jpeg[at + 1]) {
        at += 2 + u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
    }
    let extra: usize = payloads.iter().map(|p| p.len() + 4).sum();
    let mut out = Vec::with_capacity(jpeg.len() + extra);
    out.extend_from_slice(&jpeg[..at]);
    for payload in &payloads {
        out.extend([0xff, 0xe1]);
        out.extend(((2 + payload.len()) as u16).to_be_bytes());
        out.extend_from_slice(payload);
    }
    out.extend_from_slice(&jpeg[at..]);
    Ok(out)
}

/// Writes one EXIF field into a TIFF directory with its own type. BYTE, SBYTE
/// and UNDEFINED values are written as bytes.
fn write_field<W: Write + Seek, K: TiffKind>(
    dir: &mut DirectoryEncoder<'_, W, K>,
    f: &Field,
) -> Result<()> {
    let tag = Tag::Unknown(f.tag);
    let halves = || f.bytes.as_chunks::<2>().0.iter().copied();
    let words = || f.bytes.as_chunks::<4>().0.iter().copied();
    // Numerator and denominator of each rational.
    let pairs = || {
        f.bytes
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| c.as_chunks::<4>().0)
    };
    match f.kind {
        2 => dir.write_tag(tag, f.text().unwrap_or_default().as_str())?,
        3 => dir.write_tag(
            tag,
            halves()
                .map(u16::from_le_bytes)
                .collect::<Vec<_>>()
                .as_slice(),
        )?,
        8 => dir.write_tag(
            tag,
            halves()
                .map(i16::from_le_bytes)
                .collect::<Vec<_>>()
                .as_slice(),
        )?,
        4 => dir.write_tag(
            tag,
            words()
                .map(u32::from_le_bytes)
                .collect::<Vec<_>>()
                .as_slice(),
        )?,
        9 => dir.write_tag(
            tag,
            words()
                .map(i32::from_le_bytes)
                .collect::<Vec<_>>()
                .as_slice(),
        )?,
        11 => dir.write_tag(
            tag,
            words()
                .map(f32::from_le_bytes)
                .collect::<Vec<_>>()
                .as_slice(),
        )?,
        5 => {
            let v: Vec<Rational> = pairs()
                .map(|p| Rational {
                    n: u32::from_le_bytes(p[0]),
                    d: u32::from_le_bytes(p[1]),
                })
                .collect();
            write_array(dir, tag, v)?
        }
        10 => {
            let v: Vec<SRational> = pairs()
                .map(|p| SRational {
                    n: i32::from_le_bytes(p[0]),
                    d: i32::from_le_bytes(p[1]),
                })
                .collect();
            write_array(dir, tag, v)?
        }
        _ => dir.write_tag(tag, f.bytes.as_slice())?,
    }
    Ok(())
}

/// The encoder takes rationals only as fixed-size arrays; EXIF uses up to four
/// (LensInfo), GPS three (latitude, longitude, time).
fn write_array<W: Write + Seek, K: TiffKind, T>(
    dir: &mut DirectoryEncoder<'_, W, K>,
    tag: Tag,
    v: Vec<T>,
) -> Result<()>
where
    [T]: tiff::encoder::TiffValue,
{
    fn take<T, const N: usize>(v: Vec<T>) -> Option<[T; N]> {
        v.try_into().ok()
    }
    match v.len() {
        1 => dir.write_tag(tag, take::<T, 1>(v).unwrap())?,
        2 => dir.write_tag(tag, take::<T, 2>(v).unwrap())?,
        3 => dir.write_tag(tag, take::<T, 3>(v).unwrap())?,
        4 => dir.write_tag(tag, take::<T, 4>(v).unwrap())?,
        _ => {}
    }
    Ok(())
}
