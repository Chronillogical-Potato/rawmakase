//! Versioning shared by native sidecars and recipe presets.
use anyhow::{Context, Result, ensure};
pub const SCHEMA: u32 = 7;
pub const PIPELINE: u32 = 7;
/// The version written for recipes that do not use schema 7's retouching and masks,
/// so releases that predate them (which reject unknown fields) can still read them.
const COMPATIBLE: u32 = 6;

/// The schema and pipeline versions to write `recipe` with.
pub(crate) fn versions(recipe: &crate::develop::Recipe) -> (u32, u32) {
    if recipe.uses_local_tools() {
        (SCHEMA, PIPELINE)
    } else {
        (COMPATIBLE, COMPATIBLE)
    }
}

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
                | (Some(7), Some(7))
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
        let mut v = serde_json::json!({"schema": 7, "pipeline": 7, "recipe": {"retouch": []}});
        super::migrate_recipe(&mut v).unwrap();
        assert!(v["recipe"].get("engine").is_none());
    }
    #[test]
    fn only_recipes_with_local_tools_need_schema_seven() {
        use crate::develop::{Recipe, retouch};
        let mut r = Recipe::default();
        assert_eq!(super::versions(&r), (6, 6));
        let json = serde_json::to_value(&r).unwrap();
        assert!(json.get("retouch").is_none() && json.get("masks").is_none());
        r.retouch.push(retouch::RetouchOp {
            mode: retouch::RetouchMode::Heal,
            shape: retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.01,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.05, 0.],
        });
        assert_eq!(super::versions(&r), (7, 7));
        let back: Recipe = serde_json::from_value(serde_json::to_value(&r).unwrap()).unwrap();
        assert_eq!(back, r);
    }
}
