//! Lightroom's Quick Collection: B adds the selected photos or takes them
//! out, Cmd+B shows it and Cmd+Shift+B clears it. It is a collection like
//! any other, the one imported from Lightroom or made on first use, and each
//! change is one undoable command.
use super::{Library, Place};
use crate::catalog::{CollectionKind, QUICK_COLLECTION};
use anyhow::Result;

/// A change to a collection's photos, for the shared undo log.
#[derive(Clone, Debug, PartialEq)]
pub struct CollectionCommand {
    /// Orders it among other changes made in the same frame.
    pub sequence: u64,
    pub collection: i64,
    pub added: Vec<i64>,
    pub removed: Vec<i64>,
    pub place_before: Place,
    pub place_after: Place,
    /// What changed, as the status line said it.
    pub summary: String,
}

impl Library {
    /// The Quick Collection, once there is one.
    fn quick(&self) -> Option<i64> {
        self.collections
            .iter()
            .find(|c| {
                c.kind == CollectionKind::System && c.name == QUICK_COLLECTION && c.parent.is_none()
            })
            .map(|c| c.id)
    }
    /// The Quick Collection, made if there is none yet.
    fn ensure_quick(&mut self) -> Result<i64> {
        if let Some(id) = self.quick() {
            return Ok(id);
        }
        let id = self.catalog.quick_collection()?;
        self.collections = self.catalog.collections()?;
        self.collection_photos.entry(id).or_default();
        Ok(id)
    }
    pub(super) fn in_quick(&self, id: i64) -> bool {
        self.quick()
            .and_then(|q| self.collection_photos.get(&q))
            .is_some_and(|members| members.contains(&id))
    }
    /// The Quick Collection's photo count, for the Catalog panel.
    pub(super) fn quick_count(&self) -> usize {
        self.quick()
            .and_then(|q| self.collection_photos.get(&q))
            .map_or(0, |members| members.len())
    }
    pub(super) fn showing_quick(&self) -> bool {
        self.quick().is_some() && self.filters.collection == self.quick()
    }
    /// B: adds `ids` to the Quick Collection, or takes them out when they are
    /// all in it already.
    pub(in crate::app) fn toggle_quick(&mut self, ids: &[i64]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let quick = self.ensure_quick()?;
        let all_in = ids.iter().all(|id| self.in_quick(*id));
        let (added, removed, verb) = if all_in {
            (Vec::new(), ids.to_vec(), "Removed from")
        } else {
            let added: Vec<i64> = ids
                .iter()
                .copied()
                .filter(|id| !self.in_quick(*id))
                .collect();
            (added, Vec::new(), "Added to")
        };
        let count = added.len() + removed.len();
        let summary = format!(
            "{verb} Quick Collection · {count} {}",
            if count == 1 { "photo" } else { "photos" }
        );
        self.record_collection(quick, added, removed, summary)
    }
    /// Cmd+Shift+B: empties the Quick Collection.
    pub(in crate::app) fn clear_quick(&mut self) -> Result<()> {
        let Some(quick) = self.quick() else {
            return Ok(());
        };
        let removed: Vec<i64> = self
            .collection_photos
            .get(&quick)
            .map(|m| m.iter().copied().collect())
            .unwrap_or_default();
        if removed.is_empty() {
            return Ok(());
        }
        self.record_collection(
            quick,
            Vec::new(),
            removed,
            "Cleared Quick Collection".into(),
        )
    }
    /// Cmd+B: shows the Quick Collection.
    pub(in crate::app) fn show_quick(&mut self) -> Result<()> {
        let quick = self.ensure_quick()?;
        self.select_collection(quick);
        Ok(())
    }
    /// B, Cmd+B and Cmd+Shift+B, for `ids`.
    pub(super) fn quick_key(&mut self, modifiers: eframe::egui::Modifiers, ids: Vec<i64>) {
        let done = match (modifiers.command, modifiers.shift) {
            (false, false) => self.toggle_quick(&ids),
            (true, false) => self.show_quick(),
            (true, true) => self.clear_quick(),
            (false, true) => Ok(()),
        };
        if let Err(e) = done {
            self.message = format!("Quick Collection not changed: {e}");
        }
    }
    /// Makes a change and hands it to the undo log once it is saved.
    fn record_collection(
        &mut self,
        collection: i64,
        added: Vec<i64>,
        removed: Vec<i64>,
        summary: String,
    ) -> Result<()> {
        let place_before = self.place();
        self.change_collection(collection, &added, &removed)?;
        self.message = summary.clone();
        self.collection_done.push(CollectionCommand {
            sequence: crate::app::undo::sequence(),
            collection,
            added,
            removed,
            place_before,
            place_after: self.place(),
            summary,
        });
        Ok(())
    }
    /// Adds and removes photos, in the catalog and as shown; for commands
    /// and their undo.
    pub(in crate::app) fn change_collection(
        &mut self,
        collection: i64,
        add: &[i64],
        remove: &[i64],
    ) -> Result<()> {
        self.catalog.change_collection(collection, add, remove)?;
        let members = self.collection_photos.entry(collection).or_default();
        members.extend(add);
        for id in remove {
            members.remove(id);
        }
        if self.filters.collection == Some(collection) {
            self.filters.members = members.clone();
            self.filter();
        }
        Ok(())
    }
    /// The collection changes made since the last call, for the undo log.
    pub(in crate::app) fn take_collection_done(&mut self) -> Vec<CollectionCommand> {
        std::mem::take(&mut self.collection_done)
    }
}
