//! Where each master photo falls in its camera's sequences (`photo_sequences`),
//! read from its file's maker note for Auto-Stack's Group Camera Bursts and
//! kept with the file's size and modification time, so a file is read once
//! and again only when it changes. Virtual copies have none of their own.
use super::db::{Reads, sql};
use super::value::row;
use super::{Catalog, PhotoId};
use crate::storage::Stamp;
use anyhow::Result;
use rawmakase_interop::exif::{Mark, SEQUENCE_VERSION, Sequence};
use std::collections::HashMap;

/// What a photo's file said of its sequence, and which file that was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedSequence {
    /// The file as it was read: what is kept holds only while it stays so.
    pub stamp: Stamp,
    /// `None` for a file that said nothing.
    pub sequence: Option<Sequence>,
}

impl Catalog {
    /// The sequences on record, by master, read by the current version of
    /// the reader. A master missing is to be read; one whose file's stamp
    /// differs, too.
    pub fn sequences(&self) -> Result<HashMap<PhotoId, SavedSequence>> {
        stored(&self.db)
    }
    /// Records sequences read from files, in one transaction, each under
    /// its photo's master now; `None` records that a file said nothing, so
    /// it is not read again while it stays the same. A photo removed
    /// meanwhile has nothing to keep.
    pub fn save_sequences(&mut self, read: &[(PhotoId, SavedSequence)]) -> Result<()> {
        self.db.write(|w| {
            for (id, saved) in read {
                let (camera, burst, frame) = match &saved.sequence {
                    None => (None, None, None),
                    Some(Sequence { camera, mark }) => match mark {
                        Mark::Burst(id) => (Some(camera), Some(id), None),
                        Mark::Frame(n) => (Some(camera), None, Some(*n)),
                        Mark::Single => (Some(camera), None, Some(0)),
                        Mark::Continuous => (Some(camera), None, None),
                    },
                };
                let size = i64::try_from(saved.stamp.size)?;
                let modified = i64::try_from(saved.stamp.modified_ns)?;
                w.execute(
                    sql!(
                        "INSERT INTO photo_sequences
                         (photo, camera, burst, frame, size, modified_ns, version)
                         SELECT COALESCE(master_id, id), ?, ?, ?, ?, ?, ? FROM photos WHERE id = ?
                         ON CONFLICT(photo) DO UPDATE SET camera=excluded.camera,
                             burst=excluded.burst, frame=excluded.frame, size=excluded.size,
                             modified_ns=excluded.modified_ns, version=excluded.version"
                    ),
                    &[
                        &camera,
                        &burst,
                        &frame,
                        &size,
                        &modified,
                        &SEQUENCE_VERSION,
                        id,
                    ],
                )?;
            }
            Ok(())
        })
    }
}

/// [`Catalog::sequences`], read in `db`. A row that isn't one this writes
/// counts as not read, so its file is read again.
fn stored(db: &impl Reads) -> Result<HashMap<PhotoId, SavedSequence>> {
    row! {
        struct Stored {
            photo: PhotoId,
            camera: Option<String>,
            burst: Option<String>,
            frame: Option<u32>,
            size: i64,
            modified: i64,
        }
    }
    let rows: Vec<Stored> = db.read(
        sql!(
            "SELECT s.photo, s.camera, s.burst, s.frame, s.size, s.modified_ns
             FROM photo_sequences s JOIN photos p ON p.id = s.photo
             WHERE p.master_id IS NULL AND s.version = ?"
        ),
        &[&SEQUENCE_VERSION],
    )?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            let stamp = Stamp {
                size: u64::try_from(r.size).ok()?,
                modified_ns: u128::try_from(r.modified).ok()?,
            };
            let sequence = match r.camera {
                None => None,
                Some(camera) => Some(Sequence {
                    camera,
                    mark: match (r.burst, r.frame) {
                        (Some(id), None) => Mark::Burst(id),
                        (None, Some(0)) => Mark::Single,
                        (None, Some(n)) => Mark::Frame(n),
                        (None, None) => Mark::Continuous,
                        _ => return None,
                    },
                }),
            };
            Some((r.photo, SavedSequence { stamp, sequence }))
        })
        .collect())
}
