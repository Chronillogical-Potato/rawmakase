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

/// Set in `meta` once keyword export options have been copied from the
/// stored catalog.
const KEYWORD_EXPORT_BACKFILLED: &str = "lightroom_keyword_export_backfilled";

/// A photo's filename in a Lightroom catalog, from its `AgLibraryFile` row
/// named `f`.
pub(super) const LIGHTROOM_FILENAME: &str =
    "CASE WHEN f.idx_filename<>'' THEN f.idx_filename ELSE f.baseName||'.'||f.extension END";

impl Catalog {
    /// Catalogs imported before keyword export options were kept still hold
    /// the original Lightroom catalog; copy them once, so an export leaves
    /// out the keywords Lightroom would.
    pub fn backfill_keyword_export(&mut self) -> Result<usize> {
        self.backfill_once(KEYWORD_EXPORT_BACKFILLED, copy_keyword_export)
    }
    /// Runs `copy` once per catalog, as recorded under `key` in `meta`, with
    /// the Lightroom catalog this one was imported from attached as `lr`.
    /// Returns what it copied; nothing for a catalog that was not imported.
    pub(in crate::catalog) fn backfill_once(
        &mut self,
        key: &str,
        copy: fn(&Connection) -> Result<usize>,
    ) -> Result<usize> {
        if self.meta(key)?.is_some() {
            return Ok(0);
        }
        // One transaction: a row at a time would flush the journal for each.
        let copied = self
            .with_stored_lightroom(|db| {
                let tx = db.unchecked_transaction()?;
                let copied = copy(&tx)?;
                tx.commit()?;
                Ok(copied)
            })?
            .unwrap_or(0);
        self.set_meta(key, "1")?;
        Ok(copied)
    }
    /// Runs `f` with the Lightroom catalog this one was imported from
    /// attached as `lr`; `None` for a catalog that was not imported.
    fn with_stored_lightroom<T>(
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

/// Whether the database attached as `schema` has table `name`.
pub(super) fn has_table(db: &Connection, schema: &str, name: &str) -> Result<bool> {
    Ok(db
        .query_row(
            &format!("SELECT 1 FROM {schema}.sqlite_master WHERE type='table' AND name=?"),
            [name],
            |r| r.get::<_, i32>(0),
        )
        .optional()?
        .is_some())
}

/// Copies Lightroom's Include on Export and Export Containing Keywords of
/// the keywords that have either off, from a catalog attached as `lr`.
pub(super) fn copy_keyword_export(db: &Connection) -> Result<usize> {
    let columns: Vec<String> = db
        .prepare("SELECT name FROM pragma_table_info('AgLibraryKeyword', 'lr')")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let has = |c: &str| columns.iter().any(|n| n == c);
    if !has("includeOnExport") || !has("includeParents") {
        return Ok(0);
    }
    Ok(db.execute(
        "INSERT OR REPLACE INTO keyword_export(keyword, include, parents)
         SELECT id_local, COALESCE(includeOnExport, 1) <> 0, COALESCE(includeParents, 1) <> 0
         FROM lr.AgLibraryKeyword
         WHERE (includeOnExport = 0 OR includeParents = 0)
           AND id_local IN (SELECT id FROM keywords)",
        [],
    )?)
}

/// Import a closed/exported Lightroom catalog into a new, atomically published file.
/// Keep a byte-exact archive inside our catalog, including fields we cannot interpret.
pub fn import_lightroom(source: &Path, destination: &Path) -> Result<PathBuf> {
    ensure!(
        !destination.exists(),
        "Destination exists; choose a new catalog filename"
    );
    ensure_no_live_journal(
        source,
        "Lightroom catalog has a live journal. Close Lightroom and copy/export the catalog with its companion files first",
    )?;
    let (snapshot, size) = take_snapshot(source)?;
    verify_snapshot(snapshot.path())?;
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
            size as i64,
            std::fs::read(snapshot.path())?
        ],
    )?;
    copy_tables(&tx)?;
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

/// Fails with `message` while Lightroom has `source` open: its WAL or
/// rollback journal is not empty.
fn ensure_no_live_journal(source: &Path, message: &str) -> Result<()> {
    for suffix in ["-wal", "-journal"] {
        let p = PathBuf::from(format!("{}{suffix}", source.display()));
        ensure!(!p.exists() || p.metadata()?.len() == 0, "{message}");
    }
    Ok(())
}

/// A copy of `source`, refused if it changed while being copied, and its size.
fn take_snapshot(source: &Path) -> Result<(tempfile::NamedTempFile, u64)> {
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
    ensure_no_live_journal(
        source,
        "Source catalog became active during import; retry after closing Lightroom",
    )?;
    Ok((snapshot, before.len()))
}

/// Checks the snapshot is an intact Lightroom catalog with the tables an
/// import needs.
fn verify_snapshot(snapshot: &Path) -> Result<()> {
    let db = Connection::open_with_flags(snapshot, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    ensure!(
        db.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0))? == "ok",
        "Source catalog failed SQLite integrity check"
    );
    for table in [
        "Adobe_images",
        "AgLibraryFile",
        "AgLibraryFolder",
        "AgLibraryRootFolder",
    ] {
        ensure!(
            has_table(&db, "main", table)?,
            "Not a supported Lightroom catalog: missing {table}"
        );
    }
    Ok(())
}

