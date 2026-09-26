use super::lightroom::develop_fields;
use super::*;
#[test]
#[ignore = "Requires private Lightroom catalog; set RAWMAKASE_LRCAT"]
fn supplied_catalog_is_preserved_and_all_images_import() -> Result<()> {
    let source = PathBuf::from(std::env::var("RAWMAKASE_LRCAT")?);
    let before = Identity::read(&source)?;
    let dir = tempfile::tempdir()?;
    let out = dir.path().join("import.rawmakase");
    import_lightroom(&source, &out)?;
    let c = Catalog::open(&out)?;
    let photos = c.photos()?;
    assert_eq!(photos.len(), 8112);
    assert_eq!(c.folders()?.len(), 275);
    assert_eq!(c.collections()?.len(), 13);
    let archive: Vec<u8> =
        c.db.query_row("SELECT original_catalog FROM sources", [], |r| r.get(0))?;
    assert_eq!(archive, std::fs::read(&source)?);
    assert_eq!(before, Identity::read(&source)?);
    for (id, make, model) in [(350644, "Sony", "ILCE-7M2"), (1062257, "Fujifilm", "X100F")] {
        let m = crate::raw::Metadata {
            make: make.into(),
            model: model.into(),
            ..Default::default()
        };
        let (profiles, _) = crate::camera_profiles::installed(&m);
        let text = c
            .lightroom_develop(id)?
            .context("Missing representative settings")?;
        let (recipe, warnings) = convert_develop(&text, &m, &profiles, None)?;
        recipe.validate()?;
        println!(
            "{model}: compatible Develop recipe validated; {} reported limitations",
            warnings.len()
        );
    }
    let mut valid = 0;
    let mut unsupported = 0;
    for p in photos {
        if let Some(text) = c.lightroom_develop(p.id)? {
            match develop_fields(&text) {
                Ok(_) => valid += 1,
                Err(_) => unsupported += 1,
            }
        }
    }
    println!(
        "8112 photos; 275 folders; 13 collections; Develop table parser: {valid} readable, {unsupported} preserved opaque"
    );
    assert!(valid > 8000);
    Ok(())
}
