//! The mask rasters edits and snapshots refer to: stored with the rows that
//! refer to them, in the `bitmaps` table, and read back through the loader the app
//! installs (see [`rawmakase_model::storage::mask_assets`]).
//!
//! Rasters are immutable and named by content, so nothing here replaces or
//! collects them: a raster a History, snapshot or virtual copy still names must
//! stay readable, and the catalog is allowed to grow for that.
use super::db::{Db, Reads, Write, sql};
use super::{Catalog, CatalogLocation};
use crate::storage::bitmaps::Bitmap;
use crate::storage::mask_assets::{self, AssetError, AssetLoader};
use anyhow::{Result, ensure};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// The rasters one write refers to: the ones to store, and every one that has to exist
/// once it commits.
#[derive(Default)]
pub(super) struct Assets {
    unsaved: Vec<(String, Arc<Vec<u8>>)>,
    required: Vec<String>,
}
impl Assets {
    /// Stores the unsaved rasters and checks that every required one is there, in
    /// the transaction of the rows that refer to them.
    pub(super) fn write(&self, w: &mut Write<'_>) -> Result<()> {
        for (id, blob) in &self.unsaved {
            let inserted = w.execute(
                sql!("INSERT INTO bitmaps(hash, data) VALUES (?, ?) ON CONFLICT DO NOTHING"),
                &[id, blob.as_ref()],
            )?;
            // Already there: it must be what its ID names, or the edit would point at
            // damaged data once the good copy held here is let go. A damaged row is
            // replaced with the good one.
            if inserted == 0 {
                let stored: Option<Vec<u8>> =
                    w.read_optional(sql!("SELECT data FROM bitmaps WHERE hash=?"), &[id])?;
                let intact = stored
                    .and_then(|bytes| Bitmap::decompress(&bytes).ok())
                    .is_some_and(|b| b.matches_id(id));
                if !intact {
                    w.execute(
                        sql!("UPDATE bitmaps SET data=? WHERE hash=?"),
                        &[blob.as_ref(), id],
                    )?;
                }
            }
        }
        for id in &self.required {
            let found: Option<i64> =
                w.read_optional(sql!("SELECT 1 FROM bitmaps WHERE hash=?"), &[id])?;
            ensure!(found.is_some(), AssetError::Missing(id.clone()));
        }
        Ok(())
    }
    /// Notes that the write committed: the rasters can be read again from here.
    pub(super) fn saved(&self) {
        mask_assets::mark_saved(self.required.iter().map(String::as_str));
    }
}

impl Catalog {
    /// The rasters a write that refers to `ids` has to store or find. Refused in a
    /// catalog of the first format, whose older releases would not understand them.
    pub(super) fn assets_of<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> Result<Assets> {
        let mut required: Vec<String> = ids.into_iter().map(str::to_string).collect();
        required.sort();
        required.dedup();
        if required.is_empty() {
            return Ok(Assets::default());
        }
        ensure!(
            self.supports_raster_masks()?,
            "This catalog needs upgrading before it can keep masks made from a selection"
        );
        let unsaved = mask_assets::unsaved(required.iter().map(String::as_str));
        Ok(Assets { unsaved, required })
    }
    /// The catalog's format version.
    pub fn format_version(&self) -> Result<i64> {
        self.db.version()
    }
    /// Whether the catalog is of a format that keeps raster masks.
    pub fn supports_raster_masks(&self) -> Result<bool> {
        Ok(self.db.version()? >= super::db::VERSION)
    }
    /// Upgrades a catalog of the first format so it can keep raster masks, after
    /// writing a consistent backup next to it. Returns the backup's path. The
    /// catalog must not be open anywhere else, and older releases cannot open it
    /// afterwards. `None` when it already was.
    pub fn upgrade_for_raster_masks(&mut self) -> Result<Option<PathBuf>> {
        if self.supports_raster_masks()? {
            return Ok(None);
        }
        let CatalogLocation::File(path) = &self.location;
        let name = path.file_name().map_or_else(
            || "catalog".to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let backup = (0..1000)
            .map(|n| {
                let suffix = if n == 0 {
                    String::new()
                } else {
                    format!("-{n}")
                };
                path.with_file_name(format!("{name}.before-upgrade-backup{suffix}"))
            })
            .find(|candidate| !candidate.exists())
            .ok_or_else(|| anyhow::anyhow!("Too many backups next to the catalog"))?;
        self.db.upgrade(&backup)?;
        Ok(Some(backup))
    }
    /// A reader of this catalog's rasters for the mask asset store. It opens its
    /// own connection the first time one is needed, so it can be called from any
    /// thread and holds nothing meanwhile.
    pub fn mask_asset_loader(&self) -> Arc<dyn AssetLoader> {
        Arc::new(CatalogAssets {
            location: self.location.clone(),
            db: Mutex::new(None),
        })
    }
}

struct CatalogAssets {
    location: CatalogLocation,
    db: Mutex<Option<Db>>,
}
impl AssetLoader for CatalogAssets {
    fn source(&self) -> String {
        let CatalogLocation::File(path) = &self.location;
        path.display().to_string()
    }
    fn load(&self, id: &str) -> Result<Option<Bitmap>, AssetError> {
        let corrupt =
            |why: &dyn std::fmt::Display| AssetError::Corrupt(id.to_string(), why.to_string());
        let mut guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(Db::open(&self.location).map_err(|e| corrupt(&e))?);
        }
        let db = guard.as_ref().expect("opened above");
        let data: Option<Vec<u8>> =
            match db.read_optional(sql!("SELECT data FROM bitmaps WHERE hash=?"), &[&id]) {
                Ok(data) => data,
                Err(error) => {
                    // A connection that failed is opened again next time.
                    *guard = None;
                    return Err(corrupt(&error));
                }
            };
        data.map(|bytes| Bitmap::decompress(&bytes).map_err(|e| corrupt(&e)))
            .transpose()
    }
}
