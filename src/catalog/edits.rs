//! A photo's saved edit: its recipe and export options, the spots and masks
//! kept beside them, and the bitmaps recipes refer to by hash.
use super::{Catalog, SavedEdit};
use crate::{develop::Recipe, export::ExportOptions, storage::Identity};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};
use std::path::Path;

/// One photo's change for [`Catalog::change_edits`].
pub enum EditChange<'a, 'b> {
    Save(&'b EditToSave<'a>),
    Clear { id: i64 },
}

/// One photo's edit for [`Catalog::save_edits`].
pub struct EditToSave<'a> {
    pub id: i64,
    pub path: &'a Path,
    pub recipe: &'a Recipe,
    pub export: &'a ExportOptions,
    pub history: super::HistoryUpdate<'a>,
}

impl Catalog {
    /// Stores `bitmap` once and returns the hash that refers to it.
    pub fn put_bitmap(&self, bitmap: &crate::storage::bitmaps::Bitmap) -> Result<String> {
        let hash = bitmap.hash();
        self.db.execute(
            "INSERT OR IGNORE INTO bitmaps(hash, data) VALUES (?, ?)",
            params![hash, bitmap.compress()?],
        )?;
        Ok(hash)
    }
    #[cfg(test)]
    pub(crate) fn bitmap(&self, hash: &str) -> Result<Option<crate::storage::bitmaps::Bitmap>> {
        let data: Option<Vec<u8>> = self
            .db
            .query_row("SELECT data FROM bitmaps WHERE hash=?", [hash], |r| {
                r.get(0)
            })
            .optional()?;
        data.map(|d| crate::storage::bitmaps::Bitmap::decompress(&d))
            .transpose()
    }
    pub fn save_edit(
        &self,
        id: i64,
        path: &Path,
        recipe: &Recipe,
        export: &ExportOptions,
        history: super::HistoryUpdate<'_>,
    ) -> Result<()> {
        self.save_edits(&[EditToSave {
            id,
            path,
            recipe,
            export,
            history,
        }])
    }
    /// Saves several photos' edits in one transaction: all of them, or none when one
    /// fails (as a Sync to many photos is one change).
    pub fn save_edits(&self, edits: &[EditToSave<'_>]) -> Result<()> {
        self.change_edits(&edits.iter().map(EditChange::Save).collect::<Vec<_>>())
    }
    /// Saves or clears several photos' edits in one transaction. Clearing returns a
    /// photo to having no RAWmakase edit: no recipe, spots, masks or History.
    pub fn change_edits(&self, changes: &[EditChange<'_, '_>]) -> Result<()> {
        let mut identities = Vec::new();
        for change in changes {
            if let EditChange::Save(e) = change {
                e.recipe.validate()?;
                e.export.validate()?;
                identities.push(Identity::read(e.path)?);
                // Refuse replacing an edit after the underlying source changed.
                let _ = self.load_edit(e.id, e.path)?;
            }
        }
        let tx = self.db.unchecked_transaction()?;
        let mut identities = identities.into_iter();
        for change in changes {
            let e = match change {
                EditChange::Save(e) => e,
                EditChange::Clear { id } => {
                    ensure!(tx.execute("UPDATE photos SET recipe=NULL,export_options=NULL,identity=NULL,edited_at=NULL WHERE id=?", [id])? == 1, "Unknown photo");
                    tx.execute("DELETE FROM local_edits WHERE photo=?", [id])?;
                    tx.execute("DELETE FROM develop_history WHERE photo=?", [id])?;
                    continue;
                }
            };
            let identity = identities.next().expect("one identity per save");
            let (saved, local) = e.recipe.split_local();
            ensure!(tx.execute("UPDATE photos SET recipe=?,export_options=?,identity=?,edited_at=CURRENT_TIMESTAMP WHERE id=?",params![serde_json::to_string(&saved)?,serde_json::to_string(e.export)?,serde_json::to_string(&identity)?,e.id])?==1,"Unknown photo");
            if local.is_empty() {
                tx.execute("DELETE FROM local_edits WHERE photo=?", [e.id])?;
            } else {
                tx.execute(
                    "INSERT OR REPLACE INTO local_edits(photo, data) VALUES (?, ?)",
                    params![e.id, serde_json::to_string(&local)?],
                )?;
            }
            Self::put_history(&tx, e.id, e.history)?;
        }
        tx.commit()?;
        Ok(())
    }
    /// The photo's spots and masks, saved apart from its recipe.
    fn local_edits(&self, id: i64) -> Result<crate::develop::LocalEdits> {
        local_edits(self.local_text(id)?.as_deref())
    }
    /// The photo's spots and masks as stored, unread.
    pub(super) fn local_text(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT data FROM local_edits WHERE photo=?", [id], |r| {
                r.get(0)
            })
            .optional()?)
    }
    /// The photo's saved RAWmakase edit, if it has one; an error when it can't be
    /// read or its file changed since it was saved.
    pub fn load_edit(&self, id: i64, path: &Path) -> Result<Option<SavedEdit>> {
        self.edit_record(id)?.saved(path)
    }
    /// When each edited photo was last edited, as "YYYY-MM-DD HH:MM:SS"
    /// UTC: in RAWmakase, or else in Lightroom, whose history counts seconds
    /// from 2001.
    pub fn edit_times(&self) -> Result<std::collections::HashMap<i64, String>> {
        let mut query = self.db.prepare(
            "SELECT p.id, COALESCE(p.edited_at,
                 (SELECT datetime(MAX(h.created) + 978307200, 'unixepoch')
                  FROM lightroom_history h WHERE h.photo = p.id))
             FROM photos p
             WHERE p.edited_at IS NOT NULL
                OR EXISTS (SELECT 1 FROM lightroom_history h
                           WHERE h.photo = p.id AND h.created IS NOT NULL)",
        )?;
        let rows = query.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
    /// Changes whenever the photo's edit does: a hash of its recipe, its
    /// spots and masks, and its Lightroom settings. Cheaper than reading
    /// the edit itself, for previews to notice an edit saved elsewhere.
    pub fn edit_stamp(&self, id: i64) -> Result<u64> {
        use std::hash::{Hash, Hasher};
        let texts: [Option<String>; 3] = self
            .db
            .prepare_cached(
                "SELECT recipe, lightroom_develop, \
                 (SELECT data FROM local_edits WHERE photo=photos.id) FROM photos WHERE id=?",
            )?
            .query_row([id], |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?]))?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        texts.hash(&mut hasher);
        Ok(hasher.finish())
    }
    /// The saved RAWmakase recipe (JSON, with its spots and masks) and Lightroom
    /// develop text, if any.
    pub fn edit_texts(&self, id: i64) -> Result<(Option<String>, Option<String>)> {
        let (recipe, lightroom): (Option<String>, Option<String>) = self.db.query_row(
            "SELECT recipe, lightroom_develop FROM photos WHERE id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let local = self.local_edits(id)?;
        let recipe = match recipe {
            Some(text) if !local.is_empty() => {
                let recipe: Recipe = serde_json::from_str(&text)?;
                Some(serde_json::to_string(&recipe.with_local(local))?)
            }
            other => other,
        };
        Ok((recipe, lightroom))
    }
}
/// Spots and masks from their stored text; none when there is none.
pub(super) fn local_edits(text: Option<&str>) -> Result<crate::develop::LocalEdits> {
    let local: crate::develop::LocalEdits = match text {
        Some(d) => serde_json::from_str(d)?,
        None => Default::default(),
    };
    local.validate()?;
    Ok(local)
}
