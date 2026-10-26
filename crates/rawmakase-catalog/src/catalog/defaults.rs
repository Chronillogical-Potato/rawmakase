//! Default Creator and Copyright from Preferences, written for photos added
//! from folders where neither the file nor its sidecar has one: below an
//! edit or an imported value, above the file's EXIF, and never at export.
use super::PhotoId;
use super::db::Write;
use crate::metadata::{LangAlt, Value};
use crate::xmp::descriptive::Read;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MetadataDefaults {
    pub creator: String,
    pub copyright: String,
}
impl MetadataDefaults {
    fn path() -> PathBuf {
        crate::storage::data_dir().join("metadata-defaults.json")
    }
    pub fn load() -> Self {
        crate::storage::read_json_or_default(&Self::path())
    }
    pub fn save(&self) -> Result<()> {
        crate::storage::atomic_json(&Self::path(), self)
    }
    pub fn is_empty(&self) -> bool {
        self.creator.trim().is_empty() && self.copyright.trim().is_empty()
    }
}

/// Reads the camera's EXIF of a photo's file. The catalog can't read it
/// from every RAW itself (a CR3's, MRW's or X3F's is found by RAWmakase's
/// RAW decoders), so the app passes its reader in: `photo::camera_exif`.
pub type CameraExifReader = fn(&Path) -> Option<crate::exif::CameraExif>;

/// What photos added from folders get where their files have none: the
/// Preferences' Creator and Copyright, which give way to the camera's own,
/// read by the reader given. The default is none, which reads nothing.
#[derive(Clone, Debug, Default)]
pub struct NewPhotoDefaults {
    metadata: MetadataDefaults,
    camera_exif: Option<CameraExifReader>,
}
impl NewPhotoDefaults {
    /// `metadata` where `camera_exif` finds no Artist or Copyright of the
    /// camera's.
    pub fn new(metadata: MetadataDefaults, camera_exif: CameraExifReader) -> Self {
        Self {
            metadata,
            camera_exif: Some(camera_exif),
        }
    }
    /// What of the defaults a photo added from `file` gets: each one the
    /// metadata read from its files (`read`) and its camera's EXIF both lack.
    pub(super) fn for_file(&self, file: &Path, read: Option<&Read>) -> Fill {
        let defaults = &self.metadata;
        let (creator, copyright) = (defaults.creator.trim(), defaults.copyright.trim());
        let creator = !creator.is_empty() && read.is_none_or(|r| r.creator.is_none());
        let copyright = !copyright.is_empty() && read.is_none_or(|r| r.copyright.is_none());
        let Some(camera_exif) = self.camera_exif.filter(|_| creator || copyright) else {
            return Fill::default();
        };
        // Read once for both.
        let exif = camera_exif(file);
        let has = |tag| {
            exif.as_ref()
                .and_then(|e| e.get(tag).and_then(crate::exif::Field::text))
                .is_some_and(|t| !t.is_empty())
        };
        Fill {
            creator: (creator && !has(crate::exif::tag::ARTIST))
                .then(|| Value::Set(vec![defaults.creator.trim().to_string()])),
            copyright: (copyright && !has(crate::exif::tag::COPYRIGHT))
                .then(|| Value::Set(LangAlt::new(defaults.copyright.trim()))),
        }
    }
}

/// The defaults one added photo gets, worked out before it is written.
#[derive(Debug, Default)]
pub(super) struct Fill {
    creator: Option<Value<Vec<String>>>,
    copyright: Option<Value<LangAlt>>,
}

/// Writes `fill` for photo `id`, in `w`'s transaction, where it has no
/// value of its own.
pub(super) fn fill(w: &mut Write<'_>, id: PhotoId, fill: &Fill) -> Result<()> {
    if fill.creator.is_none() && fill.copyright.is_none() {
        return Ok(());
    }
    let mut d = super::descriptive::read(w, id)?;
    if d.creator.is_none() {
        d.creator.clone_from(&fill.creator);
    }
    if d.copyright.is_none() {
        d.copyright.clone_from(&fill.copyright);
    }
    super::descriptive::write(w, id, &d)
}
