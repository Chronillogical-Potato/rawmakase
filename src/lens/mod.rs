//! Lens corrections. Camera makers embed per-shot vignetting, distortion and lateral
//! chromatic aberration data in their RAW files; Lightroom applies it automatically as
//! the "built-in" lens profile. [`embedded`] reads those tables into a
//! [`LensCorrection`](crate::optics::LensCorrection), which the renderer evaluates at
//! normalized image radius; [`lcp`] reads Adobe lens profiles into the same model.
pub mod auto_ca;
pub mod choice;
pub mod embedded;
pub mod lcp;
