//! Lightroom's develop history, imported with the catalog and shown under
//! "From Lightroom" in the History panel, and the stored Lightroom develop
//! settings a photo opens with.
use crate::catalog::Catalog;
use anyhow::Result;
use rusqlite::OptionalExtension;

/// Set in `meta` once Lightroom history has been recovered from the stored catalog.
const HISTORY_BACKFILLED: &str = "lightroom_history_backfilled";
/// Copies history steps from an attached Lightroom catalog named `lr`.
pub(super) const COPY_LIGHTROOM_HISTORY: &str =
    "INSERT OR IGNORE INTO lightroom_history(photo,position,name,created,text)
    SELECT image, row_number() OVER (PARTITION BY image ORDER BY dateCreated, id_local),
           COALESCE(name,''), dateCreated, text
    FROM lr.Adobe_libraryImageDevelopHistoryStep
    WHERE text IS NOT NULL AND image IN (SELECT id FROM photos);";
/// Lightroom stores history snapshots either as text or as a 4-byte
/// big-endian length followed by a zlib stream.
pub(in crate::catalog) fn decode_history_text(bytes: &[u8]) -> Option<String> {
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
impl Catalog {
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
        // Once is enough: without history to recover, the stored catalog would
        // otherwise be written out and attached on every open.
        if self.meta(HISTORY_BACKFILLED)?.is_some() {
            return Ok(0);
        }
        let copied = self.copy_lightroom_history()?;
        self.set_meta(HISTORY_BACKFILLED, "1")?;
        Ok(copied)
    }
    fn copy_lightroom_history(&mut self) -> Result<usize> {
        let have: i64 = self
            .db
            .query_row("SELECT count(*) FROM lightroom_history", [], |r| r.get(0))?;
        if have > 0 {
            return Ok(0);
        }
        let copied = self.with_stored_lightroom(|db| {
            let exists = db
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
            Ok(db.execute(COPY_LIGHTROOM_HISTORY, [])?)
        })?;
        Ok(copied.unwrap_or(0))
    }
    pub fn lightroom_develop(&self, id: i64) -> Result<Option<String>> {
        Ok(self.db.query_row(
            "SELECT lightroom_develop FROM photos WHERE id=?",
            [id],
            |r| r.get(0),
        )?)
    }
}
