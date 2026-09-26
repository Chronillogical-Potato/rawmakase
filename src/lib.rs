//! RAWmakase's domain APIs and desktop application.
//!
//! Start with [`raw`] for decoding and native color management, [`camera_profiles`]
//! for DCP transforms, and [`develop`] for recipes and rendering. [`xmp`] translates
//! Adobe settings; [`presets`] manages reusable native and XMP presets.
//!
//! [`storage`] owns sidecars and session persistence, [`catalog`] owns the photo
//! database and Lightroom import, and [`export`] writes finished images.
//! [`app`] composes these APIs into the desktop editor; [`comparison`] provides
//! reference-image validation and [`platform`] isolates OS integration.
//!
//! The repository's `docs/code-map.md` maps implementation files and runtime flows.
//! `docs/architecture.md` records ownership and compatibility rules. New code
//! should use the domain modules below rather than the hidden legacy aliases.
pub mod app;
pub mod camera_profiles;
pub mod catalog;
mod color_math;
pub mod comparison;
pub mod decode_cache;
pub mod demosaic;
pub mod develop;
pub mod dng;
pub mod export;
pub mod lens;
pub mod platform;
pub mod presets;
pub mod raw;
pub mod storage;
mod tiff;
pub mod xmp;

// Source-compatible entry points for earlier users of the library. New code uses
// the domain modules above; these aliases contain no implementation.
#[doc(hidden)]
pub mod io;
#[doc(hidden)]
pub use app::{library, worker};
#[doc(hidden)]
pub use camera_profiles as profile;
#[doc(hidden)]
pub use catalog::preview_cache;
#[doc(hidden)]
pub use develop as core;
#[doc(hidden)]
pub use develop::{curve, effects, quality};
#[doc(hidden)]
pub use platform::network;
