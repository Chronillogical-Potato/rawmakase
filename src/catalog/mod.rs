//! Independent, versioned SQLite catalogs. Lightroom sources are never opened writable.
use crate::{develop::Recipe, export::ExportOptions, storage::Identity};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
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

pub mod lightroom;
mod models;
pub use models::{Collection, Folder, Photo, SavedEdit};
/// Lightroom's develop history per photo: one full settings snapshot per step.
const LIGHTROOM_HISTORY_TABLE: &str = "CREATE TABLE IF NOT EXISTS lightroom_history(
    photo INTEGER NOT NULL,
    position INTEGER NOT NULL,
    name TEXT NOT NULL DEFAULT '',
    created REAL,
    text TEXT NOT NULL,
    PRIMARY KEY(photo, position));";
/// Copies history steps from an attached Lightroom catalog named `lr`.
pub(crate) const COPY_LIGHTROOM_HISTORY: &str =
    "INSERT OR IGNORE INTO lightroom_history(photo,position,name,created,text)
    SELECT image, row_number() OVER (PARTITION BY image ORDER BY dateCreated, id_local),
           COALESCE(name,''), dateCreated, text
    FROM lr.Adobe_libraryImageDevelopHistoryStep
    WHERE text IS NOT NULL AND image IN (SELECT id FROM photos);";
