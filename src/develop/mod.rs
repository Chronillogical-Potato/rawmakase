//! Headless RAW development: recipes, geometry, color and detail rendering.
mod basic_tone;
mod basic_tone_data;
pub(crate) mod calibration;
pub(crate) mod color;
mod color_grade;
mod color_grade_data;
mod color_mixer;
pub mod curve;
pub mod effects;
mod geometry;
pub mod gpu;
mod local_tone;
mod local_tone_data;
mod pipeline;
mod preview_renderer;
mod pyramid;
pub use preview_renderer::PreviewRenderer;
pub mod quality;
mod recipe;
mod rendered;
mod white_balance;

pub use crate::color_math::{mul, srgb_encode};
pub use geometry::{Geometry, Transform};
pub use pipeline::{
    neutral_pick, preview, render, render_legacy, render_region, render_region_legacy,
};
pub(crate) use pipeline::{profile_matrix, render_base};
pub use recipe::{Recipe, TEMPERATURE_MAX, TEMPERATURE_MIN, TINT_LIMIT};
pub use rendered::Rendered;
