//! RAWmakase JSON recipes, including migration of earlier pipeline versions.
use crate::{
    develop::Recipe,
    storage::{atomic_json, migrate_recipe, saved_version},
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
        masks: p.masks,
    });
    recipe.upright.clear_analysis();
    recipe.validate()?;
    Ok(recipe)
}
