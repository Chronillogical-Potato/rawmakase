//! Opening a photo: LibRaw's facts about the file ([`Raw::open_file`]), then what
//! the file says beyond them and what the user's libraries add: the lens tables
//! the camera embedded, a DNG's profile, baseline exposure, colour matrix, lens
//! and default crop, and the imported lens profiles that fit. Everything that
//! develops, previews or exports a photo opens it here.
use crate::raw::Raw;
use anyhow::Result;
use std::path::Path;

/// The imported lens profiles, shared by everything that opens photos (Develop's
/// loaders, the Library's previews, exports, Sync Settings) so they are read once
/// each time the files change, not once per thread.
static LENS_PROFILES: crate::lens::lcp::LibraryCache = crate::lens::lcp::LibraryCache::new();

/// The photo at `path`, opened for developing.
pub fn open(path: &Path) -> Result<Raw> {
    let mut raw = Raw::open_file(path)?;
    let metadata = &mut raw.metadata;
    metadata.lens = crate::lens::embedded::read(path);
    if let Some(dng) = crate::dng::read(path) {
        // 0 is the DNG default; the camera table is for other raw formats.
        metadata.baseline_exposure = Some(dng.baseline_exposure.unwrap_or(0.));
        metadata.dng_neutral_calibration = dng.neutral_calibration;
        // A profile needs a forward matrix; one written as colour matrices alone
        // still describes the camera's colour, so keep that when it is all there is.
        let profile = dng
            .profile
            .as_deref()
            .and_then(|dcp| crate::camera_profiles::from_bytes(dcp).ok());
        let color_matrix = match profile {
            Some(_) => None,
            None => dng
                .profile
                .as_deref()
                .and_then(|dcp| crate::camera_profiles::d65_color_matrix(dcp).ok().flatten()),
        };
        let fits = profile.is_some_and(|p| p.ensure_camera(metadata).is_ok());
        metadata.embedded_dcp = dng.profile.filter(|_| fits).map(std::sync::Arc::from);
        // LibRaw has no XYZ-to-camera matrix for a DNG from a camera it does not
        // know, so one written with colour matrices but no profile would render
        // without a profile at all. Take the file's D65 matrix then: the same
        // matrix in the same direction, so nothing downstream has to know where
        // it came from. A camera LibRaw knows keeps LibRaw's matrix.
        if metadata.embedded_dcp.is_none()
            && metadata.cam_xyz.iter().flatten().all(|v| *v == 0.)
            && let Some(matrix) = color_matrix
            && matrix.iter().flatten().any(|v| *v != 0.)
        {
            metadata.cam_xyz = matrix;
        }
        if dng.lens.is_some() {
            metadata.lens = dng.lens;
        }
        if let Some(crop) = dng.crop {
            metadata.apply_default_crop(crop);
        }
    }
    metadata.lens_profiles = LENS_PROFILES.current().for_photo(metadata);
    Ok(raw)
}

#[cfg(test)]
mod tests {
    #[test]
    fn dng_without_baseline_exposure_uses_the_dng_default() {
        let chart = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/corpus/charts/synthetic-d65.dng"
        );
        let mut bytes = std::fs::read(chart).unwrap();
        // BaselineExposure, SRATIONAL, count 1: give it an invalid type so it is unread.
        let entry = [0x2a, 0xc6, 10, 0, 1, 0, 0, 0];
        let at = bytes.windows(8).position(|w| w == entry).unwrap();
        bytes[at + 2] = 0;
        // A closed file: Windows' LibRaw cannot open one a NamedTempFile holds open.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-baseline.dng");
        std::fs::write(&path, bytes).unwrap();
        let m = super::open(&path).unwrap().metadata;
        // A camera without a table row would otherwise take the table's median.
        assert_eq!(m.baseline_exposure, Some(0.));
        assert_eq!(crate::camera_profiles::reference::baseline_exposure(&m), 0.);
    }
}
