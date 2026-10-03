//! Default Creator and Copyright from Preferences, written for photos added
//! from folders where neither the file nor its sidecar has one: below an
//! edit or an imported value, above the file's EXIF, and never at export.
use super::{Catalog, LangAlt, Value};
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
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }
    pub fn save(&self) -> Result<()> {
        crate::storage::atomic_json(&Self::path(), self)
    }
    pub fn is_empty(&self) -> bool {
        self.creator.trim().is_empty() && self.copyright.trim().is_empty()
    }
}

impl Catalog {
    /// Writes `defaults` for the photos given, where a photo has no value of
    /// its own and its file's EXIF none either. One transaction.
    pub(super) fn apply_defaults(
        &mut self,
        added: &[(i64, PathBuf)],
        defaults: &MetadataDefaults,
    ) -> Result<()> {
        if defaults.is_empty() {
            return Ok(());
        }
        let exif = |path: &Path, tag| {
            crate::export::exif::read(path)
                .and_then(|e| e.get(tag).and_then(crate::export::exif::Field::text))
                .is_some_and(|t| !t.is_empty())
        };
        let (creator, copyright) = (defaults.creator.trim(), defaults.copyright.trim());
        let tx = self.db.transaction()?;
        for (id, path) in added {
            let mut d = super::descriptive::read(&tx, *id)?;
            let before = d.clone();
            if !creator.is_empty() && d.creator.is_none() && !exif(path, 0x013b) {
                d.creator = Some(Value::Set(vec![creator.to_string()]));
            }
            if !copyright.is_empty() && d.copyright.is_none() && !exif(path, 0x8298) {
                d.copyright = Some(Value::Set(LangAlt::new(copyright)));
            }
            if d != before {
                super::descriptive::write(&tx, *id, &d)?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}
