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

impl MetadataDefaults {
    /// What of these defaults a photo added from `file` gets: each one the
    /// metadata read from its files (`read`) and its EXIF both lack.
    pub(super) fn for_file(&self, file: &Path, read: Option<&Read>) -> Fill {
        let (creator, copyright) = (self.creator.trim(), self.copyright.trim());
        let creator = !creator.is_empty() && read.is_none_or(|r| r.creator.is_none());
        let copyright = !copyright.is_empty() && read.is_none_or(|r| r.copyright.is_none());
        if !creator && !copyright {
            return Fill::default();
        }
        // Read once for both.
        let exif = crate::exif::read(file);
        let has = |tag| {
            exif.as_ref()
                .and_then(|e| e.get(tag).and_then(crate::exif::Field::text))
                .is_some_and(|t| !t.is_empty())
        };
        Fill {
            creator: (creator && !has(crate::exif::tag::ARTIST))
                .then(|| Value::Set(vec![self.creator.trim().to_string()])),
            copyright: (copyright && !has(crate::exif::tag::COPYRIGHT))
                .then(|| Value::Set(LangAlt::new(self.copyright.trim()))),
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
