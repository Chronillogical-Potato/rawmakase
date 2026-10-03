//! Title, caption, creator, copyright, location and keyword changes made in
//! the Metadata and Keywording panels: to every photo selected in the Grid,
//! else to the active one. Each is one transaction where it can be, and one
//! command for the shared undo log, which puts back each photo's rows as they
//! were, absent ones included.
use super::{Library, Place};
use crate::catalog::{MetadataSnapshot, TextField};
use anyhow::{Result, ensure};

/// A descriptive metadata change, for the shared undo log.
#[derive(Clone, Debug, PartialEq)]
pub struct DescriptiveCommand {
    /// Orders it among other changes made in the same frame.
    pub sequence: u64,
    pub before: Vec<MetadataSnapshot>,
    pub after: Vec<MetadataSnapshot>,
    pub place_before: Place,
    pub place_after: Place,
    /// What changed, as the status line said it.
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DescriptiveEdit {
    /// The default language's text; empty clears the field.
    Text(TextField, String),
    /// In order; none clears the field.
    Creators(Vec<String>),
    ClearLocation,
    /// Keywords by path, top first.
    AddKeywords(Vec<Vec<String>>),
    RemoveKeyword(i64),
}

/// Keywords typed as Lightroom takes them: separated by commas, a child
/// before its parents, "Child < Parent". Returns their paths, top first.
pub fn parse_keywords(text: &str) -> Result<Vec<Vec<String>>> {
    let mut paths = Vec::new();
    for entry in text.split(',') {
        if entry.trim().is_empty() {
            continue;
        }
        let mut path: Vec<String> = entry
            .split('<')
            .map(|name| name.trim().to_string())
            .collect();
        ensure!(
            path.iter().all(|name| !name.is_empty()),
            "A keyword needs a name: \"{}\"",
            entry.trim()
        );
        ensure!(
            path.iter().all(|name| !name.contains('|')),
            "Keywords can't contain \"|\""
        );
        path.reverse();
        paths.push(path);
    }
    Ok(paths)
}

impl Library {
    /// Makes `edit` to the photos given and hands it to the undo log.
    pub(in crate::app) fn edit_descriptive(
        &mut self,
        ids: &[i64],
        edit: DescriptiveEdit,
    ) -> Result<()> {
        self.edit_descriptive_at(ids, edit, None)
    }
    /// `edit_descriptive`, for an edit that belongs to `place`, where it was
    /// typed, rather than where the Library is now; undo returns there.
    pub(super) fn edit_descriptive_at(
        &mut self,
        ids: &[i64],
        edit: DescriptiveEdit,
        place: Option<Place>,
    ) -> Result<()> {
        let ids: Vec<i64> = ids
            .iter()
            .copied()
            .filter(|id| self.photo(*id).is_some())
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let place_before = place.clone().unwrap_or_else(|| self.place());
        let before = self.catalog.metadata_snapshot(&ids)?;
        let what = match &edit {
            DescriptiveEdit::Text(TextField::Title, _) => "Title",
            DescriptiveEdit::Text(TextField::Caption, _) => "Caption",
            DescriptiveEdit::Text(TextField::Copyright, _) => "Copyright",
            DescriptiveEdit::Creators(_) => "Creator",
            DescriptiveEdit::ClearLocation => "Location cleared",
            DescriptiveEdit::AddKeywords(_) => "Keywords added",
            DescriptiveEdit::RemoveKeyword(_) => "Keyword removed",
        };
        let made = match edit {
            DescriptiveEdit::Text(field, text) => self.catalog.set_text(&ids, field, &text),
            DescriptiveEdit::Creators(names) => self.catalog.set_creators(&ids, &names),
            DescriptiveEdit::ClearLocation => self.catalog.clear_location(&ids),
            DescriptiveEdit::AddKeywords(paths) => self.catalog.add_keywords(&ids, &paths),
            DescriptiveEdit::RemoveKeyword(keyword) => self.catalog.remove_keyword(&ids, keyword),
        };
        made?;
        let after = self.catalog.metadata_snapshot(&ids)?;
        if before == after {
            return self.refresh_keywords(&ids);
        }
        let summary = match ids.len() {
            1 => what.to_string(),
            n => format!("{n} photos · {what}"),
        };
        self.message = summary.clone();
        self.descriptive_done.push(DescriptiveCommand {
            sequence: crate::app::undo::sequence(),
            before,
            after,
            place_before,
            place_after: place.unwrap_or_else(|| self.place()),
            summary,
        });
        self.fields.reload();
        // Recorded first: a change saved is one undo can reverse, even if
        // what is shown can't be read again.
        self.refresh_keywords(&ids)
    }
    /// Puts photos' descriptive metadata back, for undo and redo, without
    /// recording a change of its own.
    pub(in crate::app) fn restore_descriptive(
        &mut self,
        values: &[MetadataSnapshot],
    ) -> Result<()> {
        // A photo removed since (a virtual copy) is left out.
        let values: Vec<MetadataSnapshot> = values
            .iter()
            .filter(|s| self.photo(s.photo).is_some())
            .cloned()
            .collect();
        self.catalog.restore_metadata(&values)?;
        let ids: Vec<i64> = values.iter().map(|s| s.photo).collect();
        self.refresh_keywords(&ids)?;
        self.fields.reload();
        Ok(())
    }
    /// The descriptive changes made since the last call, for the undo log.
    pub(in crate::app) fn take_descriptive_done(&mut self) -> Vec<DescriptiveCommand> {
        std::mem::take(&mut self.descriptive_done)
    }
    /// The keywords shown for `ids`, read again from the catalog, and the
    /// photos shown, which a text filter may pick by them.
    fn refresh_keywords(&mut self, ids: &[i64]) -> Result<()> {
        let mut names = std::collections::HashMap::new();
        for id in ids {
            let keywords: Vec<String> = self
                .catalog
                .keywords(*id)?
                .into_iter()
                .map(|k| k.name)
                .collect();
            names.insert(*id, keywords.join(", "));
        }
        for p in &mut self.photos {
            if let Some(n) = names.remove(&p.id) {
                p.keywords = n;
            }
        }
        self.filter();
        Ok(())
    }
}
