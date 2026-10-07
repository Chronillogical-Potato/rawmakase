//! Disposable Library previews, separate from user catalogs and edit recipes.
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const APP_ID: i64 = 0x4f4d5052;
const VERSION: i64 = 1;
// 2: previews keep their aspect ratio (generation 1 forced 360×240).
const GENERATION: i64 = 2;
const LIMIT: i64 = 512 * 1024 * 1024;
pub use crate::storage::Stamp;
pub struct PreviewCache {
    db: Connection,
    writes: u32,
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
fn tagged(path: &Path, tag: &str) -> String {
    if tag.is_empty() {
        key(path)
    } else {
        format!("{}#{tag}", key(path))
    }
}
impl PreviewCache {
    pub fn path() -> PathBuf {
        crate::storage::data_dir().join("previews.sqlite3")
    }
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_millis(250))?;
        let app: i64 = db.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if app == 0 && version == 0 {
            let tables: i64 = db.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )?;
            ensure!(
                tables == 0,
                "Preview cache path contains a different database"
            );
            db.execute_batch(&format!("PRAGMA auto_vacuum=INCREMENTAL; PRAGMA application_id={APP_ID}; PRAGMA user_version={VERSION};
                CREATE TABLE previews(source_path TEXT PRIMARY KEY,source_size INTEGER NOT NULL,modified_ns TEXT NOT NULL,prefix_hash TEXT NOT NULL,generation INTEGER NOT NULL,width INTEGER NOT NULL,height INTEGER NOT NULL,jpeg BLOB NOT NULL,last_used INTEGER NOT NULL);
                CREATE INDEX previews_last_used ON previews(last_used);"))?;
        } else {
            ensure!(
                app == APP_ID && version == VERSION,
                "Unsupported preview cache format"
            );
        }
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        let mut cache = Self { db, writes: 0 };
        cache.prune(LIMIT)?;
        Ok(cache)
    }
    pub fn load(&self, path: &Path) -> Result<Option<image::RgbImage>> {
        self.load_tagged(path, "")
    }
    /// A preview variant, e.g. rendered with an edit identified by `tag`.
    pub fn load_tagged(&self, path: &Path, tag: &str) -> Result<Option<image::RgbImage>> {
        let key = tagged(path, tag);
        let row = self
            .db
            .query_row(
                "SELECT source_size,modified_ns,generation,jpeg FROM previews WHERE source_path=?",
                [&key],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Vec<u8>>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((size, modified, generation, jpeg)) = row else {
            return Ok(None);
        };
        // An offline original keeps its preview, as in Lightroom.
        let stale = Stamp::read(path).is_ok_and(|stamp| {
            stamp.size as i64 != size || stamp.modified_ns.to_string() != modified
        });
        // Maintenance is best effort, as another connection may hold the database, and
        // never changes the answer: a failed delete still misses, a failed touch hits.
        let forget = || {
            let _ = self
                .db
                .execute("DELETE FROM previews WHERE source_path=?", [&key]);
        };
        if generation != GENERATION || stale {
            forget();
            return Ok(None);
        }
        let image = match image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg) {
            Ok(im) if im.width() <= 1024 && im.height() <= 1024 => im.to_rgb8(),
            _ => {
                forget();
                return Ok(None);
            }
        };
        let _ = self.db.execute(
            "UPDATE previews SET last_used=? WHERE source_path=?",
            params![now(), &key],
        );
        Ok(Some(image))
    }
    pub fn store(&mut self, path: &Path, stamp: &Stamp, image: &image::RgbImage) -> Result<()> {
        self.store_tagged(path, "", stamp, image)
    }
    pub fn store_tagged(
        &mut self,
        path: &Path,
        tag: &str,
        stamp: &Stamp,
        image: &image::RgbImage,
    ) -> Result<()> {
        ensure!(
            image.width() <= 1024 && image.height() <= 1024,
            "Preview exceeds cache size limit"
        );
        ensure!(
            Stamp::read(path)? == *stamp,
            "Source changed while generating preview"
        );
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 88).encode(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )?;
        // prefix_hash, which needed the file's first 64 KB, is no longer checked.
        self.db.execute("INSERT INTO previews VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(source_path) DO UPDATE SET source_size=excluded.source_size,modified_ns=excluded.modified_ns,prefix_hash=excluded.prefix_hash,generation=excluded.generation,width=excluded.width,height=excluded.height,jpeg=excluded.jpeg,last_used=excluded.last_used",params![tagged(path, tag),stamp.size as i64,stamp.modified_ns.to_string(),"",GENERATION,image.width(),image.height(),jpeg,now()])?;
        self.writes += 1;
        if self.writes.is_multiple_of(32) {
            self.prune(LIMIT)?;
        }
        Ok(())
    }
    fn prune(&mut self, budget: i64) -> Result<()> {
        loop {
            let bytes: i64 = self.db.query_row(
                "SELECT COALESCE(sum(length(jpeg)),0) FROM previews",
                [],
                |r| r.get(0),
            )?;
            if bytes <= budget {
                break;
            }
            self.db.execute("DELETE FROM previews WHERE source_path IN (SELECT source_path FROM previews ORDER BY last_used,source_path LIMIT 32)",[])?;
        }
        self.db.execute_batch("PRAGMA incremental_vacuum(256);")?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistent_hits_offline_previews_and_source_invalidation() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let db = d.path().join("previews.sqlite3");
        let mut cache = PreviewCache::open(&db)?;
        let im = image::RgbImage::from_pixel(24, 16, image::Rgb([120, 70, 40]));
        cache.store(&raw, &Stamp::read(&raw)?, &im)?;
        drop(cache);
        let cache = PreviewCache::open(&db)?;
        assert_eq!(cache.load(&raw)?.unwrap().dimensions(), (24, 16));
        std::fs::remove_file(&raw)?;
        assert!(cache.load(&raw)?.is_some());
        std::fs::write(&raw, b"replacement raw")?;
        assert!(cache.load(&raw)?.is_none());
        Ok(())
    }
    #[test]
    fn same_size_rewrite_is_stale_by_its_modification_time() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        let im = image::RgbImage::new(8, 8);
        cache.store(&raw, &Stamp::read(&raw)?, &im)?;
        let modified = std::fs::metadata(&raw)?.modified()?;
        std::fs::write(&raw, b"replaced raw")?;
        std::fs::File::options()
            .write(true)
            .open(&raw)?
            .set_modified(modified + Duration::from_secs(1))?;
        assert!(cache.load(&raw)?.is_none());
        Ok(())
    }
    /// On a network share every read of a photo is a round trip, so a cached
    /// preview must come back without opening its original.
    #[cfg(unix)]
    #[test]
    fn cached_previews_never_open_the_original() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        let im = image::RgbImage::new(8, 8);
        cache.store(&raw, &Stamp::read(&raw)?, &im)?;
        cache.store_tagged(&raw, "edit-1", &Stamp::read(&raw)?, &im)?;
        std::fs::set_permissions(&raw, std::fs::Permissions::from_mode(0o000))?;
        // Unless running as root, which ignores permissions.
        if std::fs::File::open(&raw).is_err() {
            assert!(cache.load(&raw)?.is_some());
            assert!(cache.load_tagged(&raw, "edit-1")?.is_some());
        }
        Ok(())
    }
    #[test]
    fn edited_previews_are_kept_apart_from_the_embedded_one() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        let stamp = Stamp::read(&raw)?;
        let plain = image::RgbImage::from_pixel(24, 16, image::Rgb([120, 70, 40]));
        let edited = image::RgbImage::from_pixel(16, 16, image::Rgb([20, 70, 140]));
        cache.store(&raw, &stamp, &plain)?;
        cache.store_tagged(&raw, "edit-1", &stamp, &edited)?;
        assert_eq!(cache.load(&raw)?.unwrap().dimensions(), (24, 16));
        assert_eq!(
            cache.load_tagged(&raw, "edit-1")?.unwrap().dimensions(),
            (16, 16)
        );
        assert!(cache.load_tagged(&raw, "edit-2")?.is_none());
        Ok(())
    }
    #[test]
    fn reads_survive_failed_maintenance_writes() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let db = d.path().join("previews.sqlite3");
        let mut cache = PreviewCache::open(&db)?;
        let im = image::RgbImage::from_pixel(24, 16, image::Rgb([120, 70, 40]));
        cache.store(&raw, &Stamp::read(&raw)?, &im)?;
        // Another connection holds the write lock: no touch or delete can happen.
        let other = Connection::open(&db)?;
        other.execute_batch("BEGIN IMMEDIATE")?;
        assert!(cache.load(&raw)?.is_some());
        std::fs::write(&raw, b"replacement raw")?;
        assert!(cache.load(&raw)?.is_none());
        other.execute_batch("COMMIT")?;
        assert!(cache.load(&raw)?.is_none());
        Ok(())
    }
    #[test]
    fn broken_entries_budget_and_unrelated_database_are_safe() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.RAF");
        std::fs::write(&raw, b"fixture")?;
        let mut cache = PreviewCache::open(&d.path().join("cache.db"))?;
        let im = image::RgbImage::new(8, 8);
        cache.store(&raw, &Stamp::read(&raw)?, &im)?;
        cache.db.execute("UPDATE previews SET jpeg=X'001122'", [])?;
        assert!(cache.load(&raw)?.is_none());
        cache.store(&raw, &Stamp::read(&raw)?, &im)?;
        cache.prune(0)?;
        assert!(cache.load(&raw)?.is_none());
        let other = d.path().join("other.db");
        let db = Connection::open(&other)?;
        db.execute("CREATE TABLE precious(value TEXT)", [])?;
        drop(db);
        assert!(PreviewCache::open(&other).is_err());
        assert!(
            Connection::open(&other)?
                .prepare("SELECT * FROM precious")
                .is_ok()
        );
        Ok(())
    }
}
