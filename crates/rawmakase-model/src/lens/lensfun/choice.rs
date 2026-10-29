//! A Lensfun lens as an edit chooses it: which lens (its identity in the database),
//! the camera it was chosen for, and a copy of its calibration with where it came
//! from. The copy is what renders, so updating or removing the database never changes
//! an edit; an edit read from XMP names only the lens and stays unresolved, rendering
//! no Lensfun correction, until the user resolves it against an installed database.
use super::matching::{Photo, exif_crop, frame_crop};
use super::names::same_text;
use crate::camera_data::Metadata;
use crate::optics::LensCorrection;
use crate::optics::lensfun::{Calibration, DISTANCE, Evaluation, Frame, Shortfall, Shot};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Where Lensfun's data comes from, and its terms.
pub const SOURCE_URL: &str = "https://lensfun.github.io/db/version_2.tar.bz2";
pub const ATTRIBUTION: &str = "Lensfun contributors";
pub const LICENSE: &str = "CC BY-SA 3.0";
pub const LICENSE_URI: &str = "https://creativecommons.org/licenses/by-sa/3.0/";
pub const CONVERTED: &str = "Converted from Lensfun's XML database by RAWmakase";

/// A lens's identity in Lensfun's database: maker and model as it identifies them
/// (not the shown names), its mounts, and the crop factor and aspect ratio it was
/// calibrated at, which tell apart entries of the same name.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LensfunLens {
    pub maker: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<String>,
    pub crop: f32,
    pub aspect: f32,
}

/// The camera a lens was chosen for: its EXIF make and model, and the crop factor
/// and aspect ratio of its sensor (Lensfun's, or from the 35 mm equivalent focal
/// length when Lensfun doesn't know it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LensfunCamera {
    pub make: String,
    pub model: String,
    pub crop: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspect: Option<f32>,
}

/// Where a calibration was copied from, under which terms: kept with every copy, so
/// an exported catalog carries its attribution.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub source: String,
    /// The database's timestamp (seconds since 1970).
    pub database: i64,
    pub attribution: String,
    pub license: String,
    pub license_uri: String,
    pub note: String,
}
impl Provenance {
    pub fn lensfun(database: i64) -> Self {
        Self {
            source: SOURCE_URL.into(),
            database,
            attribution: ATTRIBUTION.into(),
            license: LICENSE.into(),
            license_uri: LICENSE_URI.into(),
            note: CONVERTED.into(),
        }
    }
}

/// A Lensfun lens an edit uses.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LensfunChoice {
    pub lens: LensfunLens,
    /// The name shown for it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<LensfunCamera>,
    /// The calibration, copied from the database when the lens was chosen; `None`
    /// while the edit only names the lens (from XMP) and it isn't resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calibration: Option<Arc<Calibration>>,
    /// Where the copy came from. An unresolved choice records the database the
    /// identity was written from, when known.
    pub provenance: Provenance,
}

/// What a Lensfun choice does for one photo.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    pub correction: Option<LensCorrection>,
    /// Why it corrects less than its calibration holds, or nothing.
    pub notes: Vec<String>,
}

impl LensfunChoice {
    /// The choice of `lens` from `db` for a photo, with its calibration copied.
    pub fn of(lens: &super::Lens, db: &super::Database, camera: Option<LensfunCamera>) -> Self {
        Self {
            lens: lens.identity(),
            label: lens.label(),
            camera,
            calibration: Some(Arc::new(lens.calibration.clone())),
            provenance: Provenance::lensfun(db.timestamp),
        }
    }
    /// A choice naming a lens only, as XMP carries it.
    pub fn unresolved(lens: LensfunLens, label: String, database: i64) -> Self {
        Self {
            lens,
            label,
            camera: None,
            calibration: None,
            provenance: Provenance::lensfun(database),
        }
    }
    pub fn is_resolved(&self) -> bool {
        self.calibration.is_some()
    }
    /// The name to show.
    pub fn name(&self) -> &str {
        if self.label.is_empty() {
            &self.lens.model
        } else {
            &self.label
        }
    }
    /// The choice checked as values, as a saved edit's must be.
    pub fn validate(&self) -> bool {
        let finite = |v: f32| v.is_finite() && v > 0.;
        !self.lens.model.is_empty()
            && finite(self.lens.crop)
            && self.lens.aspect >= 1.
            && self.lens.aspect.is_finite()
            && self
                .camera
                .as_ref()
                .is_none_or(|c| finite(c.crop) && c.aspect.is_none_or(|a| a.is_finite() && a >= 1.))
            && self.calibration.as_ref().is_none_or(|c| c.validate())
    }
    /// The photo's frame as the calibration needs it: the decoded image, its default
    /// crop and that crop's crop factor; `None` without a crop factor.
    pub fn frame(&self, m: &Metadata) -> Option<Frame> {
        let size = [m.width as f32, m.height as f32];
        let rect = if m.crop_width > 0 && m.crop_height > 0 {
            [
                m.crop_left as f32,
                m.crop_top as f32,
                m.crop_width as f32,
                m.crop_height as f32,
            ]
        } else {
            [0., 0., size[0], size[1]]
        };
        let photo = photo_of(m, [rect[2], rect[3]]);
        let crop = match &self.camera {
            Some(c) if same_text(&c.make, photo.make) && same_text(&c.model, photo.model) => {
                Some(frame_crop(c.crop, c.aspect, photo.frame))
            }
            _ => exif_crop(&photo),
        }?;
        Some(Frame { size, rect, crop })
    }
    /// The correction for photo `m`, from the copied calibration only.
    pub fn apply(&self, m: &Metadata) -> Applied {
        let Some(calibration) = &self.calibration else {
            return Applied {
                correction: None,
                notes: vec!["isn't resolved against an installed Lensfun database".into()],
            };
        };
        let Some(frame) = self.frame(m) else {
            return Applied {
                correction: None,
                notes: vec![
                    "needs the camera's crop factor, which this photo doesn't record".into(),
                ],
            };
        };
        let shot = Shot {
            focal: m.focal,
            aperture: m.aperture,
            distance: DISTANCE,
        };
        let Evaluation {
            correction,
            shortfalls,
        } = calibration.evaluate(&frame, &shot, &format!("Lensfun: {}", self.name()));
        Applied {
            correction,
            notes: shortfalls.iter().map(Shortfall::describe).collect(),
        }
    }
}

/// What the matcher reads of photo `m`, with a frame of `frame` pixels: the file's
/// own EXIF make and model when the reader kept them, else RAWmakase's names.
pub fn photo_of(m: &Metadata, frame: [f32; 2]) -> Photo<'_> {
    let (make, model) = if m.exif_make.is_empty() && m.exif_model.is_empty() {
        (m.make.as_str(), m.model.as_str())
    } else {
        (m.exif_make.as_str(), m.exif_model.as_str())
    };
    Photo {
        make,
        model,
        lens: &m.lens_model,
        focal: m.focal,
        focal_35mm: m.focal_35mm,
        frame,
    }
}

/// The photo frame of `m`, in pixels: its default crop, else the decoded image.
pub fn frame_size(m: &Metadata) -> [f32; 2] {
    if m.crop_width > 0 && m.crop_height > 0 {
        [m.crop_width as f32, m.crop_height as f32]
    } else {
        [m.width as f32, m.height as f32]
    }
}
