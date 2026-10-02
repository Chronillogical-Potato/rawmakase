//! Read-only Lightroom catalog import and best-effort Develop conversion.
mod develop;
pub(super) mod history;
use super::Catalog;
use anyhow::{Context, Result, ensure};
pub use develop::convert_develop;
#[cfg(test)]
pub(super) use develop::develop_fields;
pub use history::HistoryStep;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::{Path, PathBuf};

impl Catalog {
    /// Runs `f` with the Lightroom catalog this one was imported from
    /// attached as `lr`; `None` for a catalog that was not imported.
    pub(in crate::catalog) fn with_stored_lightroom<T>(
        &mut self,
        f: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<Option<T>> {
        let original: Option<Vec<u8>> = self
            .db
            .query_row(
                "SELECT original_catalog FROM sources WHERE original_catalog IS NOT NULL LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let Some(original) = original else {
            return Ok(None);
        };
        let snapshot = tempfile::NamedTempFile::new()?;
        std::fs::write(snapshot.path(), original)?;
        self.db.execute(
            "ATTACH DATABASE ? AS lr",
            [snapshot.path().to_string_lossy()],
        )?;
        let result = f(&self.db);
        self.db.execute_batch("DETACH DATABASE lr")?;
        result.map(Some)
    }
}

/// Import a closed/exported Lightroom catalog into a new, atomically published file.
/// Keep a byte-exact archive inside our catalog, including fields we cannot interpret.
pub fn import_lightroom(source: &Path, destination: &Path) -> Result<PathBuf> {
    ensure!(
        !destination.exists(),
        "Destination exists; choose a new catalog filename"
    );
    for suffix in ["-wal", "-journal"] {
        let p = PathBuf::from(format!("{}{suffix}", source.display()));
        ensure!(
            !p.exists() || p.metadata()?.len() == 0,
            "Lightroom catalog has a live journal. Close Lightroom and copy/export the catalog with its companion files first"
        );
    }
    let before = source.metadata()?;
    ensure!(
        before.len() < 2_000_000_000,
        "Catalog is too large for this importer"
    );
    let snapshot = tempfile::NamedTempFile::new()?;
    std::fs::copy(source, snapshot.path())?;
    let after = source.metadata()?;
    ensure!(
        before.len() == after.len() && before.modified()? == after.modified()?,
        "Source catalog changed during import; retry after closing Lightroom"
    );
    for suffix in ["-wal", "-journal"] {
        let p = PathBuf::from(format!("{}{suffix}", source.display()));
        ensure!(
            !p.exists() || p.metadata()?.len() == 0,
            "Source catalog became active during import; retry after closing Lightroom"
        );
    }
    let source_db = Connection::open_with_flags(snapshot.path(), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    ensure!(
        source_db.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))? == "ok",
        "Source catalog failed SQLite integrity check"
    );
    for table in [
        "Adobe_images",
        "AgLibraryFile",
        "AgLibraryFolder",
        "AgLibraryRootFolder",
    ] {
        ensure!(
            source_db
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
                    [table],
                    |r| r.get::<_, i32>(0)
                )
                .optional()?
                .is_some(),
            "Not a supported Lightroom catalog: missing {table}"
        );
    }
    drop(source_db);
    let parent = crate::storage::parent_dir(destination);
    std::fs::create_dir_all(parent)?;
    let tmpdir = tempfile::tempdir_in(parent)?;
    let working = tmpdir.path().join("import.rawmakase");
    let mut catalog = Catalog::create(&working)?;
    catalog.db.execute(
        "ATTACH DATABASE ? AS lr",
        [snapshot.path().to_string_lossy()],
    )?;
    let tx = catalog.db.transaction()?;
    tx.execute(
        "INSERT INTO sources(path,original_size,original_catalog) VALUES(?,?,?)",
        params![
            source.to_string_lossy(),
            before.len() as i64,
            std::fs::read(snapshot.path())?
        ],
    )?;
    tx.execute_batch("INSERT INTO roots(id,original_path) SELECT id_local,absolutePath FROM lr.AgLibraryRootFolder;
    INSERT INTO folders(id,root,relative_path) SELECT id_local,rootFolder,pathFromRoot FROM lr.AgLibraryFolder;
    INSERT INTO photos(id,folder,filename,original_path,captured,rating,flag,label,format,copy_name,master_id,orientation)
    SELECT i.id_local,f.folder,CASE WHEN f.idx_filename<>'' THEN f.idx_filename ELSE f.baseName||'.'||f.extension END,
    r.absolutePath||d.pathFromRoot||CASE WHEN f.idx_filename<>'' THEN f.idx_filename ELSE f.baseName||'.'||f.extension END,
    COALESCE(i.captureTime,''),COALESCE(i.rating,0),COALESCE(i.pick,0),COALESCE(i.colorLabels,''),COALESCE(i.fileFormat,''),COALESCE(i.copyName,''),i.masterImage,i.orientation
    FROM lr.Adobe_images i JOIN lr.AgLibraryFile f ON f.id_local=i.rootFile JOIN lr.AgLibraryFolder d ON d.id_local=f.folder JOIN lr.AgLibraryRootFolder r ON r.id_local=d.rootFolder;")?;
    let has = |name: &str| -> Result<bool> {
        Ok(tx
            .query_row(
                "SELECT 1 FROM lr.sqlite_master WHERE type='table' AND name=?",
                [name],
                |r| r.get::<_, i32>(0),
            )
            .optional()?
            .is_some())
    };
    if has("Adobe_imageDevelopSettings")? {
        tx.execute_batch("UPDATE photos SET lightroom_develop=(SELECT text FROM lr.Adobe_imageDevelopSettings WHERE image=photos.id LIMIT 1);")?;
    }
    if has("Adobe_libraryImageDevelopHistoryStep")? {
        tx.execute_batch(history::COPY_LIGHTROOM_HISTORY)?;
    }
    if has("AgLibraryCollection")? {
        tx.execute_batch("INSERT INTO collections SELECT id_local,name,parent,creationId FROM lr.AgLibraryCollection;")?;
    }
    if has("AgLibraryCollectionImage")? {
        tx.execute_batch("INSERT OR IGNORE INTO collection_photos SELECT collection,image,positionInCollection FROM lr.AgLibraryCollectionImage WHERE collection IN(SELECT id FROM collections) AND image IN(SELECT id FROM photos);")?;
    }
    if has("AgLibraryKeyword")? {
        tx.execute_batch("INSERT INTO keywords SELECT id_local,COALESCE(name,''),parent FROM lr.AgLibraryKeyword;")?;
    }
    super::info::copy_lightroom_info(&tx)?;
    super::sidecar::copy_lightroom_metadata(&tx)?;
    // Copied here, so opening the new catalog has nothing to backfill.
    tx.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES (?, '1')",
        [super::info::INFO_BACKFILLED],
    )?;
    tx.execute(
        "INSERT OR REPLACE INTO meta(key, value) VALUES (?, '1')",
        [super::sidecar::METADATA_BACKFILLED],
    )?;
    if has("AgLibraryKeywordImage")? {
        tx.execute_batch("INSERT OR IGNORE INTO photo_keywords SELECT image,tag FROM lr.AgLibraryKeywordImage WHERE image IN(SELECT id FROM photos) AND tag IN(SELECT id FROM keywords);")?;
    }
    let imported: i64 = tx.query_row("SELECT count(*) FROM photos", [], |r| r.get(0))?;
    let expected: i64 = tx.query_row("SELECT count(*) FROM lr.Adobe_images", [], |r| r.get(0))?;
    ensure!(
        imported == expected,
        "Catalog has orphaned image records ({imported}/{expected}); import rolled back"
    );
    tx.commit()?;
    catalog.db.execute_batch("DETACH DATABASE lr")?;
    ensure!(
        catalog
            .db
            .query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))?
            == "ok",
        "Imported catalog failed integrity check"
    );
    drop(catalog);
    // hard_link gives atomic no-clobber publication on the destination filesystem.
    std::fs::hard_link(&working, destination)
        .context("Publish imported catalog without overwriting")?;
    Ok(destination.into())
}
