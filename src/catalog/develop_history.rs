//! A photo's Develop History, kept with its edit so it survives switching photos and
//! restarting. Stored zlib-compressed in `develop_history`; each step holds the full
//! state it leaves, with large settings (camera profile, masks, spots, curves) stored
//! once per History and referred to from every step that has them.
use super::Catalog;
use crate::develop::Recipe;
use anyhow::{Context, Result, ensure};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::io::Read;

/// Format of the stored History; a newer one is ignored, not misread.
const VERSION: u32 = 1;
/// Largest stored History accepted, decompressed.
const MAX_BYTES: u64 = 256 << 20;
/// Settings longer than this (as JSON) are stored once and referred to.
const POOLED_BYTES: usize = 256;

/// A History as saved: the state before its first step, every step with the state it
/// leaves, done steps first, and how many are applied.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedHistory {
    pub origin: Recipe,
    pub steps: Vec<SavedStep>,
    pub applied: usize,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SavedStep {
    pub name: String,
    pub value: String,
    pub recipe: Recipe,
}

/// What saving an edit does to its stored History.
#[derive(Clone, Copy, Debug)]
pub enum HistoryUpdate<'a> {
    /// Leave it: the edit changed outside Develop's History (Undo after moving to
    /// another photo, a sidecar import), which the History notices when the photo
    /// next opens.
    Keep,
    Replace(&'a SavedHistory),
}

/// One recipe with its large settings replaced by indexes into `Stored::pool`.
#[derive(Serialize, Deserialize)]
struct StoredState {
    fields: Map<String, Value>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pooled: std::collections::BTreeMap<String, usize>,
}
#[derive(Serialize, Deserialize)]
struct StoredStep {
    name: String,
    value: String,
    state: StoredState,
}
#[derive(Serialize, Deserialize)]
struct Stored {
    version: u32,
    pool: Vec<Value>,
    origin: StoredState,
    steps: Vec<StoredStep>,
    applied: usize,
}

/// Interns large setting values so each is stored once.
#[derive(Default)]
struct Pool {
    values: Vec<Value>,
    index: std::collections::HashMap<String, usize>,
}
impl Pool {
    fn state(&mut self, r: &Recipe) -> Result<StoredState> {
        let Value::Object(all) = serde_json::to_value(r)? else {
            anyhow::bail!("Recipe is not an object");
        };
        let mut state = StoredState {
            fields: Map::new(),
            pooled: Default::default(),
        };
        for (key, value) in all {
            let text = value.to_string();
            if text.len() <= POOLED_BYTES {
                state.fields.insert(key, value);
                continue;
            }
            let n = *self.index.entry(text).or_insert_with(|| {
                self.values.push(value);
                self.values.len() - 1
            });
            state.pooled.insert(key, n);
        }
        Ok(state)
    }
}
impl StoredState {
    fn recipe(self, pool: &[Value]) -> Result<Recipe> {
        let mut fields = self.fields;
        for (key, n) in self.pooled {
            let value = pool.get(n).context("Missing pooled History setting")?;
            fields.insert(key, value.clone());
        }
        let recipe: Recipe = serde_json::from_value(Value::Object(fields))?;
        recipe.validate()?;
        Ok(recipe)
    }
}

impl SavedHistory {
    /// How saving the edit treats the stored History. Without steps nothing was
    /// recorded here, so a stored one (even one this release cannot read, from a
    /// newer release) is kept.
    pub fn update(&self) -> HistoryUpdate<'_> {
        if self.steps.is_empty() {
            HistoryUpdate::Keep
        } else {
            HistoryUpdate::Replace(self)
        }
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        ensure!(self.applied <= self.steps.len(), "Invalid History position");
        let mut pool = Pool::default();
        let origin = pool.state(&self.origin)?;
        let steps = self
            .steps
            .iter()
            .map(|s| {
                Ok(StoredStep {
                    name: s.name.clone(),
                    value: s.value.clone(),
                    state: pool.state(&s.recipe)?,
                })
            })
            .collect::<Result<_>>()?;
        let stored = Stored {
            version: VERSION,
            pool: pool.values,
            origin,
            steps,
            applied: self.applied,
        };
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        serde_json::to_writer(&mut z, &stored)?;
        Ok(z.finish()?)
    }
    /// The History in `data`; `None` for a format from a newer release.
    pub fn decode(data: &[u8]) -> Result<Option<Self>> {
        let mut text = Vec::new();
        ZlibDecoder::new(data)
            .take(MAX_BYTES + 1)
            .read_to_end(&mut text)?;
        ensure!(text.len() as u64 <= MAX_BYTES, "Develop History too large");
        let value: Value = serde_json::from_slice(&text)?;
        let version = value
            .get("version")
            .and_then(Value::as_u64)
            .context("Develop History without a version")?;
        if version > u64::from(VERSION) {
            return Ok(None);
        }
        let stored: Stored = serde_json::from_value(value)?;
        ensure!(
            stored.applied <= stored.steps.len(),
            "Invalid History position"
        );
        let pool = stored.pool;
        Ok(Some(Self {
            origin: stored.origin.recipe(&pool)?,
            steps: stored
                .steps
                .into_iter()
                .map(|s| {
                    Ok(SavedStep {
                        name: s.name,
                        value: s.value,
                        recipe: s.state.recipe(&pool)?,
                    })
                })
                .collect::<Result<_>>()?,
            applied: stored.applied,
        }))
    }
}

impl Catalog {
    /// The photo's saved Develop History. `None` without one, or when it cannot be
    /// read (from a newer release, or damaged): the edit itself stays usable.
    pub fn load_history(&self, id: i64) -> Result<Option<SavedHistory>> {
        let data: Option<Vec<u8>> = self
            .db
            .query_row(
                "SELECT data FROM develop_history WHERE photo=?",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(data.and_then(|d| SavedHistory::decode(&d).ok().flatten()))
    }
    /// Stores `history` for the photo inside the transaction saving its edit.
    pub(super) fn put_history(
        tx: &rusqlite::Transaction<'_>,
        id: i64,
        history: HistoryUpdate<'_>,
    ) -> Result<()> {
        match history {
            HistoryUpdate::Keep => {}
            // An empty History stores as none (Sync's Undo on a photo that had none).
            HistoryUpdate::Replace(h) if h.steps.is_empty() => {
                tx.execute("DELETE FROM develop_history WHERE photo=?", [id])?;
            }
            HistoryUpdate::Replace(h) => {
                tx.execute(
                    "INSERT OR REPLACE INTO develop_history(photo, data) VALUES (?, ?)",
                    params![id, h.encode()?],
                )?;
            }
        }
        Ok(())
    }
}
