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
    pub smart: bool,
    pub count: usize,
}
#[derive(Debug)]
pub struct SavedEdit {
    pub recipe: Recipe,
    pub export: ExportOptions,
}
