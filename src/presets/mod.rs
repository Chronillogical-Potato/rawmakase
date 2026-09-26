//! Native recipe presets and installed XMP preset collections.
mod library;
mod native;
pub use library::{
    Library, display_name, favorite_path, import_file, library_dirs, load_favorites, load_library,
    save_favorites,
};
pub use native::{load_preset, save_preset};

#[cfg(test)]
mod tests;
