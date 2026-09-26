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
