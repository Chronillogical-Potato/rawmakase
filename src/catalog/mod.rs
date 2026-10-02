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
}

mod copies;
mod edits;
mod ingest;
pub mod lightroom;
mod models;
pub use models::{Collection, CollectionKind, Folder, Photo, SavedEdit};
// Compatibility for existing clients.
pub use lightroom::{HistoryStep, convert_develop, import_lightroom};
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
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
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
        Ok(Self {
            path: path.into(),
            db,
        })
    }
    pub fn photos(&self) -> Result<Vec<Photo>> {
        let mappings = self.folders()?;
        let paths: std::collections::HashMap<_, _> =
            mappings.into_iter().map(|f| (f.id, f.path)).collect();
        let mut q = self.db.prepare(
            "SELECT p.id,p.folder,p.filename,p.captured,p.rating,p.flag,p.label,p.format,p.copy_name,p.master_id, COALESCE((SELECT group_concat(k.name, ', ')
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
                path: paths
                    .get(&folder)
                    .cloned()
                    .unwrap_or_default()
                    .join(&filename),
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
    pub fn folders(&self) -> Result<Vec<Folder>> {
        struct F {
            id: i64,
            root: i64,
            relative: String,
            base: String,
            mapped: Option<String>,
            count: usize,
        }
        let mut q = self.db.prepare(
            "SELECT f.id,f.root,f.relative_path,COALESCE(r.mapped_path,r.original_path),m.path,(SELECT count(*)
             FROM photos p WHERE p.folder=f.id)
             FROM folders f JOIN roots r ON r.id=f.root LEFT JOIN folder_mappings m ON m.folder=f.id
             ORDER BY r.original_path,f.relative_path",
        )?;
        let fs = q
            .query_map([], |r| {
                Ok(F {
                    id: r.get(0)?,
                    root: r.get(1)?,
                    // Folders added on Windows were stored with its separator;
                    // the Library's tree and saved sources split on '/'.
                    relative: if cfg!(windows) {
                        r.get::<_, String>(2)?.replace('\\', "/")
                    } else {
                        r.get(2)?
                    },
                    base: r.get(3)?,
                    mapped: r.get(4)?,
                    count: r.get::<_, i64>(5)? as usize,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(fs
            .iter()
            .map(|f| {
                let mut path = PathBuf::from(&f.base).join(&f.relative);
                // The most specific explicit mapping wins; descendants inherit a folder relink.
                let mut best = 0;
                for parent in &fs {
                    if parent.root == f.root
                        && let Some(mapped) = &parent.mapped
                        && let Ok(tail) = Path::new(&f.relative).strip_prefix(&parent.relative)
                        && parent.relative.len() >= best
                    {
                        path = Path::new(mapped).join(tail);
                        best = parent.relative.len();
                    }
                }
                Folder {
                    relative: f.relative.clone(),
                    id: f.id,
                    root: f.root,
                    name: if f.relative.is_empty() {
                        f.base.clone()
                    } else {
                        f.relative.clone()
                    },
                    path,
                    count: f.count,
                }
            })
            .collect())
    }
    pub fn collections(&self) -> Result<Vec<Collection>> {
        let mut query = self.db.prepare(
            "SELECT id, name, parent, kind,
                    (SELECT count(*) FROM collection_photos WHERE collection=c.id)
             FROM collections c ORDER BY name",
        )?;
        Ok(query
            .query_map([], |row| {
                let name: String = row.get(1)?;
                Ok(Collection {
                    id: row.get(0)?,
                    kind: CollectionKind::from_lightroom(&row.get::<_, String>(3)?, &name),
                    name,
                    parent: row.get(2)?,
                    count: row.get::<_, i64>(4)? as usize,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    /// Every collection's photos, by collection.
    pub fn collection_photos(
        &self,
    ) -> Result<std::collections::HashMap<i64, std::collections::HashSet<i64>>> {
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
    #[cfg(test)]
    pub(crate) fn collection_members(&self, id: i64) -> Result<std::collections::HashSet<i64>> {
        Ok(self
            .db
            .prepare("SELECT photo FROM collection_photos WHERE collection=?")?
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn roots(&self) -> Result<Vec<(i64, String, Option<String>)>> {
        Ok(self
            .db
            .prepare("SELECT id,original_path,mapped_path FROM roots ORDER BY id")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn relink_root(&self, id: i64, path: &Path) -> Result<()> {
        ensure!(path.is_dir(), "Choose an existing folder");
        ensure!(
            self.db.execute(
                "UPDATE roots SET mapped_path=? WHERE id=?",
                params![path.to_string_lossy(), id]
            )? == 1,
            "Unknown root"
        );
        Ok(())
    }
    pub fn relink_folder(&self, id: i64, path: &Path) -> Result<()> {
        ensure!(path.is_dir(), "Choose an existing folder");
        self.db.execute("INSERT INTO folder_mappings(folder,path) VALUES(?,?) ON CONFLICT(folder) DO UPDATE SET path=excluded.path",params![id,path.to_string_lossy()])?;
        Ok(())
    }
    pub fn set_metadata(&mut self, id: i64, rating: i32, flag: i32, label: &str) -> Result<()> {
        self.set_metadata_of(&[(id, rating, flag, label.into())])
    }
    /// Sets rating, flag and label of several photos in one transaction:
    /// all of them are saved, or none.
    pub fn set_metadata_of(&mut self, changes: &[(i64, i32, i32, String)]) -> Result<()> {
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

pub mod preview_cache;
#[cfg(test)]
mod private_tests;
#[cfg(test)]
mod tests;
