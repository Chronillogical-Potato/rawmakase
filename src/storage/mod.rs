//! Durable application state: file identity, legacy sidecars and shared storage
//! locations.
pub mod bitmaps;
mod files;
mod identity;
mod sidecar;

pub use files::{RAW_EXTENSIONS, Replace, data_dir, is_hidden, is_raw, list_raws, local_data_dir};
pub(crate) use files::{
    asset_dirs, atomic_json, parent_dir, persist, read_json_or_default, stage, sync_dir,
    write_atomic,
};
pub use identity::Stamp;
pub(crate) use identity::{FNV_OFFSET, fnv1a};
pub(crate) use sidecar::import;
pub use sidecar::{Identity, Sidecar, load, save, sidecar_path};
