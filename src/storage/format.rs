//! Versioning shared by native sidecars and recipe presets.
use anyhow::{Context, Result, ensure};
pub const SCHEMA: u32 = 6;
pub const PIPELINE: u32 = 6;

pub(crate) fn migrate_recipe(value: &mut serde_json::Value) -> Result<()> {
    let schema = value["schema"].as_u64();
    let pipeline = value["pipeline"].as_u64();
    ensure!(
        matches!(
            (schema, pipeline),
            (Some(1), Some(1))
                | (Some(2), Some(2))
                | (Some(3), Some(3))
                | (Some(4), Some(4))
                | (Some(5), Some(5))
                | (Some(6), Some(6))
        ),
        "Unsupported saved recipe version: preserved without changes"
    );
    let recipe = value
        .get_mut("recipe")
        .and_then(serde_json::Value::as_object_mut)
        .context("Saved recipe must be an object")?;
    if pipeline.is_some_and(|p| p < 6) {
        // Engine 4 changed the default rendering; earlier recipes keep their look.
        recipe.entry("engine").or_insert(3.into());
    }
    if pipeline.is_some_and(|p| p < 3) {
        recipe.insert("engine".into(), 2.into());
        recipe.insert("profile".into(), serde_json::Value::Null);
        recipe.entry("sharpening").or_insert(0.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn pre_engine_four_recipes_keep_their_engine() {
        let mut v = serde_json::json!({"schema": 5, "pipeline": 5, "recipe": {"exposure": 0.5}});
        super::migrate_recipe(&mut v).unwrap();
        assert_eq!(v["recipe"]["engine"], 3);
        let mut v = serde_json::json!({"schema": 5, "pipeline": 5, "recipe": {"engine": 3}});
        super::migrate_recipe(&mut v).unwrap();
        assert_eq!(v["recipe"]["engine"], 3);
        let mut v = serde_json::json!({"schema": 6, "pipeline": 6, "recipe": {}});
        super::migrate_recipe(&mut v).unwrap();
        assert!(v["recipe"].get("engine").is_none());
    }
}
