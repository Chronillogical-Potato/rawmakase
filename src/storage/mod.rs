//! Durable application state: sidecars, sessions and shared storage locations.
pub mod bitmaps;
mod files;
mod format;
mod identity;
mod paths;
mod session;
mod sidecar;

pub use files::{RAW_EXTENSIONS, Replace, data_dir, is_hidden, is_raw, list_raws};
pub(crate) use files::{
    asset_dirs, atomic_json, parent_dir, persist, read_json_or_default, stage, write_atomic,
};
pub(crate) use format::migrate_recipe;
pub use format::{PIPELINE, SCHEMA, saved_version};
pub use identity::Stamp;
pub(crate) use identity::{FNV_OFFSET, fnv1a};
pub use session::{CropGuideLayout, LibraryLayout, Session, load_session, save_session};
pub(crate) use sidecar::import;
pub use sidecar::{Identity, Sidecar, load, save, sidecar_path};
