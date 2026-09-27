use super::*;
use crate::presets::{load_preset, save_preset};
#[test]
fn legacy_sidecar_and_preset_migrate_without_curve_changes() -> Result<()> {
    let d = tempfile::tempdir()?;
    let raw = d.path().join("legacy.ARW");
    fs::write(&raw, b"fixture")?;
    let store = d.path().join("store");
    let p = save_at(&raw, &Recipe::default(), &ExportOptions::default(), &store)?;
    let mut v: serde_json::Value = serde_json::from_reader(File::open(&p)?)?;
    v["schema"] = 1.into();
    v["pipeline"] = 1.into();
    v["recipe"]["curve"] = serde_json::json!([0.1, 0.2, 0.5, 0.9, 1.]);
    fs::write(&p, serde_json::to_vec(&v)?)?;
    let loaded = load_at(&raw, &store)?.unwrap();
    assert!(!loaded.recipe.curve.smooth);
    save_at(&raw, &loaded.recipe, &loaded.export, &store)?;
    assert_eq!(load_at(&raw, &store)?.unwrap().recipe, loaded.recipe);
    let preset = d.path().join("preset.json");
    fs::write(
        &preset,
        serde_json::to_vec(&serde_json::json!({"schema":1,"pipeline":1,"recipe":v["recipe"]}))?,
    )?;
    assert_eq!(load_preset(&preset)?, loaded.recipe);
    save_preset(&preset, &loaded.recipe)?;
    assert_eq!(load_preset(&preset)?, loaded.recipe);
    Ok(())
}
#[test]
fn readonly_folder_uses_fallback_and_restores() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir()?;
    let store = tempfile::tempdir()?;
    let p = dir.path().join("photo.ARW");
    fs::write(&p, b"raw")?;
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555))?;
    // Root (as in CI containers) ignores directory permissions; nothing to test then.
    if fs::write(dir.path().join("probe"), b"").is_ok() {
        return Ok(());
    }
    let result = save_at(
        &p,
        &Recipe::default(),
        &ExportOptions::default(),
        store.path(),
    );
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755))?;
    let output = result?;
    assert!(output.starts_with(store.path()));
    assert!(load_at(&p, store.path())?.is_some());
    Ok(())
}
#[test]
fn sidecar_roundtrip_and_protection() -> Result<()> {
    let d = tempfile::tempdir()?;
    let raw = d.path().join("test.ARW");
    fs::write(&raw, b"fixture")?;
    let r = Recipe {
        exposure: 1.25,
        ..Default::default()
    };
    let p = save(&raw, &r, &ExportOptions::default())?;
    assert_eq!(load(&raw)?.unwrap().recipe, r);
    assert!(p.ends_with("test.ARW.rawmakase.json"));
    let mut v: serde_json::Value = serde_json::from_reader(File::open(&p)?)?;
    v["schema"] = 999.into();
    fs::write(&p, serde_json::to_vec(&v)?)?;
    assert!(save(&raw, &r, &ExportOptions::default()).is_err());
    assert_eq!(
        serde_json::from_reader::<_, serde_json::Value>(File::open(&p)?)?["schema"],
        999
    );
    Ok(())
}
#[test]
fn changed_source_refused() -> Result<()> {
    let d = tempfile::tempdir()?;
    let p = d.path().join("x.RAF");
    fs::write(&p, b"one")?;
    save(&p, &Recipe::default(), &ExportOptions::default())?;
    fs::write(&p, b"two longer")?;
    assert!(load(&p).is_err());
    Ok(())
}
/// Spots and masks go to the companion file, so the sidecar itself stays a schema 6
/// recipe that releases before them read; they load back into the recipe.
#[test]
fn spots_and_masks_save_beside_a_compatible_sidecar() -> Result<()> {
    use crate::develop::{masks, retouch};
    let d = tempfile::tempdir()?;
    let raw = d.path().join("photo.ARW");
    fs::write(&raw, b"fixture")?;
    let store = d.path().join("store");
    let mut r = Recipe {
        exposure: 0.4,
        ..Default::default()
    };
    r.retouch.push(retouch::RetouchOp {
        mode: retouch::RetouchMode::Clone,
        shape: retouch::RetouchShape::Spot {
            center: [0.4, 0.5],
            radius: 0.02,
        },
        feather: 0.5,
        opacity: 1.,
        offset: [0.1, 0.],
    });
    r.masks.push(masks::MaskGroup {
        components: vec![masks::MaskComponent::new(masks::MaskShape::Linear {
            from: [0.5, 0.],
            to: [0.5, 0.5],
        })],
        adjust: masks::LocalAdjust {
            exposure: -1.,
            ..Default::default()
        },
        ..Default::default()
    });
    let p = save_at(&raw, &r, &ExportOptions::default(), &store)?;
    let v: serde_json::Value = serde_json::from_reader(File::open(&p)?)?;
    assert_eq!(
        (v["schema"].as_u64(), v["pipeline"].as_u64()),
        (Some(6), Some(6))
    );
    let recipe = v["recipe"].as_object().unwrap();
    assert!(!recipe.contains_key("retouch") && !recipe.contains_key("masks"));
    assert_eq!(recipe["exposure"], 0.4);
    assert!(local_path(&raw).exists());
    assert_eq!(load_at(&raw, &store)?.unwrap().recipe, r);
    // Unknown top-level fields of a newer release survive a save.
    let mut v = v;
    v["future"] = serde_json::json!({"x": 1});
    fs::write(&p, serde_json::to_vec(&v)?)?;
    let loaded = load_at(&raw, &store)?.unwrap();
    save_at(&raw, &loaded.recipe, &loaded.export, &store)?;
    let v: serde_json::Value = serde_json::from_reader(File::open(&p)?)?;
    assert_eq!(v["future"]["x"], 1);
    // Without spots and masks the companion goes away.
    save_at(&raw, &Recipe::default(), &ExportOptions::default(), &store)?;
    assert!(!local_path(&raw).exists());
    // A development build's schema 7 sidecar, with them in the recipe, still loads.
    let mut v: serde_json::Value = serde_json::from_reader(File::open(&p)?)?;
    v["schema"] = 7.into();
    v["pipeline"] = 7.into();
    v["recipe"] = serde_json::to_value(&r)?;
    fs::write(&p, serde_json::to_vec(&v)?)?;
    assert_eq!(load_at(&raw, &store)?.unwrap().recipe, r);
    Ok(())
}
