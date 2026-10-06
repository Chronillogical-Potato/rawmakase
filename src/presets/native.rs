//! RAWmakase JSON recipes, including migration of earlier pipeline versions.
use crate::{
    develop::Recipe,
    develop::saved_format::{migrate_recipe, saved_version},
    storage::atomic_json,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs::File, path::Path};

#[derive(Serialize, Deserialize)]
struct Preset {
    schema: u32,
    pipeline: u32,
    recipe: Recipe,
    /// Masks, kept outside the recipe so releases before them read the rest (the
    /// envelope accepts unknown keys in every release).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    masks: Vec<crate::develop::masks::MaskGroup>,
}
pub fn save_preset(path: &Path, r: &Recipe) -> Result<()> {
    r.validate()?;
    // Spot removal is specific to its photo; Lightroom presets never include it.
    let (mut recipe, local) = r.split_local();
    // Auto white balance was estimated for this photo; applied elsewhere its values are
    // just Custom.
    recipe.auto_white_balance = None;
    // Upright's corrections were analysed from this photo; the mode stays.
    recipe.upright.clear_analysis();
    atomic_json(
        path,
        &Preset {
            schema: saved_version(&recipe),
            pipeline: saved_version(&recipe),
            recipe,
            masks: local.masks,
        },
    )
}
pub fn load_preset(path: &Path) -> Result<Recipe> {
    let mut v: serde_json::Value = serde_json::from_reader(File::open(path)?)?;
    migrate_recipe(&mut v)?;
    let p: Preset = serde_json::from_value(v)?;
    let mut recipe = p.recipe.with_local(crate::develop::LocalEdits {
        retouch: Vec::new(),
        red_eye: Default::default(),
        masks: p.masks,
    });
    recipe.upright.clear_analysis();
    recipe.validate()?;
    Ok(recipe)
}

/// `preset` as applied to a photo edited as `photo`: presets never carry spot removal
/// or red eye, so the photo keeps its own and their panel switches; the preset's masks
/// replace the photo's only when it has any, as in Lightroom.
pub fn applied_to(mut preset: Recipe, photo: &Recipe) -> Recipe {
    use crate::develop::panels::Panel;
    preset.retouch = photo.retouch.clone();
    preset.retouch_model = photo.retouch_model;
    preset.red_eye = photo.red_eye.clone();
    for panel in [Panel::SpotRemoval, Panel::RedEye] {
        preset.panels.set(panel, photo.panels.state(panel));
    }
    if preset.masks.is_empty() {
        preset.masks = photo.masks.clone();
    }
    preset
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::{
        panels::{Panel, PanelState},
        red_eye::RedEyeOp,
    };
    #[test]
    fn a_preset_leaves_the_photos_red_eye_and_its_switch() {
        let mut photo = Recipe::default();
        photo.red_eye.push(RedEyeOp {
            kind: Default::default(),
            center: [0.4, 0.4],
            radius: [0.01; 2],
            correlation: 0.,
            pupil_size: 0.5,
            darken: 0.5,
        });
        // The photo's spots keep the feather they were made with.
        let mut preset = Recipe {
            exposure: 0.5,
            retouch_model: crate::develop::retouch::RetouchModel::Measured,
            ..Default::default()
        };
        preset.panels.set(Panel::RedEye, PanelState::Off);
        preset.panels.set(Panel::SpotRemoval, PanelState::Off);
        let applied = applied_to(preset, &photo);
        assert_eq!(applied.exposure, 0.5);
        assert_eq!(applied.red_eye, photo.red_eye);
        assert_eq!(applied.retouch_model, photo.retouch_model);
        assert_eq!(applied.panels.state(Panel::RedEye), PanelState::On);
        assert_eq!(applied.panels.state(Panel::SpotRemoval), PanelState::On);
    }
}
