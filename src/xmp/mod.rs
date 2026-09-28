//! Namespace-aware Adobe settings parsing and application to a develop recipe.
mod apply;
pub mod local;
mod parse;
pub mod write;
use crate::develop::curve::ToneCurve;
pub use parse::parse;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub group: String,
    pub path: PathBuf,
    pub settings: BTreeMap<String, String>,
    pub curves: BTreeMap<String, ToneCurve>,
    pub look: String,
    pub blockers: Vec<String>,
    pub notes: Vec<String>,
    pub photo_settings: bool,
    /// Spot removal and masks (see `local`), as nested data.
    pub local: BTreeMap<String, local::Node>,
    /// Shipped with RAWmakase (see `presets::builtin`). A built-in preset's Adobe
    /// profile falls back to RAWmakase's own profile when it isn't imported.
    pub builtin: bool,
}

// Compatibility exports; preset collection management lives in `presets`.
#[doc(hidden)]
pub use crate::presets::{
    Library, display_name, favorite_path, import_file, library_dirs, load_favorites, load_library,
    save_favorites,
};
#[cfg(test)]
mod tests;
