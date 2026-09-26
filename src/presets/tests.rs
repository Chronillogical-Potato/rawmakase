use super::*;
use crate::develop::Recipe;
use anyhow::Result;
use std::fs::{self, File};
#[test]
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
fn embedded_profile_roundtrip_and_old_engine_pixels() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("profile.json");
    let m = crate::raw::Metadata {
        make: "Fujifilm".into(),
        model: "X100F".into(),
        cam_xyz: [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.5235],
        ],
        ..Default::default()
    };
    let recipe = Recipe {
        profile: crate::camera_profiles::CameraProfile::camera_matrix_default(&m)
            .map(std::sync::Arc::new),
        ..Recipe::for_metadata(&m)
    };
    assert!(recipe.profile.is_some());
    save_preset(&path, &recipe)?;
    assert_eq!(load_preset(&path)?, recipe);
    let mut legacy = Recipe {
        engine: 2,
        sharpening: 0.,
        exposure: 0.75,
        ..Default::default()
    };
    legacy.curve.insert([0.3, 0.2]);
    save_preset(&path, &legacy)?;
    let mut v: serde_json::Value = serde_json::from_reader(File::open(&path)?)?;
    v["schema"] = 2.into();
    v["pipeline"] = 2.into();
    for key in [
        "engine",
        "profile",
        "sharpening_radius",
        "sharpening_detail",
        "sharpening_masking",
    ] {
        v["recipe"].as_object_mut().unwrap().remove(key);
    }
    fs::write(&path, serde_json::to_vec(&v)?)?;
    let loaded = load_preset(&path)?;
    assert_eq!(loaded.engine, 2);
    assert_eq!(loaded.sharpening, 0.);
    assert!(loaded.profile.is_none());
    assert_eq!(loaded, legacy);
    Ok(())
}
