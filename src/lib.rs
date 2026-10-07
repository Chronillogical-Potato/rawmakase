//! RAWmakase's domain APIs and desktop application.
//!
//! Start with [`raw`] for decoding and native color management, [`camera_profiles`]
//! for DCP transforms, and [`develop`] for recipes and rendering. [`xmp`] translates
//! Adobe settings; [`presets`] manages reusable native and XMP presets.
//!
//! [`storage`] owns file identity and paths, [`catalog`] owns legacy sidecar import,
//! the photo database, edits and Lightroom import, and [`export`] writes finished images.
//! [`app`] composes these APIs into the desktop editor; [`comparison`] provides
//! reference-image validation and [`platform`] isolates OS integration.
//!
//! The repository's `docs/code-map.md` maps implementation files and runtime flows,
//! and `docs/architecture.md` records ownership rules. This library exists for the
//! RAWmakase binary, its examples and its tests; it is not a stable public API.
// The values a photo's edit is made of, built on their own (crates/rawmakase-model).
// File formats and presets over the model (crates/rawmakase-interop).
pub use rawmakase_interop::{
    exif, export_settings, jpeg, lr_develop, presets, raw_defaults, watermark, xmp,
};
pub use rawmakase_model::{
    camera_data, camera_profiles, cameras, color, dng, ids, lens, metadata, model, optics,
    rendered, storage, tiff, xml,
};
pub mod app;
mod build_info;
pub mod catalog;
pub mod catalog_session;
pub mod comparison;
pub mod decode;
pub mod decode_cache;
pub mod demosaic;
pub mod develop;
pub mod edit_session;
pub mod edits;
pub mod export;
pub mod photo;
pub mod platform;
pub mod raw;
#[cfg(feature = "telemetry")]
pub mod stats;
pub mod time;
pub mod updates;