/// Lightroom stores history snapshots either as text or as a 4-byte
/// big-endian length followed by a zlib stream.
fn decode_history_text(bytes: &[u8]) -> Option<String> {
    if bytes.len() > 6 && bytes[4] == 0x78 {
        use std::io::Read;
        let mut text = String::new();
        flate2::read::ZlibDecoder::new(&bytes[4..])
            .read_to_string(&mut text)
            .ok()?;
        return Some(text);
    }
    String::from_utf8(bytes.to_vec()).ok()
}
/// One Lightroom history step.
#[derive(Clone, Debug)]
pub struct HistoryStep {
    pub name: String,
    /// Seconds since 2001-01-01 (Lightroom's epoch).
    pub created: Option<f64>,
    pub text: String,
}
// Compatibility for existing clients.
pub use lightroom::{convert_develop, import_lightroom};
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
        // Added after version 1 shipped; additive, so older catalogs gain it on open.
        db.execute_batch(LIGHTROOM_HISTORY_TABLE)?;
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
            "SELECT p.id,p.folder,p.filename,p.captured,p.rating,p.flag,p.label,p.format,p.copy_name, COALESCE((SELECT group_concat(k.name, ', ')
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
                keywords: r.get(9)?,
                has_lightroom_edits: r.get(10)?,
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
                    relative: r.get(2)?,
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
                Ok(Collection {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    parent: row.get(2)?,
                    smart: row.get::<_, String>(3)?.contains("smart_collection"),
                    count: row.get::<_, i64>(4)? as usize,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
    pub fn collection_members(&self, id: i64) -> Result<std::collections::HashSet<i64>> {
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
    pub fn set_metadata(&self, id: i64, rating: i32, flag: i32, label: &str) -> Result<()> {
        ensure!((0..=5).contains(&rating), "Rating must be between 0 and 5");
        ensure!((-1..=1).contains(&flag), "Invalid pick/reject flag");
        ensure!(
            self.db.execute(
                "UPDATE photos SET rating=?,flag=?,label=? WHERE id=?",
                params![rating, flag, label, id]
            )? == 1,
            "Unknown photo"
        );
        Ok(())
    }
    pub fn save_edit(
        &self,
        id: i64,
        path: &Path,
        recipe: &Recipe,
        export: &ExportOptions,
    ) -> Result<()> {
        recipe.validate()?;
        export.validate()?;
        let identity = Identity::read(path)?;
        // Refuse replacing an edit after the underlying source changed.
        let _ = self.load_edit(id, path)?;
        ensure!(self.db.execute("UPDATE photos SET recipe=?,export_options=?,identity=?,edited_at=CURRENT_TIMESTAMP WHERE id=?",params![serde_json::to_string(recipe)?,serde_json::to_string(export)?,serde_json::to_string(&identity)?,id])?==1,"Unknown photo");
        Ok(())
    }
    pub fn load_edit(&self, id: i64, path: &Path) -> Result<Option<SavedEdit>> {
        let (recipe, export, identity): (Option<String>, Option<String>, Option<String>) =
            self.db.query_row(
                "SELECT recipe,export_options,identity FROM photos WHERE id=?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
        if let Some(recipe) = recipe {
            let saved: Identity =
                serde_json::from_str(&identity.context("Missing photo identity")?)?;
            ensure!(
                saved == Identity::read(path)?,
                "Photo changed since this catalog edit was saved; catalog edit protected"
            );
            let recipe: Recipe = serde_json::from_str(&recipe)?;
            recipe.validate()?;
            let export: ExportOptions =
                serde_json::from_str(&export.context("Missing export settings")?)?;
            export.validate()?;
            Ok(Some(SavedEdit { recipe, export }))
        } else {
            Ok(None)
        }
    }
    /// Lightroom's history for a photo, oldest step first.
    pub fn lightroom_history(&self, id: i64) -> Result<Vec<HistoryStep>> {
        let mut q = self.db.prepare(
            "SELECT name, created, text FROM lightroom_history WHERE photo=? ORDER BY position",
        )?;
        let rows = q
            .query_map([id], |r| {
                let text = match r.get_ref(2)? {
                    rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => {
                        t.to_vec()
                    }
                    _ => Vec::new(),
                };
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<f64>>(1)?, text))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows
            .into_iter()
            .filter_map(|(name, created, bytes)| {
                Some(HistoryStep {
                    name,
                    created,
                    text: decode_history_text(&bytes)?,
                })
            })
            .collect())
    }
    /// Catalogs imported before history was kept still hold the original
    /// Lightroom catalog; copy its history steps once. Returns steps added.
    pub fn backfill_lightroom_history(&mut self) -> Result<usize> {
        let have: i64 = self
            .db
            .query_row("SELECT count(*) FROM lightroom_history", [], |r| r.get(0))?;
        if have > 0 {
            return Ok(0);
        }
        let original: Option<Vec<u8>> = self
            .db
            .query_row(
                "SELECT original_catalog FROM sources WHERE original_catalog IS NOT NULL LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let Some(original) = original else {
            return Ok(0);
        };
        let snapshot = tempfile::NamedTempFile::new()?;
        std::fs::write(snapshot.path(), original)?;
        self.db.execute(
            "ATTACH DATABASE ? AS lr",
            [snapshot.path().to_string_lossy()],
        )?;
        let result = (|| -> Result<usize> {
            let exists = self
                .db
                .query_row(
                    "SELECT 1 FROM lr.sqlite_master WHERE type='table' AND name='Adobe_libraryImageDevelopHistoryStep'",
                    [],
                    |r| r.get::<_, i32>(0),
                )
                .optional()?
                .is_some();
            if !exists {
                return Ok(0);
            }
            Ok(self.db.execute(COPY_LIGHTROOM_HISTORY, [])?)
        })();
        self.db.execute_batch("DETACH DATABASE lr")?;
        result
    }
    pub fn lightroom_develop(&self, id: i64) -> Result<Option<String>> {
        Ok(self.db.query_row(
            "SELECT lightroom_develop FROM photos WHERE id=?",
            [id],
            |r| r.get(0),
        )?)
    }
    pub fn add_folder(&mut self, folder: &Path) -> Result<usize> {
        let folder = folder.canonicalize()?;
        let mut files = Vec::new();
        fn walk(p: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
            for entry in std::fs::read_dir(p)? {
                let e = entry?;
                let t = e.file_type()?;
                if t.is_symlink() || crate::storage::is_hidden(&e.path()) {
                    continue;
                }
                if t.is_dir() {
                    walk(&e.path(), files)?
                } else if crate::storage::is_raw(&e.path())
                    || e.path().extension().is_some_and(|x| {
                        matches!(
                            x.to_string_lossy().to_ascii_lowercase().as_str(),
                            "jpg" | "jpeg" | "png" | "tif" | "tiff"
                        )
                    })
                {
                    files.push(e.path());
                }
            }
            Ok(())
        }
        walk(&folder, &mut files)?;
        let existing_paths: std::collections::HashSet<_> =
            self.photos()?.into_iter().map(|p| p.path).collect();
        let tx = self.db.transaction()?;
        let root: i64 = tx
            .query_row(
                "SELECT id FROM roots WHERE original_path=?",
                [folder.to_string_lossy()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let root = if root == 0 {
            tx.execute(
                "INSERT INTO roots(original_path) VALUES(?)",
                [folder.to_string_lossy()],
            )?;
            tx.last_insert_rowid()
        } else {
            root
        };
        let mut count = 0;
        for file in files {
            if existing_paths.contains(&file) {
                continue;
            }
            if tx
                .query_row(
                    "SELECT 1 FROM photos WHERE original_path=?",
                    [file.to_string_lossy()],
                    |r| r.get::<_, i32>(0),
                )
                .optional()?
                .is_some()
            {
                continue;
            }
            let relative = file
                .parent()
                .unwrap()
                .strip_prefix(&folder)?
                .to_string_lossy();
            let existing = tx
                .query_row(
                    "SELECT id FROM folders WHERE root=? AND relative_path=?",
                    params![root, relative],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?;
            let fid = if let Some(id) = existing {
                id
            } else {
                tx.execute(
                    "INSERT INTO folders(root,relative_path) VALUES(?,?)",
                    params![root, relative],
                )?;
                tx.last_insert_rowid()
            };
            tx.execute(
                "INSERT INTO photos(folder,filename,original_path,format) VALUES(?,?,?,?)",
                params![
                    fid,
                    file.file_name().unwrap().to_string_lossy(),
                    file.to_string_lossy(),
                    file.extension()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_ascii_uppercase()
                ],
            )?;
            count += 1;
        }
        tx.commit()?;
        Ok(count)
    }
}

pub mod preview_cache;
#[cfg(test)]
mod private_tests;
#[cfg(test)]
mod tests;
