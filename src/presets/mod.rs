//! Native recipe presets, built-in presets and installed XMP preset collections.
pub mod amount;
pub mod builtin;
pub mod curves;
mod library;
mod native;
pub mod user;
pub use library::{
    Library, display_name, favorite_path, find_preset, import_file, library_dirs, load_favorites,
    load_library, save_favorites,
};
pub use native::{applied_to, load_preset, save_preset};

#[cfg(test)]
mod tests;
