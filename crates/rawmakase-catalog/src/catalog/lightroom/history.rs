//! Lightroom's develop history, imported with the catalog and shown under
//! "From Lightroom" in the History panel, and the stored Lightroom develop
//! settings a photo opens with.
use crate::catalog::db::{LightroomWrite, Reads, SqliteSql, sql, sqlite_sql};
use crate::catalog::value::{TextOrBlob, row};
use crate::catalog::{Catalog, PhotoId};
use anyhow::Result;

/// Set in `meta` once Lightroom history has been recovered from the stored catalog.
const HISTORY_BACKFILLED: &str = "lightroom_history_backfilled";
/// Copies history steps from an attached Lightroom catalog named `lr`.
pub(super) const COPY_LIGHTROOM_HISTORY: SqliteSql = sqlite_sql!(
    "INSERT INTO lightroom_history(photo,position,name,created,text)
    SELECT image, row_number() OVER (PARTITION BY image ORDER BY dateCreated, id_local),
           COALESCE(name,''), dateCreated, text
    FROM lr.Adobe_libraryImageDevelopHistoryStep
    WHERE text IS NOT NULL AND image IN (SELECT id FROM photos)
    ON CONFLICT DO NOTHING"
);
/// Largest history snapshot accepted. A step's text is a develop-settings string
/// of a few kilobytes; the cap is far above that and only exists so a corrupt
/// catalog cannot name a length the allocation would follow.
const MAX_TEXT_BYTES: usize = 1 << 20;
/// Lightroom stores history snapshots either as text or as a 4-byte
/// big-endian length followed by a zlib stream.
pub(in crate::catalog) fn decode_history_text(bytes: &[u8]) -> Option<String> {
    if bytes.len() > 6 && bytes[4] == 0x78 {
        use std::io::Read;
        // The prefix declares the decompressed length, so it bounds the read and
        // a snapshot that expands past it is corrupt and refused.
        let expected = u32::from_be_bytes(bytes[..4].try_into().ok()?) as usize;
        if expected > MAX_TEXT_BYTES {
            return None;
        }
        let mut text = String::new();
        flate2::read::ZlibDecoder::new(&bytes[4..])
            .take(expected as u64 + 1)
            .read_to_string(&mut text)
            .ok()?;
        return (text.len() <= expected).then_some(text);
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
impl Catalog {
    /// Lightroom's history for a photo, oldest step first.
    pub fn lightroom_history(&self, id: PhotoId) -> Result<Vec<HistoryStep>> {
        row! {
            struct Step {
                name: String,
                created: Option<f64>,
                text: TextOrBlob,
            }
        }
        let steps: Vec<Step> = self.db.read(
            sql!(
                "SELECT name, created, text FROM lightroom_history WHERE photo=? ORDER BY position"
            ),
            &[&id],
        )?;
        Ok(steps
            .into_iter()
            .filter_map(|step| {
                Some(HistoryStep {
                    name: step.name,
                    created: step.created,
                    // A step of any other class is read as empty text.
                    text: decode_history_text(&step.text.0.unwrap_or_default())?,
                })
            })
            .collect())
    }
    /// Catalogs imported before history was kept still hold the original
    /// Lightroom catalog; copy its history steps once. Returns steps added.
    pub fn backfill_lightroom_history(&mut self) -> Result<usize> {
        // Once is enough: without history to recover, the stored catalog would
        // otherwise be written out and attached on every open. History that
        // came with the import leaves nothing to recover, so it is not.
        if self.meta(HISTORY_BACKFILLED)?.is_none() && self.has_lightroom_history()? {
            self.set_meta(HISTORY_BACKFILLED, "1")?;
        }
        self.backfill_once(HISTORY_BACKFILLED, copy_history)
    }
    fn has_lightroom_history(&self) -> Result<bool> {
        let have: i64 = self
            .db
            .read_one(sql!("SELECT count(*) FROM lightroom_history"), &[])?;
        Ok(have > 0)
    }
    #[cfg(test)]
    pub fn lightroom_develop(&self, id: PhotoId) -> Result<Option<String>> {
        self.db.read_one(
            sql!("SELECT lightroom_develop FROM photos WHERE id=?"),
            &[&id],
        )
    }
}

/// Copies history steps from a Lightroom catalog attached as `lr` that has them.
fn copy_history(lr: &mut LightroomWrite<'_>) -> Result<usize> {
    if !lr.has_table("Adobe_libraryImageDevelopHistoryStep")? {
        return Ok(0);
    }
    lr.execute_sqlite(COPY_LIGHTROOM_HISTORY, &[])
}
