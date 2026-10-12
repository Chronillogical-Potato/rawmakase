//! Each master photo's file checksum (`photo_checksums`), read in the
//! background, for finding the photo again once its file is moved or renamed
//! outside RAWmakase. Virtual copies have none of their own.
use super::db::{Reads, sql};
use super::value::row;
use super::{Catalog, FolderId, PhotoId};
use crate::storage::{CHECKSUM_VERSION, FileChecksum};
use anyhow::Result;
use std::collections::HashMap;
use std::path::PathBuf;

/// A checksum read from the file of a photo, as it was when the read began.
#[derive(Clone, Debug)]
pub struct ReadChecksum {
    pub photo: PhotoId,
    pub folder: FolderId,
    pub filename: String,
    /// The file it was read from.
    pub path: PathBuf,
    pub checksum: FileChecksum,
}

impl Catalog {
    /// The checksums on record, by master, of the current version.
    pub fn checksums(&self) -> Result<HashMap<PhotoId, FileChecksum>> {
        stored(&self.db)
    }
    /// How many masters have a full checksum on record, of how many.
    pub fn checksum_count(&self) -> Result<(usize, usize)> {
        row! {
            struct Count {
                masters: i64,
                full: i64,
            }
        }
        let count: Count = self.db.read_one(
            sql!(
                "SELECT count(*), count(c.full_hash) FROM photos p
                 LEFT JOIN photo_checksums c ON c.photo = p.id AND c.version = ?
                 WHERE p.master_id IS NULL"
            ),
            &[&CHECKSUM_VERSION],
        )?;
        Ok((count.full as usize, count.masters as usize))
    }
    /// Records checksums read from files, in one transaction. Each is kept
    /// only while its photo is still a master in the folder, under the name
    /// and at the path it was read from; one replaced, moved, renamed, made
    /// a copy or located elsewhere meanwhile is dropped. Returns the photos
    /// whose checksums were saved.
    pub fn save_checksums(&mut self, read: &[ReadChecksum]) -> Result<Vec<PhotoId>> {
        let computer = self.computer.id.clone();
        self.db.write(|w| {
            let mut saved = Vec::new();
            for r in read {
                let here = super::locations::folder_path(w, &computer, r.folder)?;
                if here.map(|folder| folder.join(&r.filename)).as_ref() != Some(&r.path) {
                    continue;
                }
                let identity = serde_json::to_string(&r.checksum.identity)?;
                let size = i64::try_from(r.checksum.size)?;
                let written = w.execute(
                    sql!(
                        "INSERT INTO photo_checksums
                         (photo, size, quick_hash, full_hash, identity, version)
                         SELECT id, ?, ?, ?, ?, ? FROM photos
                         WHERE id = ? AND folder = ? AND filename = ? AND master_id IS NULL
                         ON CONFLICT(photo) DO UPDATE SET size=excluded.size,
                             quick_hash=excluded.quick_hash, full_hash=excluded.full_hash,
                             identity=excluded.identity, version=excluded.version"
                    ),
                    &[
                        &size,
                        &r.checksum.quick,
                        &r.checksum.full,
                        &identity,
                        &CHECKSUM_VERSION,
                        &r.photo,
                        &r.folder,
                        &r.filename,
                    ],
                )?;
                if written > 0 {
                    saved.push(r.photo);
                }
            }
            Ok(saved)
        })
    }
}

/// The checksums on record, by master, of the current version, read in `db`.
pub(super) fn stored(db: &impl Reads) -> Result<HashMap<PhotoId, FileChecksum>> {
    row! {
        struct Stored {
            photo: PhotoId,
            size: i64,
            quick: String,
            full: Option<String>,
            identity: String,
        }
    }
    let rows: Vec<Stored> = db.read(
        sql!(
            "SELECT c.photo, c.size, c.quick_hash, c.full_hash, c.identity
             FROM photo_checksums c JOIN photos p ON p.id = c.photo
             WHERE p.master_id IS NULL AND c.version = ?"
        ),
        &[&CHECKSUM_VERSION],
    )?;
    Ok(rows
        .into_iter()
        // One this release can't read is read again.
        .filter_map(|r| {
            Some((
                r.photo,
                FileChecksum {
                    size: u64::try_from(r.size).ok()?,
                    quick: r.quick,
                    full: r.full,
                    identity: serde_json::from_str(&r.identity).ok()?,
                },
            ))
        })
        .collect())
}
