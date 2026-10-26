//! The EXIF directories an export writes: the camera's own when it is included,
//! with the export's size, orientation, resolution and software.
use super::{Embed, exif::tiff_block};
use crate::build_info::SOFTWARE;
use crate::{
    camera_data::Metadata,
    exif::{
        CameraExif, Field,
        tag::{
            self, COLOR_SPACE, LENS_MODEL, MAKE, ORIENTATION, PIXEL_X_DIMENSION, PIXEL_Y_DIMENSION,
            RESOLUTION_UNIT, X_RESOLUTION, Y_RESOLUTION, YCBCR_POSITIONING,
        },
    },
};

pub(super) fn directories(m: &Metadata, embed: &Embed, width: u32, height: u32) -> CameraExif {
    let mut d = embed
        .camera
        .clone()
        .unwrap_or_else(|| CameraExif::from_libraw(m));
    if embed.camera_fallback && !d.main.iter().any(|f| f.tag == MAKE) {
        d.main.extend(CameraExif::from_libraw(m).main);
    }
    // A lens the camera did not name (Pentax records only its LensType) is named
    // from the lens profile that identified it; the camera's own name is kept.
    if embed.camera_fallback
        && !m.lens_model.is_empty()
        && !d.exif.iter().any(|f| f.tag == LENS_MODEL)
    {
        d.exif.push(Field::ascii(LENS_MODEL, &m.lens_model));
    }
    let ppi = embed.ppi.clamp(1, 10_000);
    d.main.retain(|f| {
        ![
            ORIENTATION,
            tag::SOFTWARE,
            X_RESOLUTION,
            Y_RESOLUTION,
            RESOLUTION_UNIT,
        ]
        .contains(&f.tag)
    });
    d.main.extend([
        Field::short(ORIENTATION, 1),
        Field::rational(X_RESOLUTION, ppi, 1),
        Field::rational(Y_RESOLUTION, ppi, 1),
        Field::short(RESOLUTION_UNIT, 2),
        Field::ascii(tag::SOFTWARE, SOFTWARE),
    ]);
    d.exif.retain(|f| f.tag != COLOR_SPACE);
    d.exif.extend([
        Field::short(COLOR_SPACE, 1),
        Field::long(PIXEL_X_DIMENSION, width),
        Field::long(PIXEL_Y_DIMENSION, height),
    ]);
    d
}

/// The eXIf payload of a PNG export: the TIFF stream alone.
pub(super) fn png_exif(d: CameraExif) -> Vec<u8> {
    tiff_block(d)
}

/// The APP1 EXIF payload of a JPEG export.
pub(super) fn jpeg_exif(mut d: CameraExif) -> Vec<u8> {
    // Required for JPEG by the EXIF standard: chroma samples are centered.
    d.main.push(Field::short(YCBCR_POSITIONING, 1));
    tiff_block(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exif::tag::MODEL;

    #[test]
    fn a_lens_named_from_its_profile_is_exported_where_the_camera_wrote_none() {
        // A Pentax DNG's EXIF reads fine but has no LensModel; the lens is named
        // from the imported profile its LensType matched (issue #501).
        let m = Metadata {
            make: "Pentax".into(),
            model: "K-x".into(),
            lens_model: "smc PENTAX-DA 18-55mm F3.5-5.6 AL".into(),
            ..Default::default()
        };
        let embed = |exif: Vec<Field>, camera_fallback| Embed {
            camera: Some(CameraExif {
                main: vec![
                    Field::ascii(MAKE, "PENTAX"),
                    Field::ascii(MODEL, "PENTAX K-x"),
                ],
                exif,
                gps: Vec::new(),
            }),
            camera_fallback,
            ..Default::default()
        };
        let lens = |d: &CameraExif| {
            d.exif
                .iter()
                .filter(|f| f.tag == LENS_MODEL)
                .filter_map(Field::text)
                .collect::<Vec<_>>()
        };
        let d = directories(&m, &embed(Vec::new(), true), 4, 4);
        assert_eq!(lens(&d), ["smc PENTAX-DA 18-55mm F3.5-5.6 AL"]);
        // The camera's own name is kept.
        let d = directories(
            &m,
            &embed(vec![Field::ascii(LENS_MODEL, "DA 18-55")], true),
            4,
            4,
        );
        assert_eq!(lens(&d), ["DA 18-55"]);
        // Nothing of the camera's when its information is left out.
        let left_out = Embed {
            camera: Some(CameraExif::default()),
            camera_fallback: false,
            ..Default::default()
        };
        assert!(lens(&directories(&m, &left_out, 4, 4)).is_empty());
    }
}
