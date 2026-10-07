//! The native boundary: LibRaw and Little CMS through `native/raw.cpp` ([`raw`]),
//! RAWmakase's own demosaicing ([`demosaic`]), opening a photo ([`photo`]) and its
//! full-size decode through the decode cache ([`decode`], [`decode_cache`]). The
//! C++ bridge is compiled and linked by this crate's build script, so nothing that
//! does not depend on it links LibRaw by accident. Highlight recovery after a
//! decode is the renderer's (crates/rawmakase-engine).
use rawmakase_engine::develop;
use rawmakase_model::{camera_data, camera_profiles, dng, lens, storage};

pub mod decode;
pub mod decode_cache;
mod demosaic;
pub mod photo;
pub mod raw;