/// Copies photos, folders, history, collections, keywords, info and
/// descriptive metadata from the Lightroom catalog attached as `lr`, in the
/// caller's transaction. Fails if any image was left out.
fn copy_tables(tx: &Connection) -> Result<()> {
    tx.execute_batch(&format!("INSERT INTO roots(id,original_path) SELECT id_local,absolutePath FROM lr.AgLibraryRootFolder;
    INSERT INTO folders(id,root,relative_path) SELECT id_local,rootFolder,pathFromRoot FROM lr.AgLibraryFolder;
    INSERT INTO photos(id,folder,filename,original_path,captured,rating,flag,label,format,copy_name,master_id,orientation)
    SELECT i.id_local,f.folder,{LIGHTROOM_FILENAME},
    r.absolutePath||d.pathFromRoot||{LIGHTROOM_FILENAME},
    COALESCE(i.captureTime,''),COALESCE(i.rating,0),COALESCE(i.pick,0),COALESCE(i.colorLabels,''),COALESCE(i.fileFormat,''),COALESCE(i.copyName,''),i.masterImage,i.orientation
    FROM lr.Adobe_images i JOIN lr.AgLibraryFile f ON f.id_local=i.rootFile JOIN lr.AgLibraryFolder d ON d.id_local=f.folder JOIN lr.AgLibraryRootFolder r ON r.id_local=d.rootFolder;"))?;
    let has = |name: &str| has_table(tx, "lr", name);
    if has("Adobe_imageDevelopSettings")? {
        tx.execute_batch("UPDATE photos SET lightroom_develop=(SELECT text FROM lr.Adobe_imageDevelopSettings WHERE image=photos.id LIMIT 1);")?;
    }
    if has("Adobe_libraryImageDevelopHistoryStep")? {
        tx.execute_batch(history::COPY_LIGHTROOM_HISTORY)?;
    }
    if has("Adobe_libraryImageDevelopSnapshot")? {
        tx.execute_batch(crate::catalog::snapshots::COPY_LIGHTROOM_SNAPSHOTS)?;
    }
    if has("AgLibraryCollection")? {
        tx.execute_batch("INSERT INTO collections SELECT id_local,name,parent,creationId FROM lr.AgLibraryCollection;")?;
    }
    if has("AgLibraryCollectionImage")? {
        tx.execute_batch("INSERT OR IGNORE INTO collection_photos SELECT collection,image,positionInCollection FROM lr.AgLibraryCollectionImage WHERE collection IN(SELECT id FROM collections) AND image IN(SELECT id FROM photos);")?;
    }
    if has("AgLibraryKeyword")? {
        tx.execute_batch("INSERT INTO keywords SELECT id_local,COALESCE(name,''),parent FROM lr.AgLibraryKeyword;")?;
        copy_keyword_export(tx)?;
    }
    super::info::copy_lightroom_info(tx)?;
    super::sidecar::copy_lightroom_metadata(tx)?;
    // Copied here, so opening the new catalog has nothing to backfill.
    for key in [
        super::info::INFO_BACKFILLED,
        super::sidecar::METADATA_BACKFILLED,
        KEYWORD_EXPORT_BACKFILLED,
    ] {
        super::set_meta(tx, key, "1")?;
    }
    if has("AgLibraryKeywordImage")? {
        tx.execute_batch("INSERT OR IGNORE INTO photo_keywords SELECT image,tag FROM lr.AgLibraryKeywordImage WHERE image IN(SELECT id FROM photos) AND tag IN(SELECT id FROM keywords);")?;
    }
    let imported: i64 = tx.query_row("SELECT count(*) FROM photos", [], |r| r.get(0))?;
    let expected: i64 = tx.query_row("SELECT count(*) FROM lr.Adobe_images", [], |r| r.get(0))?;
    ensure!(
        imported == expected,
        "Catalog has orphaned image records ({imported}/{expected}); import rolled back"
    );
    Ok(())
}
