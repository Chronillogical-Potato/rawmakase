//! Photo info: camera settings and size, copied from the Lightroom catalog a
//! photo was imported from, or read from the file for photos added from
//! folders (`fill_photo_info`).
use super::db::{LightroomWrite, Reads, Write, sql, sqlite_sql};
use super::value::row;
use super::{Catalog, PhotoId};
use crate::metadata::PhotoInfo;
use anyhow::Result;

/// Set in `meta` once photo info has been copied from the stored catalog.
pub(super) const INFO_BACKFILLED: &str = "lightroom_info_backfilled";

impl Catalog {
    /// Each photo's width over height, as shown, where it is known; a
    /// virtual copy has its master's.
    pub fn aspect_ratios(&self) -> Result<std::collections::HashMap<PhotoId, f32>> {
        row! {
            struct Size {
                id: PhotoId,
                width: f64,
                height: f64,
            }
        }
        let sizes: Vec<Size> = self.db.read(
            sql!(
                "SELECT p.id, i.width, i.height FROM photos p
                 JOIN photo_info i ON i.photo = COALESCE(p.master_id, p.id)
                 WHERE i.width > 0 AND i.height > 0"
            ),
            &[],
        )?;
        Ok(sizes
            .into_iter()
            .map(|s| (s.id, (s.width / s.height) as f32))
            .collect())
    }
    /// A photo's info; a virtual copy has its master's.
    pub fn photo_info(&self, id: PhotoId) -> Result<Option<PhotoInfo>> {
        row! {
            struct Info {
                camera: Option<String>,
                lens: Option<String>,
                focal: Option<f64>,
                aperture: Option<f64>,
                exposure: Option<f64>,
                iso: Option<f64>,
                width: Option<u32>,
                height: Option<u32>,
            }
        }
        let info: Option<Info> = self.db.read_optional(
            sql!(
                "SELECT camera, lens, focal, aperture, exposure, iso, width, height
                 FROM photo_info
                 WHERE photo = (SELECT COALESCE(master_id, id) FROM photos WHERE id = ?)"
            ),
            &[&id],
        )?;
        Ok(info.map(|i| PhotoInfo {
            camera: i.camera,
            lens: i.lens,
            focal: i.focal,
            aperture: i.aperture,
            exposure: i.exposure,
            iso: i.iso,
            dimensions: i.width.zip(i.height),
        }))
    }
    /// The cameras the catalog's photos were taken with, as photo info names
    /// them, in alphabetical order.
    pub fn cameras(&self) -> Result<Vec<String>> {
        let mut cameras: Vec<String> = self.db.read(
            sql!(
                "SELECT DISTINCT camera FROM photo_info
                 WHERE camera IS NOT NULL AND camera != '' ORDER BY camera"
            ),
            &[],
        )?;
        // Case folded in ASCII only, as SQLite's NOCASE does.
        cameras.sort_by_cached_key(|camera| camera.to_ascii_lowercase());
        Ok(cameras)
    }
    /// The cameras the catalog's RAW photos were taken with, leaving out
    /// cameras seen only in JPEGs, TIFFs and videos.
    pub fn raw_cameras(&self) -> Result<Vec<String>> {
        row! {
            struct Taken {
                camera: String,
                filename: String,
            }
        }
        let taken: Vec<Taken> = self.db.read(
            sql!(
                "SELECT i.camera, p.filename FROM photo_info i
                 JOIN photos p ON p.id = i.photo
                 WHERE i.camera IS NOT NULL AND i.camera != ''"
            ),
            &[],
        )?;
        let cameras: std::collections::BTreeSet<String> = taken
            .into_iter()
            .filter(|t| crate::storage::is_raw(std::path::Path::new(&t.filename)))
            .map(|t| t.camera)
            .collect();
        Ok(cameras.into_iter().collect())
    }
    /// Masters with no info yet, whose files may have it.
    pub fn photos_without_info(&self) -> Result<Vec<PhotoId>> {
        self.db.read(
            sql!(
                "SELECT id FROM photos
                 WHERE master_id IS NULL AND id NOT IN (SELECT photo FROM photo_info)"
            ),
            &[],
        )
    }
    /// Records info read from files, in one transaction; `None` records that
    /// a file had none, so it is not read again.
    pub fn fill_photo_info(&mut self, infos: &[(PhotoId, Option<PhotoInfo>)]) -> Result<()> {
        self.db.write(|w| {
            for (id, info) in infos {
                // Under the photo's master now, in case it became a copy while
                // being read.
                // A photo removed meanwhile has nothing to keep.
                let master: Option<PhotoId> = w.read_optional(
                    sql!("SELECT COALESCE(master_id, id) FROM photos WHERE id = ?"),
                    &[id],
                )?;
                if let Some(master) = master {
                    insert(w, master, info.as_ref().unwrap_or(&PhotoInfo::default()))?;
                }
            }
            Ok(())
        })
    }
    /// Catalogs imported before photo info was kept still hold the original
    /// Lightroom catalog; copy its info once.
    pub fn backfill_lightroom_info(&mut self) -> Result<usize> {
        self.backfill_once(INFO_BACKFILLED, copy_lightroom_info)
    }
}

