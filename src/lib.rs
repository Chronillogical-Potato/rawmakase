//! RAWmakase's domain APIs and desktop application.
//!
//! Start with [`raw`] for decoding and native color management, [`camera_profiles`]
//! for DCP transforms, and [`develop`] for recipes and rendering. [`xmp`] translates
//! Adobe settings; [`presets`] manages reusable native and XMP presets.
//!
//! [`storage`] owns session persistence and legacy sidecar import, [`catalog`] owns
//! the photo database, edits and Lightroom import, and [`export`] writes finished images.
//! [`app`] composes these APIs into the desktop editor; [`comparison`] provides
//! reference-image validation and [`platform`] isolates OS integration.
//!
//! The repository's `docs/code-map.md` maps implementation files and runtime flows,
//! and `docs/architecture.md` records ownership rules. This library exists for the
//! RAWmakase binary, its examples and its tests; it is not a stable public API.
pub mod app;
mod build_info;
pub mod camera_profiles;
pub mod cameras;
pub mod catalog;
pub mod color;
pub mod comparison;
pub mod decode_cache;
pub mod demosaic;
pub mod develop;
pub mod dng;
pub mod exif;
pub mod export;
mod jpeg;
pub mod lens;
pub mod lr_develop;
pub mod metadata;
pub mod optics;
pub mod platform;
pub mod presets;
pub mod raw;
#[cfg(feature = "telemetry")]
pub mod stats;
pub mod storage;
mod tiff;
pub mod time;
pub mod updates;
pub mod watermark;
pub mod xml;
pub mod xmp;
