//! Photo info: camera settings and size, copied from the Lightroom catalog a
//! photo was imported from, or read from the file for photos added from
//! folders (`fill_photo_info`).
use super::Catalog;
use crate::metadata::PhotoInfo;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

/// Set in `meta` once photo info has been copied from the stored catalog.
pub(super) const INFO_BACKFILLED: &str = "lightroom_info_backfilled";

impl Catalog {
    /// Each photo's width over height, as shown, where it is known; a
    /// virtual copy has its master's.
    pub fn aspect_ratios(&self) -> Result<std::collections::HashMap<i64, f32>> {
        let mut query = self.db.prepare(
            "SELECT p.id, i.width, i.height FROM photos p
             JOIN photo_info i ON i.photo = COALESCE(p.master_id, p.id)
             WHERE i.width > 0 AND i.height > 0",
        )?;
        let rows = query.query_map([], |r| {
            let (width, height): (f64, f64) = (r.get(1)?, r.get(2)?);
            Ok((r.get(0)?, (width / height) as f32))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    /// A photo's info; a virtual copy has its master's.
    pub fn photo_info(&self, id: i64) -> Result<Option<PhotoInfo>> {
        Ok(self
            .db
            .query_row(
                "SELECT camera, lens, focal, aperture, exposure, iso, width, height
                 FROM photo_info
                 WHERE photo = (SELECT COALESCE(master_id, id) FROM photos WHERE id = ?)",
                [id],
                |r| {
                    let size: (Option<u32>, Option<u32>) = (r.get(6)?, r.get(7)?);
                    Ok(PhotoInfo {
                        camera: r.get(0)?,
                        lens: r.get(1)?,
                        focal: r.get(2)?,
                        aperture: r.get(3)?,
                        exposure: r.get(4)?,
                        iso: r.get(5)?,
                        dimensions: size.0.zip(size.1),
                    })
                },
            )
            .optional()?)
    }
    /// The cameras the catalog's photos were taken with, as photo info names
    /// them, in alphabetical order.
    pub fn cameras(&self) -> Result<Vec<String>> {
        Ok(self
            .db
            .prepare(
                "SELECT DISTINCT camera FROM photo_info
                 WHERE camera IS NOT NULL AND camera != ''
                 ORDER BY camera COLLATE NOCASE",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    /// The cameras the catalog's RAW photos were taken with, leaving out
    /// cameras seen only in JPEGs, TIFFs and videos.
    pub fn raw_cameras(&self) -> Result<Vec<String>> {
        let mut query = self.db.prepare(
            "SELECT i.camera, p.filename FROM photo_info i
             JOIN photos p ON p.id = i.photo
             WHERE i.camera IS NOT NULL AND i.camera != ''",
        )?;
        let mut cameras = std::collections::BTreeSet::new();
        for row in query.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
            let (camera, filename) = row?;
            if crate::storage::is_raw(std::path::Path::new(&filename)) {
                cameras.insert(camera);
            }
        }
        Ok(cameras.into_iter().collect())
    }
    /// Masters with no info yet, whose files may have it.
    pub fn photos_without_info(&self) -> Result<Vec<i64>> {
        Ok(self
            .db
            .prepare(
                "SELECT id FROM photos
                 WHERE master_id IS NULL AND id NOT IN (SELECT photo FROM photo_info)",
            )?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    }
    /// Records info read from files, in one transaction; `None` records that
    /// a file had none, so it is not read again.
    pub fn fill_photo_info(&mut self, infos: &[(i64, Option<PhotoInfo>)]) -> Result<()> {
        let tx = self.db.transaction()?;
        for (id, info) in infos {
            // Under the photo's master now, in case it became a copy while
            // being read.
            // A photo removed meanwhile has nothing to keep.
            let master: Option<i64> = tx
                .query_row(
                    "SELECT COALESCE(master_id, id) FROM photos WHERE id = ?",
                    [id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(master) = master {
                insert(&tx, master, info.as_ref().unwrap_or(&PhotoInfo::default()))?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Catalogs imported before photo info was kept still hold the original
    /// Lightroom catalog; copy its info once.
    pub fn backfill_lightroom_info(&mut self) -> Result<usize> {
        self.backfill_once(INFO_BACKFILLED, copy_lightroom_info)
    }
}

fn insert(db: &Connection, id: i64, info: &PhotoInfo) -> Result<()> {
    db.execute(
        "INSERT OR REPLACE INTO photo_info
         (photo, camera, lens, focal, aperture, exposure, iso, width, height)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            id,
            info.camera,
            info.lens,
            info.focal,
            info.aperture,
            info.exposure,
            info.iso,
            info.dimensions.map(|d| d.0),
            info.dimensions.map(|d| d.1),
        ],
    )?;
    Ok(())
}

/// Copies photo info from a Lightroom catalog attached as `lr`. Lightroom
/// stores aperture and shutter speed as APEX values: aperture 2.0 is f/2,
/// shutter speed 4.64 is 1/25 s. Returns the photos copied.
pub(super) fn copy_lightroom_info(db: &Connection) -> Result<usize> {
    let has = |table: &str| super::lightroom::has_table(db, "lr", table);
    let interned = has("AgInternedExifCameraModel")? && has("AgInternedExifLens")?;
    if !has("AgHarvestedExifMetadata")? || !interned {
        return Ok(0);
    }
    let rows: Vec<(i64, PhotoInfo)> = db
        .prepare(
            "SELECT i.id_local, c.value, l.value, e.focalLength, e.aperture, e.shutterSpeed,
                    e.isoSpeedRating, i.fileWidth, i.fileHeight, i.orientation
             FROM lr.Adobe_images i
             LEFT JOIN lr.AgHarvestedExifMetadata e ON e.image = i.id_local
             LEFT JOIN lr.AgInternedExifCameraModel c ON c.id_local = e.cameraModelRef
             LEFT JOIN lr.AgInternedExifLens l ON l.id_local = e.lensRef
             WHERE i.id_local IN (SELECT id FROM photos)",
        )?
        .query_map([], |r| {
            let (width, height): (Option<f64>, Option<f64>) = (r.get(7)?, r.get(8)?);
            let orientation: Option<String> = r.get(9)?;
            // Quarter turns, mirrored or not: the photo shows taller than stored.
            let turned = matches!(orientation.as_deref(), Some("BC" | "DA" | "AD" | "CB"));
            let dimensions = width.zip(height).map(|(w, h)| {
                let (w, h) = (w as u32, h as u32);
                if turned { (h, w) } else { (w, h) }
            });
            Ok((
                r.get(0)?,
                PhotoInfo {
                    camera: r.get(1)?,
                    lens: r.get(2)?,
                    focal: r.get(3)?,
                    aperture: r.get::<_, Option<f64>>(4)?.map(|av| 2f64.powf(av / 2.)),
                    exposure: r.get::<_, Option<f64>>(5)?.map(|tv| 2f64.powf(-tv)),
                    iso: r.get(6)?,
                    dimensions,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (id, info) in &rows {
        insert(db, *id, info)?;
    }
    Ok(rows.len())
}