fn insert(w: &mut Write<'_>, id: PhotoId, info: &PhotoInfo) -> Result<()> {
    w.execute(
        sql!(
            "INSERT INTO photo_info
             (photo, camera, lens, focal, aperture, exposure, iso, width, height)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(photo) DO UPDATE SET camera=excluded.camera, lens=excluded.lens,
                 focal=excluded.focal, aperture=excluded.aperture, exposure=excluded.exposure,
                 iso=excluded.iso, width=excluded.width, height=excluded.height"
        ),
        &[
            &id,
            &info.camera,
            &info.lens,
            &info.focal,
            &info.aperture,
            &info.exposure,
            &info.iso,
            &info.dimensions.map(|d| d.0),
            &info.dimensions.map(|d| d.1),
        ],
    )?;
    Ok(())
}

/// Copies photo info from a Lightroom catalog attached as `lr`. Lightroom
/// stores aperture and shutter speed as APEX values: aperture 2.0 is f/2,
/// shutter speed 4.64 is 1/25 s. Returns the photos copied.
pub(super) fn copy_lightroom_info(lr: &mut LightroomWrite<'_>) -> Result<usize> {
    let interned =
        lr.has_table("AgInternedExifCameraModel")? && lr.has_table("AgInternedExifLens")?;
    if !lr.has_table("AgHarvestedExifMetadata")? || !interned {
        return Ok(0);
    }
    row! {
        struct Harvested {
            id: PhotoId,
            camera: Option<String>,
            lens: Option<String>,
            focal: Option<f64>,
            aperture: Option<f64>,
            shutter: Option<f64>,
            iso: Option<f64>,
            width: Option<f64>,
            height: Option<f64>,
            orientation: Option<String>,
        }
    }
    let rows: Vec<Harvested> = lr.read_sqlite(
        sqlite_sql!(
            "SELECT i.id_local, c.value, l.value, e.focalLength, e.aperture, e.shutterSpeed,
                    e.isoSpeedRating, i.fileWidth, i.fileHeight, i.orientation
             FROM lr.Adobe_images i
             LEFT JOIN lr.AgHarvestedExifMetadata e ON e.image = i.id_local
             LEFT JOIN lr.AgInternedExifCameraModel c ON c.id_local = e.cameraModelRef
             LEFT JOIN lr.AgInternedExifLens l ON l.id_local = e.lensRef
             WHERE i.id_local IN (SELECT id FROM photos)"
        ),
        &[],
    )?;
    let count = rows.len();
    for r in rows {
        // Quarter turns, mirrored or not: the photo shows taller than stored.
        let turned = matches!(r.orientation.as_deref(), Some("BC" | "DA" | "AD" | "CB"));
        let dimensions = r.width.zip(r.height).map(|(w, h)| {
            let (w, h) = (w as u32, h as u32);
            if turned { (h, w) } else { (w, h) }
        });
        let info = PhotoInfo {
            camera: r.camera,
            lens: r.lens,
            focal: r.focal,
            aperture: r.aperture.map(|av| 2f64.powf(av / 2.)),
            exposure: r.shutter.map(|tv| 2f64.powf(-tv)),
            iso: r.iso,
            dimensions,
        };
        insert(lr.write(), r.id, &info)?;
    }
    Ok(count)
}
