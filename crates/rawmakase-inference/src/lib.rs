//! Local ONNX inference for interactive subject selection (Segment Anything 2).
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

pub mod auto;
mod error;
pub mod manifest;
pub mod panoptic;
pub mod process;
pub mod refine;
pub mod runtime;

pub use auto::Proposal;
pub use error::InferenceError;
pub use manifest::{ModelFile, ModelSpec, PANOPTIC, PanopticSpec, SUBJECT};
pub use process::{Coverage, Point, Prompt, RgbImage};
pub use runtime::{Analysis, Embedding, LoadOptions, RUNTIME_ENV, Session, Subject};
