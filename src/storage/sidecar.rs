//! Identity-checked edits with read-only-folder fallback and conflict protection.
use super::{atomic_json, data_dir, migrate_recipe, versions};
use crate::{develop::Recipe, export::ExportOptions};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub size: u64,
    pub modified_ns: u128,
    pub prefix_hash: String,
}
impl Identity {
    pub fn read(path: &Path) -> Result<Self> {
        let m = fs::metadata(path)?;
        let mut bytes = Vec::new();
        File::open(path)?.take(65536).read_to_end(&mut bytes)?;
        let hash = bytes.iter().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
        });
        Ok(Self {
            size: m.len(),
            modified_ns: m.modified()?.duration_since(UNIX_EPOCH)?.as_nanos(),
            prefix_hash: format!("{hash:016x}"),
        })
    }
    fn key(&self) -> String {
        format!("{}-{}-{}", self.prefix_hash, self.size, self.modified_ns)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sidecar {
    pub schema: u32,
    pub pipeline: u32,
    pub source: Identity,
    pub recipe: Recipe,
    pub export: ExportOptions,
    /// Compressed bitmaps the recipe refers to by hash, base64-encoded (see
    /// `storage::bitmaps`). Omitted when empty, so older releases can read the file.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub bitmaps: std::collections::BTreeMap<String, String>,
}
impl Sidecar {
    pub fn bitmap(&self, hash: &str) -> Result<Option<super::bitmaps::Bitmap>> {
        self.bitmaps
            .get(hash)
            .map(|text| super::bitmaps::Bitmap::decompress(&super::bitmaps::from_base64(text)?))
            .transpose()
    }
}
pub fn sidecar_path(raw: &Path) -> PathBuf {
    let mut p = raw.as_os_str().to_os_string();
    p.push(".rawmakase.json");
    p.into()
}
fn fallback_at(identity: &Identity, store: &Path) -> PathBuf {
    store
        .to_path_buf()
        .join("sidecars")
        .join(format!("{}.json", identity.key()))
}
fn parse_sidecar(path: &Path, id: &Identity) -> Result<Sidecar> {
    ensure!(fs::metadata(path)?.len() < 16_000_000, "Sidecar too large");
    let mut v: serde_json::Value = serde_json::from_reader(File::open(path)?)?;
    migrate_recipe(&mut v)?;
    let s: Sidecar = serde_json::from_value(v)?;
    ensure!(
        &s.source == id,
        "RAW identity differs from saved edits: sidecar preserved"
    );
    s.recipe.validate()?;
    s.export.validate()?;
    Ok(s)
}
pub fn load(raw: &Path) -> Result<Option<Sidecar>> {
    load_at(raw, &data_dir())
}
fn load_at(raw: &Path, store: &Path) -> Result<Option<Sidecar>> {
    let id = Identity::read(raw)?;
    let primary = sidecar_path(raw);
    let backup = fallback_at(&id, store);
    // Validate both stores before choosing newest; never hide a conflicting primary.
    let a = if primary.exists() {
        Some(parse_sidecar(&primary, &id)?)
    } else {
        None
    };
    let b = if backup.exists() {
        Some(parse_sidecar(&backup, &id)?)
    } else {
        None
    };
    if a.is_some()
        && b.is_some()
        && fs::metadata(&backup)?.modified()? > fs::metadata(&primary)?.modified()?
    {
        Ok(b)
    } else {
        Ok(a.or(b))
    }
}
pub fn save(raw: &Path, recipe: &Recipe, export: &ExportOptions) -> Result<PathBuf> {
    save_at(raw, recipe, export, &data_dir())
}
fn save_at(raw: &Path, recipe: &Recipe, export: &ExportOptions, store: &Path) -> Result<PathBuf> {
    recipe.validate()?;
    export.validate()?;
    let source = Identity::read(raw)?;
    let primary = sidecar_path(raw);
    let mut bitmaps = std::collections::BTreeMap::new();
    if primary.exists() {
        bitmaps = parse_sidecar(&primary, &source)?.bitmaps;
    }
    let backup = fallback_at(&source, store);
    if backup.exists() {
        bitmaps.extend(parse_sidecar(&backup, &source)?.bitmaps);
    }
    let (schema, pipeline) = versions(recipe);
    let s = Sidecar {
        schema,
        pipeline,
        source,
        recipe: recipe.clone(),
        export: export.clone(),
        bitmaps,
    };
    match atomic_json(&primary, &s) {
        Ok(()) => Ok(primary),
        Err(e) => {
            let permission = e.downcast_ref::<std::io::Error>().is_some_and(|e| {
                e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(30)
            });
            if permission {
                atomic_json(&backup, &s)?;
                Ok(backup)
            } else {
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests;
