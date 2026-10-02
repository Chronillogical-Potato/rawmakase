use crate::{develop::Recipe, export::ExportOptions};
use std::path::PathBuf;
#[derive(Clone, Debug)]
pub struct Photo {
    pub id: i64,
    pub folder: i64,
    pub path: PathBuf,
    pub filename: String,
    pub captured: String,
    pub rating: i32,
    pub flag: i32,
    pub label: String,
    pub format: String,
    pub copy_name: String,
    /// The photo this one is a virtual copy of; `None` for a master.
    pub master: Option<i64>,
    pub keywords: String,
    pub has_lightroom_edits: bool,
}
#[derive(Clone, Debug)]
pub struct Folder {
    pub relative: String,
    pub id: i64,
    pub root: i64,
    pub name: String,
    pub path: PathBuf,
    pub count: usize,
}
#[derive(Clone, Debug)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub parent: Option<i64>,
    pub kind: CollectionKind,
    pub count: usize,
}
/// What a Lightroom collection row is, from its `creationId`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollectionKind {
    /// A collection set: holds collections and other sets, never photos.
    Set,
    /// A collection, or a saved print, book, slideshow or web creation.
    Collection,
    /// A smart collection, whose rules are not evaluated yet.
    Smart,
    /// Lightroom's own: the Quick Collection and unsaved creations.
    System,
}
impl CollectionKind {
    pub fn from_lightroom(creation_id: &str, name: &str) -> Self {
        match creation_id {
            "com.adobe.ag.library.group" => Self::Set,
            "com.adobe.ag.library.smart_collection" => Self::Smart,
            // Lightroom keeps its Quick Collection as a plain collection by this name.
            "com.adobe.ag.library.collection" if name == "quick collection" => Self::System,
            id if id.ends_with(".unsaved") => Self::System,
            _ => Self::Collection,
        }
    }
}
#[derive(Debug)]
pub struct SavedEdit {
    pub recipe: Recipe,
    pub export: ExportOptions,
}
