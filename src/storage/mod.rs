//! Durable application state: file identity, shared storage locations and
//! atomic writes.
pub mod bitmaps;
mod files;
mod identity;

pub use files::{Replace, data_dir, is_hidden, is_raw, list_raws, local_data_dir};
pub(crate) use files::{
    asset_dirs, atomic_json, parent_dir, persist, read_json_or_default, stage, sync_dir,
    write_atomic,
};
pub use identity::Identity;
pub use identity::Stamp;
pub(crate) use identity::{FNV_OFFSET, fnv1a};
