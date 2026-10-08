//! Local ONNX inference for automatic subject selection.
//!
//! This crate owns the pinned model contract ([`manifest`]), the pure image
//! to tensor and matte to coverage mappings ([`process`]) and the lazily loaded
//! ONNX Runtime session ([`runtime`]). It depends on no other workspace crate:
//! callers pass an 8-bit sRGB [`RgbImage`] and get 8-bit [`Coverage`] back in
//! the same frame. Where the model file lives, how it is downloaded and
//! verified, job scheduling and cache identity belong to the callers.
//!
//! A missing or unusable ONNX Runtime library is reported as
//! [`InferenceError::RuntimeUnavailable`]; the library is never linked.

mod error;
pub mod manifest;
pub mod process;
pub mod runtime;

pub use error::InferenceError;
pub use manifest::{Activation, ModelSpec, Resize, SUBJECT};
pub use process::{Coverage, RgbImage};
pub use runtime::{LoadOptions, RUNTIME_ENV, Session, Subject};
