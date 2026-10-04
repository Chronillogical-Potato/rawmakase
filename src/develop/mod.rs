//! Headless RAW development: recipes, geometry, color and detail rendering.
mod auto;
mod basic_tone;
mod basic_tone_data;
pub(crate) mod calibration;
pub(crate) mod color;
mod color_grade;
mod color_grade_data;
mod color_mixer;
mod crop_constraint;
pub mod curve;
pub mod effects;
mod geometry;
pub mod gpu;
mod image_space;
mod local_tone;
mod local_tone_data;
pub mod masks;
pub mod panels;
mod pipeline;
mod preview_renderer;
mod pyramid;
pub mod settings_groups;
pub mod upright;
pub use preview_renderer::PreviewRenderer;
pub mod quality;
mod recipe;
mod rendered;
pub mod retouch;
mod stage_cache;
mod white_balance;

pub use crate::color_math::{mul, srgb_encode};
pub use auto::{
    auto_tone, auto_tone_basis, auto_tone_cancellable, auto_white_balance,
    auto_white_balance_cancellable,
};
pub use geometry::{Geometry, Transform, Upright, UprightMode, display_axes};
pub use image_space::{ImageFrame, ViewMapping};
pub use pipeline::{
    neutral_pick, pick_fringe, preview, render, render_legacy, render_region, render_region_legacy,
};
pub(crate) use pipeline::{profile_matrix, render_base};
pub use recipe::{
    LocalEdits, ProfileCorrections, Recipe, TEMPERATURE_MAX, TEMPERATURE_MIN, TINT_LIMIT,
};
pub use rendered::Rendered;
pub(crate) use rendered::unit_to_u8;
pub use white_balance::{NamedWhiteBalance, TemperatureTint};
