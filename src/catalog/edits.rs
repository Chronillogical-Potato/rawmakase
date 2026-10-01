//! A photo's saved edit: its recipe and export options, the spots and masks
//! kept beside them, and the bitmaps recipes refer to by hash.
use super::{Catalog, SavedEdit};
use crate::{develop::Recipe, export::ExportOptions, storage::Identity};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use std::path::Path;

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
    ) -> Result<()> {
        recipe.validate()?;
        export.validate()?;
        let identity = Identity::read(path)?;
        // Refuse replacing an edit after the underlying source changed.
        let _ = self.load_edit(id, path)?;
        let (saved, local) = recipe.split_local();
        let tx = self.db.unchecked_transaction()?;
        ensure!(tx.execute("UPDATE photos SET recipe=?,export_options=?,identity=?,edited_at=CURRENT_TIMESTAMP WHERE id=?",params![serde_json::to_string(&saved)?,serde_json::to_string(export)?,serde_json::to_string(&identity)?,id])?==1,"Unknown photo");
        if local.is_empty() {
            tx.execute("DELETE FROM local_edits WHERE photo=?", [id])?;
        } else {
            tx.execute(
                "INSERT OR REPLACE INTO local_edits(photo, data) VALUES (?, ?)",
                params![id, serde_json::to_string(&local)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// The photo's spots and masks, saved apart from its recipe.
    fn local_edits(&self, id: i64) -> Result<crate::develop::LocalEdits> {
        let data: Option<String> = self
            .db
            .query_row("SELECT data FROM local_edits WHERE photo=?", [id], |r| {
                r.get(0)
            })
            .optional()?;
        let local: crate::develop::LocalEdits = match data {
            Some(d) => serde_json::from_str(&d)?,
            None => Default::default(),
        };
        local.validate()?;
        Ok(local)
    }
    pub fn load_edit(&self, id: i64, path: &Path) -> Result<Option<SavedEdit>> {
        let (recipe, export, identity): (Option<String>, Option<String>, Option<String>) =
            self.db.query_row(
                "SELECT recipe,export_options,identity FROM photos WHERE id=?",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
        if let Some(recipe) = recipe {
            let saved: Identity =
                serde_json::from_str(&identity.context("Missing photo identity")?)?;
            ensure!(
                saved == Identity::read(path)?,
                "Photo changed since this catalog edit was saved; catalog edit protected"
            );
            let recipe: Recipe = serde_json::from_str(&recipe)?;
            let recipe = recipe.with_local(self.local_edits(id)?);
            recipe.validate()?;
            let export: ExportOptions =
                serde_json::from_str(&export.context("Missing export settings")?)?;
            export.validate()?;
            Ok(Some(SavedEdit { recipe, export }))
        } else {
            Ok(None)
        }
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
