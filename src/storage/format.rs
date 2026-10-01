//! Versioning shared by native sidecars and recipe presets.
use anyhow::{Context, Result, ensure};
/// The version written. Spots and masks are saved apart from the recipe (see
/// `LocalEdits`), so recipes stay readable by releases that predate them.
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
                // Development builds of the retouch tools wrote 7 with the spots and
                // masks inside the recipe; they load as they are.
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
    /// The fields a freshly saved recipe writes, as of schema 6. Releases that read
    /// schema 6 keep fields they don't know, so adding a field is safe only when it
    /// is skipped at its default (`skip_serializing_if`) or when every release that
    /// may still read the file can be shown to tolerate it; either way, changing this
    /// list is a deliberate compatibility decision, not a side effect.
    const SAVED_FIELDS: &[&str] = &[
        "black_point",
        "blacks",
        "camera_exposure",
        "contrast",
        "crop",
        "curve",
        "effects",
        "engine",
        "exposure",
        "flip_x",
        "flip_y",
        "grading",
        "highlights",
        "hsl",
        "lens_builtin",
        "lens_ca",
        "lens_distortion",
        "lens_profile",
        "lens_vignetting",
        "midtone",
        "noise_chroma",
        "noise_luma",
        "preset_name",
        "preset_settings",
        "profile",
        "profile_tone",
        "reference_calibration",
        "reference_color",
        "reference_curves",
        "rotation",
        "saturation",
        "shadows",
        "sharpening",
        "sharpening_detail",
        "sharpening_masking",
        "sharpening_radius",
        "straighten",
        "temperature",
        "tint",
        "transform",
        "vibrance",
        "wb",
        "white_point",
        "whites",
        "wide_gamut_curves",
    ];
    /// Spots and masks never enter the saved recipe, which writes exactly the known
    /// fields; and fields from newer releases survive a round trip.
    #[test]
    fn saved_recipes_write_only_the_known_fields() {
        use crate::develop::{Recipe, retouch};
        let mut r = Recipe::default();
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
        let (saved, local) = r.split_local();
        let json = serde_json::to_value(&saved).unwrap();
        let written: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            written, SAVED_FIELDS,
            "a saved recipe's fields changed; see SAVED_FIELDS before updating it"
        );
        let mut v = serde_json::json!({"schema": super::SCHEMA, "pipeline": super::PIPELINE, "recipe": json});
        super::migrate_recipe(&mut v).unwrap();
        assert_eq!(v["schema"], 6);
        let back: Recipe = serde_json::from_value(v["recipe"].clone()).unwrap();
        assert_eq!(back.with_local(local), r);
        // A setting from a newer release is kept.
        let newer: Recipe =
            serde_json::from_value(serde_json::json!({"exposure": 0.5, "future_slider": [1, 2]}))
                .unwrap();
        assert_eq!(newer.exposure, 0.5);
        let again = serde_json::to_value(&newer).unwrap();
        assert_eq!(again["future_slider"], serde_json::json!([1, 2]));
        // A development build's schema 7, with spots in the recipe, still loads them.
        let mut v = serde_json::json!({"schema": 7, "pipeline": 7, "recipe": serde_json::to_value(&r).unwrap()});
        super::migrate_recipe(&mut v).unwrap();
        let old: Recipe = serde_json::from_value(v["recipe"].clone()).unwrap();
        assert_eq!(old.retouch, r.retouch);
    }
}
