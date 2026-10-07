//! Independent, versioned SQLite catalogs. Lightroom sources are never opened writable.
//!
//! `Catalog` owns the connection; `schema.sql` owns every table. The catalog's
//! operations are grouped by what they change: browsing queries and relinking
//! here, edits in `edits`, virtual copies in `copies`, adding folders in
//! `ingest`, and everything Lightroom-specific under `lightroom`.
use anyhow::{Result, ensure};
use rusqlite::{Connection, OpenFlags, params};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
const APPLICATION_ID: i64 = 0x4f4d4152;
const VERSION: i64 = 1;

pub struct Catalog {
    pub path: PathBuf,
    db: Connection,
    /// The computer it is open on, whose folder locations apply.
    computer: locations::Computer,
}

mod copies;
mod defaults;
mod descriptive;
mod develop_history;
mod edit_records;
mod edits;
mod info;
mod ingest;
pub mod legacy_sidecar;
pub mod lightroom;
pub mod locations;
mod models;
// XMP metadata sidecars; `legacy_sidecar` is the old `*.rawmakase.json` edits.
mod sidecar;
mod snapshots;
pub use crate::ids::{CollectionId, FolderId, PhotoId, RootId};
pub use crate::xmp::descriptive::Read as FileMetadata;
pub use defaults::MetadataDefaults;
pub use descriptive::MetadataSnapshot;
pub use develop_history::{HistoryUpdate, SavedHistory, SavedStep};
pub use edits::{EditChange, EditToSave};
pub use ingest::{Ambiguity, Choice, Conflict};
pub use lightroom::HistoryStep;
pub use locations::{Override, Overrides, RootLocations};
pub use models::{Collection, CollectionKind, Folder, Photo, QUICK_COLLECTION};
pub use sidecar::{SidecarReport, read_file as read_file_metadata, sidecars};
pub use snapshots::{Snapshot, SnapshotSettings};
impl Catalog {
    pub fn create(path: &Path) -> Result<Self> {
        ensure!(!path.exists(), "Catalog already exists: {}", path.display());
        let parent = crate::storage::parent_dir(path);
        std::fs::create_dir_all(parent)?;
        let file = tempfile::NamedTempFile::new_in(parent)?;
        let db = Connection::open(file.path())?;
        db.execute_batch(&format!(
            "PRAGMA application_id={APPLICATION_ID}; PRAGMA user_version={VERSION};"
        ))?;
        db.execute_batch(include_str!("schema.sql"))?;
        drop(db);
        file.persist_noclobber(path)?;
        Self::open(path)
    }
    /// Opens a catalog on this computer.
    pub fn open(path: &Path) -> Result<Self> {
        Self::open_as(path, &locations::Computer::this())
    }
    /// Opens a catalog on `computer`, whose folder locations apply.
    pub fn open_as(path: &Path, computer: &locations::Computer) -> Result<Self> {
        let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        ensure!(
            db.query_row("PRAGMA application_id", [], |r| r.get::<_, i64>(0))? == APPLICATION_ID,
            "Not an RAWmakase catalog"
        );
        ensure!(
            db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))? == VERSION,
            "Unsupported RAWmakase catalog version; file left unchanged"
        );
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
        // The schema is idempotent: a catalog from an earlier release gains the
        // tables added since.
        db.execute_batch(include_str!("schema.sql"))?;
        locations::prepare(&mut db, computer)?;
        Ok(Self {
            path: path.into(),
            db,
            computer: computer.clone(),
        })
    }
    /// The connection, for tests that set up stored state directly.
    #[cfg(any(test, feature = "test-support"))]
    pub fn db_for_tests(&self) -> &Connection {
        &self.db
    }
    pub fn photos(&self) -> Result<Vec<Photo>> {
        let mappings = self.folders()?;
        let paths: std::collections::HashMap<_, _> =
            mappings.into_iter().map(|f| (f.id, f.path)).collect();
        let mut q = self.db.prepare(
            "SELECT p.id,p.folder,p.filename,p.captured,p.rating,p.flag,p.label,p.format,p.copy_name,p.master_id, COALESCE((SELECT string_agg(k.name, ', ')
             FROM photo_keywords pk JOIN keywords k ON k.id=pk.keyword WHERE pk.photo=p.id),''),length(COALESCE(p.lightroom_develop,''))>0
             FROM photos p
             ORDER BY p.captured,p.filename,p.id",
        )?;
        Ok(q.query_map([], |r| {
            let folder = r.get(1)?;
            let filename: String = r.get(2)?;
            Ok(Photo {
                id: r.get(0)?,
                folder,
                // No path where its folder can't be on this computer.
                path: paths
                    .get(&folder)
                    .filter(|path| !path.as_os_str().is_empty())
                    .map(|path| path.join(&filename))
                    .unwrap_or_default(),
                filename,
                captured: r.get(3)?,
                rating: r.get(4)?,
                flag: r.get(5)?,
                label: r.get(6)?,
                format: r.get(7)?,
                copy_name: r.get(8)?,
                master: r.get(9)?,
                keywords: r.get(10)?,
                has_lightroom_edits: r.get(11)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
    }
    /// Every folder where it is on this computer (see `locations`); an
    /// empty path where it can't be here.
    pub fn folders(&self) -> Result<Vec<Folder>> {
        let rows = self.location_rows()?;
        let mut q = self.db.prepare(
            "SELECT f.id,f.root,r.original_path,f.relative_path,p.path,(SELECT count(*)
             FROM photos p WHERE p.folder=f.id)
             FROM folders f JOIN roots r ON r.id=f.root LEFT JOIN folder_paths p ON p.folder=f.id
             ORDER BY r.original_path,COALESCE(p.path,f.relative_path)",
        )?;
        let folders = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, FolderId>(0)?,
                    r.get::<_, RootId>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, i64>(5)? as usize,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(folders
            .into_iter()
            .map(|(id, root, original, relative, logical, count)| {
                // An older release may have added it since this catalog opened.
                let relative =
                    logical.unwrap_or_else(|| locations::logical_from_legacy(&original, &relative));
                let own = rows.get(&root).map_or(&[][..], |r| &r[..]);
                let path = locations::resolve_in(&original, own, &relative, cfg!(windows))
                    .unwrap_or_default();
                Folder {
                    relative,
                    id,
                    root,
                    path,
                    count,
                }
            })
            .collect())
    }
    pub fn collections(&self) -> Result<Vec<Collection>> {
        let mut query = self
            .db
            .prepare("SELECT id, name, parent, kind FROM collections ORDER BY name")?;
        Ok(query
            .query_map([], |row| {
                let name: String = row.get(1)?;
                Ok(Collection {
                    id: row.get(0)?,
                    kind: CollectionKind::from_lightroom(&row.get::<_, String>(3)?, &name),
                    name,
                    parent: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    /// Lightroom's Quick Collection: the one imported with the catalog, or a
    /// new one made the same way.
    pub fn quick_collection(&mut self) -> Result<CollectionId> {
        use rusqlite::OptionalExtension;
        const KIND: &str = "com.adobe.ag.library.collection";
        let found = self
            .db
            .query_row(
                "SELECT id FROM collections WHERE name=?1 AND kind=?2 AND parent IS NULL",
                params![models::QUICK_COLLECTION, KIND],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = found {
            return Ok(id);
        }
        Ok(self.db.query_row(
            "INSERT INTO collections(name, parent, kind) VALUES (?, NULL, ?) RETURNING id",
            params![models::QUICK_COLLECTION, KIND],
            |r| r.get(0),
        )?)
    }
    /// Adds `add` to and removes `remove` from a collection, in one transaction.
    pub fn change_collection(
        &mut self,
        collection: CollectionId,
        add: &[PhotoId],
        remove: &[PhotoId],
    ) -> Result<()> {
        let tx = self.db.transaction()?;
        for photo in add {
            tx.execute(
                "INSERT INTO collection_photos(collection, photo) VALUES (?, ?)
                 ON CONFLICT DO NOTHING",
                params![collection, photo],
            )?;
        }
        for photo in remove {
            tx.execute(
                "DELETE FROM collection_photos WHERE collection=? AND photo=?",
                params![collection, photo],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Every collection's photos, by collection.
    pub fn collection_photos(
        &self,
    ) -> Result<std::collections::HashMap<CollectionId, std::collections::HashSet<PhotoId>>> {
        let mut members: std::collections::HashMap<_, std::collections::HashSet<_>> =
            Default::default();
        let mut query = self
            .db
            .prepare("SELECT collection, photo FROM collection_photos")?;
        for row in query.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))? {
            let (collection, photo) = row?;
            members.entry(collection).or_default().insert(photo);
        }
        Ok(members)
    }
    /// Makes every write of descriptive metadata fail, as on a full disk.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fail_metadata_writes(&self) -> Result<()> {
        self.db.execute_batch(
            "CREATE TEMP TRIGGER fail_metadata BEFORE INSERT ON photo_fields
             BEGIN SELECT RAISE(FAIL, 'disk full'); END;",
        )?;
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn collection_members(
        &self,
        id: CollectionId,
    ) -> Result<std::collections::HashSet<PhotoId>> {
        Ok(self
            .db
            .prepare("SELECT photo FROM collection_photos WHERE collection=?")?
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    /// Every root: where it was added and this computer's location of it.
    pub fn roots(&self) -> Result<Vec<(RootId, String, Option<String>)>> {
        Ok(self
            .db
            .prepare(
                "SELECT r.id,r.original_path,l.path FROM roots r LEFT JOIN folder_locations l
                 ON l.root=r.id AND l.relative_path='' AND l.computer=? ORDER BY r.id",
            )?
            .query_map([&self.computer.id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    /// A fact about the catalog itself, from the `meta` table.
    fn meta(&self, key: &str) -> Result<Option<String>> {
        use rusqlite::OptionalExtension;
        Ok(self
            .db
            .query_row("SELECT value FROM meta WHERE key=?", [key], |r| r.get(0))
            .optional()?)
    }
    fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        set_meta(&self.db, key, value)
    }
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_metadata(&mut self, id: PhotoId, rating: i32, flag: i32, label: &str) -> Result<()> {
        self.set_metadata_of(&[(id, rating, flag, label.into())])
    }
    /// Sets rating, flag and label of several photos in one transaction:
    /// all of them are saved, or none.
    pub fn set_metadata_of(&mut self, changes: &[(PhotoId, i32, i32, String)]) -> Result<()> {
        let tx = self.db.transaction()?;
        for (id, rating, flag, label) in changes {
            ensure!((0..=5).contains(rating), "Rating must be between 0 and 5");
            ensure!((-1..=1).contains(flag), "Invalid pick/reject flag");
            ensure!(
                tx.execute(
                    "UPDATE photos SET rating=?,flag=?,label=? WHERE id=?",
                    params![rating, flag, label, id]
                )? == 1,
                "Unknown photo"
            );
        }
        tx.commit()?;
        Ok(())
    }
}

/// Records a fact about the catalog in its `meta` table.
fn set_meta(db: &Connection, key: &str, value: &str) -> Result<()> {
    db.execute(
        "INSERT INTO meta(key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub use sidecar::Merge;
#[cfg(test)]
mod descriptive_tests;
#[cfg(test)]
mod locations_tests;
#[cfg(test)]
mod portable_sql_tests;
pub mod preview_cache;
#[cfg(test)]
mod private_tests;
#[cfg(test)]
mod tests;
