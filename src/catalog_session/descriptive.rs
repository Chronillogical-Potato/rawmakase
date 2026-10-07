//! Title, caption, creator, copyright, location and keywords, and virtual
//! copies: changes to the catalog that the session's lists must follow.
use super::CatalogSession;
use crate::catalog::{FileMetadata, Merge, MetadataSnapshot, PhotoId, SidecarReport};
use crate::metadata::TextField;
use anyhow::Result;
use std::path::PathBuf;

/// A descriptive metadata change to some photos.
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

/// What a [`DescriptiveEdit`] changed.
#[derive(Debug)]
pub struct DescriptiveChange {
    pub before: Vec<MetadataSnapshot>,
    pub after: Vec<MetadataSnapshot>,
    /// Reading the photos' keywords back into the lists: the change is saved
    /// either way, so it can be undone even when this failed.
    pub listed: Result<()>,
}

impl CatalogSession {
    /// Makes `edit` to `ids` in the catalog and lists their keywords again.
    pub fn edit_descriptive(
        &mut self,
        ids: &[PhotoId],
        edit: DescriptiveEdit,
    ) -> Result<DescriptiveChange> {
        let before = self.catalog.metadata_snapshot(ids)?;
        match edit {
            DescriptiveEdit::Text(field, text) => self.catalog.set_text(ids, field, &text),
            DescriptiveEdit::Creators(names) => self.catalog.set_creators(ids, &names),
            DescriptiveEdit::ClearLocation => self.catalog.clear_location(ids),
            DescriptiveEdit::AddKeywords(paths) => self.catalog.add_keywords(ids, &paths),
            DescriptiveEdit::RemoveKeyword(keyword) => self.catalog.remove_keyword(ids, keyword),
        }?;
        let after = self.catalog.metadata_snapshot(ids)?;
        Ok(DescriptiveChange {
            before,
            after,
            listed: self.refresh_keywords(ids),
        })
    }
    /// Puts photos' descriptive metadata back as `values` has it, with the
    /// rating, flag and label in `ratings` (none for a change that left them),
    /// then lists them again. Both are written before anything is read back,
    /// so a failed read never leaves an undo half done.
    pub fn restore_descriptive(
        &mut self,
        values: &[MetadataSnapshot],
        ratings: &[(PhotoId, i32, i32, String)],
    ) -> Result<()> {
        self.catalog.restore_metadata(values)?;
        if !ratings.is_empty() {
            self.set_ratings(ratings)?;
        }
        let ids: Vec<PhotoId> = values.iter().map(|s| s.photo).collect();
        self.refresh_photos(&ids)
    }
    /// Writes what was read from the files of `ids` (Read Metadata from
    /// Files), replacing the catalog's values, and lists them again.
    pub fn apply_file_metadata(
        &mut self,
        ids: &[PhotoId],
        read: &[(PhotoId, PathBuf, FileMetadata)],
    ) -> Result<SidecarReport> {
        let written = self.catalog.apply_file_metadata(read, Merge::Overwrite)?;
        self.refresh_photos(ids)?;
        Ok(written)
    }
    /// Makes a virtual copy of `id` and reads the lists again.
    pub fn create_virtual_copy(&mut self, id: PhotoId) -> Result<PhotoId> {
        let copy = self.catalog.create_virtual_copy(id)?;
        self.reload()?;
        Ok(copy)
    }
    /// Makes copy `id` its photo's master and reads the lists again.
    pub fn set_copy_as_master(&mut self, id: PhotoId) -> Result<()> {
        self.catalog.set_copy_as_master(id)?;
        self.reload()
    }
    /// Removes virtual copy `id` and reads the lists again.
    pub fn remove_virtual_copy(&mut self, id: PhotoId) -> Result<()> {
        self.catalog.remove_virtual_copy(id)?;
        self.reload()
    }
}
