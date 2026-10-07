//! Namespace-aware Adobe settings parsing and application to a develop recipe.
mod apply;
pub mod descriptive;
pub mod local;
pub(crate) mod look;
mod parse;
pub mod preset_write;
pub mod write;
use crate::color::curve::ToneCurve;
pub use parse::parse;

/// What applying settings measures on the decoded photo when they ask for Auto:
/// Lightroom's Auto white balance and Auto black & white mix, and the camera image
/// Auto Tone reads. The renderer provides it (`develop::Measures`), so applying
/// settings needs no rendering code of its own.
pub trait PhotoMeasures {
    fn camera_image(&self) -> &crate::camera_data::CameraImage;
    /// `base` with the white balance the WB menu's Auto picks.
    fn auto_white_balance(
        &self,
        base: &crate::model::recipe::Recipe,
    ) -> anyhow::Result<crate::model::recipe::Recipe>;
    /// The Auto black & white mix for `r`, in slider units.
    fn auto_gray_mix(
        &self,
        r: &crate::model::recipe::Recipe,
        m: &crate::camera_data::Metadata,
    ) -> [f32; 8];
}
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub group: String,
    pub path: PathBuf,
    pub settings: BTreeMap<String, String>,
    pub curves: BTreeMap<String, ToneCurve>,
    pub look: String,
    pub blockers: Vec<String>,
    pub notes: Vec<String>,
    pub photo_settings: bool,
    /// Spot removal and masks (see `local`), as nested data.
    pub local: BTreeMap<String, local::Node>,
    /// Shipped with RAWmakase (see `presets::builtin`). Like a photo's own edit, a
    /// built-in preset's Adobe profile falls back to RAWmakase's own profile when it
    /// isn't imported.
    pub builtin: bool,
}
#[cfg(test)]
mod tests;
