//! Develop Snapshots: named states of a photo's edit, as Lightroom's Snapshots panel
//! keeps them. Each photo and virtual copy has its own, as in Lightroom's catalog,
//! where snapshots belong to one image. Snapshots imported from Lightroom keep
//! Lightroom's settings text and are converted when applied.
use super::Catalog;
use crate::develop::Recipe;
use anyhow::{Context, Result, ensure};
use rusqlite::params;

/// Copies snapshots from a Lightroom catalog attached as `lr`.
pub(super) const COPY_LIGHTROOM_SNAPSHOTS: &str =
    "INSERT INTO develop_snapshots(photo, name, recipe, lightroom)
    SELECT image, COALESCE(name, ''), NULL, text
    FROM lr.Adobe_libraryImageDevelopSnapshot
    WHERE text IS NOT NULL AND image IN (SELECT id FROM photos)
      AND NOT EXISTS (SELECT 1 FROM develop_snapshots s
                      WHERE s.photo = image AND s.name = COALESCE(name, '') AND s.lightroom = text);";

/// A snapshot, by name.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub id: i64,
    pub name: String,
    pub settings: SnapshotSettings,
}

/// What a snapshot holds.
#[derive(Clone, Debug, PartialEq)]
pub enum SnapshotSettings {
    /// Made in RAWmakase.
    Recipe(Box<Recipe>),
    /// Imported from Lightroom: its develop-settings text.
    Lightroom(String),
}

/// Set in `meta` once snapshots have been recovered from the stored Lightroom catalog.
pub(super) const SNAPSHOTS_BACKFILLED: &str = "lightroom_snapshots_backfilled";

impl Catalog {
    /// Catalogs imported before snapshots were kept still hold the original Lightroom
    /// catalog; copy its snapshots once. Returns snapshots added.
    pub fn backfill_lightroom_snapshots(&mut self) -> Result<usize> {
        self.backfill_once(SNAPSHOTS_BACKFILLED, |db| {
            if !super::lightroom::has_table(db, "lr", "Adobe_libraryImageDevelopSnapshot")? {
                return Ok(0);
            }
            Ok(db.execute(COPY_LIGHTROOM_SNAPSHOTS, [])?)
        })
    }
    /// The photo's snapshots, alphabetically as Lightroom lists them. A snapshot
    /// that cannot be read (from a newer release) is left out.
    pub fn snapshots(&self, photo: i64) -> Result<Vec<Snapshot>> {
        let mut q = self
            .db
            .prepare("SELECT id, name, recipe, lightroom FROM develop_snapshots WHERE photo=?")?;
        let rows = q
            .query_map([photo], |r| {
                let lightroom = match r.get_ref(3)? {
                    rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => {
                        Some(t.to_vec())
                    }
                    _ => None,
                };
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    lightroom,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut snapshots: Vec<Snapshot> = rows
            .into_iter()
            .filter_map(|(id, name, recipe, lightroom)| {
                let settings = match (recipe, lightroom) {
                    (Some(json), _) => {
                        let recipe: Recipe = serde_json::from_str(&json).ok()?;
                        recipe.validate().ok()?;
                        SnapshotSettings::Recipe(Box::new(recipe))
                    }
                    (None, Some(bytes)) => SnapshotSettings::Lightroom(
                        super::lightroom::history::decode_history_text(&bytes)?,
                    ),
                    (None, None) => return None,
                };
                Some(Snapshot { id, name, settings })
            })
            .collect();
        snapshots.sort_by_cached_key(|s| (s.name.to_lowercase(), s.id));
        Ok(snapshots)
    }
    /// Saves `recipe` as a new snapshot of the photo named `name`; returns its id.
    pub fn add_snapshot(&self, photo: i64, name: &str, recipe: &Recipe) -> Result<i64> {
        recipe.validate()?;
        self.db.execute(
            "INSERT INTO develop_snapshots(photo, name, recipe) VALUES (?, ?, ?)",
            params![photo, snapshot_name(name)?, serde_json::to_string(recipe)?],
        )?;
        Ok(self.db.last_insert_rowid())
    }
    /// Lightroom's Update with Current Settings: the snapshot now holds `recipe`.
    pub fn update_snapshot(&self, id: i64, recipe: &Recipe) -> Result<()> {
        recipe.validate()?;
        let n = self.db.execute(
            "UPDATE develop_snapshots SET recipe=?, lightroom=NULL WHERE id=?",
            params![serde_json::to_string(recipe)?, id],
        )?;
        ensure!(n == 1, "Unknown snapshot");
        Ok(())
    }
    pub fn rename_snapshot(&self, id: i64, name: &str) -> Result<()> {
        let n = self.db.execute(
            "UPDATE develop_snapshots SET name=? WHERE id=?",
            params![snapshot_name(name)?, id],
        )?;
        ensure!(n == 1, "Unknown snapshot");
        Ok(())
    }
    pub fn delete_snapshot(&self, id: i64) -> Result<()> {
        self.db
            .execute("DELETE FROM develop_snapshots WHERE id=?", [id])?;
        Ok(())
    }
}

/// A snapshot's name, trimmed; it may not be empty.
fn snapshot_name(name: &str) -> Result<&str> {
    let name = name.trim();
    (!name.is_empty())
        .then_some(name)
        .context("A snapshot needs a name")
}
