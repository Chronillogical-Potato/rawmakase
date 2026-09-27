//! RAWmakase JSON recipes, including migration of earlier pipeline versions.
use crate::{
    develop::Recipe,
    storage::{atomic_json, migrate_recipe, versions},
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs::File, path::Path};

#[derive(Serialize, Deserialize)]
struct Preset {
    schema: u32,
    pipeline: u32,
    recipe: Recipe,
}
pub fn save_preset(path: &Path, r: &Recipe) -> Result<()> {
    r.validate()?;
    // Spot removal is specific to its photo; Lightroom presets never include it.
    let recipe = Recipe {
        retouch: Vec::new(),
        ..r.clone()
    };
    let (schema, pipeline) = versions(&recipe);
    atomic_json(
        path,
        &Preset {
            schema,
            pipeline,
            recipe,
        },
    )
}
pub fn load_preset(path: &Path) -> Result<Recipe> {
    let mut v: serde_json::Value = serde_json::from_reader(File::open(path)?)?;
    migrate_recipe(&mut v)?;
    let p: Preset = serde_json::from_value(v)?;
    p.recipe.validate()?;
    Ok(p.recipe)
}
