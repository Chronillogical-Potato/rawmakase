use super::lightroom::develop_fields;
use super::*;
fn fixture(path: &Path) -> Result<()> {
    let db = Connection::open(path)?;
    db.execute_batch("CREATE TABLE AgLibraryRootFolder(id_local INTEGER, absolutePath TEXT);
        CREATE TABLE AgLibraryFolder(id_local INTEGER,rootFolder INTEGER,pathFromRoot TEXT);
        CREATE TABLE AgLibraryFile(id_local INTEGER,folder INTEGER,idx_filename TEXT,baseName TEXT,extension TEXT);
        CREATE TABLE Adobe_images(id_local INTEGER,rootFile INTEGER,captureTime TEXT,rating INTEGER,pick INTEGER,colorLabels TEXT,fileFormat TEXT,copyName TEXT,masterImage INTEGER,orientation TEXT);
        CREATE TABLE Adobe_imageDevelopSettings(image INTEGER,text TEXT);
        CREATE TABLE AgLibraryCollection(id_local INTEGER,name TEXT,parent INTEGER,creationId TEXT);
        CREATE TABLE AgLibraryCollectionImage(collection INTEGER,image INTEGER,positionInCollection TEXT);
        CREATE TABLE AgLibraryKeyword(id_local INTEGER,name TEXT,parent INTEGER);
        CREATE TABLE AgLibraryKeywordImage(image INTEGER,tag INTEGER);
        CREATE TABLE ProprietaryData(blob BLOB);
        INSERT INTO ProprietaryData VALUES(X'001122FF');
        INSERT INTO AgLibraryRootFolder VALUES(10,'/Volumes/Photos/');
        INSERT INTO AgLibraryFolder VALUES(20,10,'Trip/'),(21,10,'Trip/Day2/');
        INSERT INTO AgLibraryFile VALUES(30,20,'image.ARW','image','ARW');
        INSERT INTO Adobe_images VALUES(40,30,'2021-06-06T10:00:00',4,1,'Red','RAW','',NULL,'AB'),(41,30,'2021-06-06T10:00:00',2,-1,'Blue','RAW','B&W',40,'AB');
        INSERT INTO Adobe_imageDevelopSettings VALUES(40,'s = { Exposure2012 = 1.5 }'),(41,'s = { ConvertToGrayscale = true }');
        INSERT INTO AgLibraryCollection VALUES(50,'Travel',NULL,'com.adobe.ag.library.collection');
        INSERT INTO AgLibraryCollectionImage VALUES(50,40,'a'),(50,41,'b');
        INSERT INTO AgLibraryKeyword VALUES(60,'City',NULL);
        INSERT INTO AgLibraryKeywordImage VALUES(40,60);")?;
    Ok(())
}
#[test]
fn lightroom_metadata_preserves_all_labels_flags_and_unrated_photos() -> Result<()> {
    let d = tempfile::tempdir()?;
    let source = d.path().join("metadata.lrcat");
    fixture(&source)?;
    {
        let db = Connection::open(&source)?;
        db.execute(
            "UPDATE Adobe_images SET rating=NULL,pick=NULL,colorLabels=NULL WHERE id_local=40",
            [],
        )?;
        for (i, label) in [
            "Red",
            "Yellow",
            "Green",
            "Blue",
            "Purple",
            "Client approved",
            "Czerwony",
        ]
        .iter()
        .enumerate()
        {
            db.execute("INSERT INTO Adobe_images VALUES(?,30,'2021-06-06T10:00:00',?,?,?,'RAW','Virtual',40,'AB')",
                    params![100 + i as i64, (i % 6) as f64, (i as i32 % 3 - 1) as f64, label])?;
        }
    }
    let original = std::fs::read(&source)?;
    let destination = d.path().join("metadata.rawmakase");
    import_lightroom(&source, &destination)?;
    let cat = Catalog::open(&destination)?;
    let photos = cat.photos()?;
    let unrated = photos.iter().find(|p| p.id == 40).unwrap();
    assert_eq!(
        (unrated.rating, unrated.flag, unrated.label.as_str()),
        (0, 0, "")
    );
    for (i, label) in [
        "Red",
        "Yellow",
        "Green",
        "Blue",
        "Purple",
        "Client approved",
        "Czerwony",
    ]
    .iter()
    .enumerate()
    {
        let photo = photos.iter().find(|p| p.id == 100 + i as i64).unwrap();
        assert_eq!(
            (photo.rating, photo.flag, photo.label.as_str()),
            ((i % 6) as i32, i as i32 % 3 - 1, *label)
        );
    }
    cat.set_metadata(100, 5, 1, "Purple")?;
    assert!(cat.set_metadata(100, 6, 1, "Red").is_err());
    assert!(cat.set_metadata(100, 0, 2, "Red").is_err());
    assert!(cat.set_metadata(9999, 0, 0, "").is_err());
    drop(cat);
    let reopened = Catalog::open(&destination)?;
    let photos = reopened.photos()?;
    let edited = photos.iter().find(|p| p.id == 100).unwrap();
    assert_eq!(
        (edited.rating, edited.flag, edited.label.as_str()),
        (5, 1, "Purple")
    );
    assert_eq!(photos.iter().find(|p| p.id == 40).unwrap().rating, 0);
    assert_eq!(photos.iter().find(|p| p.id == 101).unwrap().label, "Yellow");
    assert_eq!(std::fs::read(&source)?, original);
    Ok(())
}
#[test]
fn import_is_lossless_atomic_and_virtual_copies_are_independent() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.lrcat");
    fixture(&source)?;
    let bytes = std::fs::read(&source)?;
    let identity = Identity::read(&source)?;
    let output = dir.path().join("Photos.rawmakase");
    import_lightroom(&source, &output)?;
    assert_eq!(identity, Identity::read(&source)?);
    assert_eq!(bytes, std::fs::read(&source)?);
    let cat = Catalog::open(&output)?;
    let archive: Vec<u8> = cat
        .db
        .query_row("SELECT original_catalog FROM sources", [], |r| r.get(0))?;
    assert_eq!(archive, bytes);
    let photos = cat.photos()?;
    assert_eq!(photos.len(), 2);
    assert_eq!(photos[0].rating, 4);
    assert_eq!(photos[0].keywords, "City");
    assert_eq!(photos[1].copy_name, "B&W");
    assert_eq!(cat.collection_members(50)?.len(), 2);
    let local = dir.path().join("local");
    std::fs::create_dir(&local)?;
    std::fs::write(local.join("image.ARW"), b"synthetic raw identity")?;
    cat.relink_folder(20, &local)?;
    assert_eq!(
        cat.folders()?.iter().find(|f| f.id == 21).unwrap().path,
        local.join("Day2/")
    );
    let p = cat.photos()?[0].path.clone();
    let edit = Recipe {
        exposure: 1.25,
        ..Default::default()
    };
    cat.save_edit(40, &p, &edit, &ExportOptions::default())?;
    assert_eq!(cat.load_edit(40, &p)?.unwrap().recipe, edit);
    assert!(cat.load_edit(41, &p)?.is_none());
    assert!(!crate::storage::sidecar_path(&p).exists());
    cat.set_metadata(41, 5, 1, "Purple")?;
    assert_eq!(cat.photos()?[0].rating, 4);
    assert_eq!(cat.photos()?[1].rating, 5);
    std::fs::write(&p, b"changed raw")?;
    assert!(cat.load_edit(40, &p).is_err());
    assert!(
        cat.save_edit(40, &p, &edit, &ExportOptions::default())
            .is_err()
    );
    let size = output.metadata()?.len();
    assert!(import_lightroom(&source, &output).is_err());
    assert_eq!(size, output.metadata()?.len());
    assert_eq!(bytes, std::fs::read(&source)?);
    Ok(())
}
#[test]
fn bad_imports_leave_no_destination_and_future_catalogs_are_rejected() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("bad.lrcat");
    std::fs::write(&source, b"not sqlite")?;
    let dest = dir.path().join("bad.rawmakase");
    assert!(import_lightroom(&source, &dest).is_err());
    assert!(!dest.exists());
    std::fs::remove_file(&source)?;
    fixture(&source)?;
    std::fs::write(dir.path().join("bad.lrcat-wal"), b"active")?;
    assert!(import_lightroom(&source, &dest).is_err());
    assert!(!dest.exists());
    std::fs::remove_file(dir.path().join("bad.lrcat-wal"))?;
    let db = Connection::open(&source)?;
    db.execute(
        "INSERT INTO Adobe_images(id_local,rootFile) VALUES(99,999)",
        [],
    )?;
    drop(db);
    assert!(import_lightroom(&source, &dest).is_err());
    assert!(!dest.exists());
    let cat = Catalog::create(&dest)?;
    cat.db.execute_batch("PRAGMA user_version=999")?;
    drop(cat);
    assert!(Catalog::open(&dest).is_err());
    Ok(())
}
#[test]
fn folder_import_is_idempotent_and_does_not_touch_photos() -> Result<()> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("test.RAF"), b"test")?;
    std::fs::write(photos.join("note.txt"), b"ignore")?;
    // macOS AppleDouble metadata written on exFAT/FAT drives.
    std::fs::write(photos.join("._test.RAF"), b"metadata")?;
    let mut cat = Catalog::create(&d.path().join("new.rawmakase"))?;
    assert_eq!(cat.add_folder(&photos)?, 1);
    assert_eq!(cat.add_folder(&photos)?, 0);
    assert_eq!(cat.photos()?.len(), 1);
    assert_eq!(std::fs::read(photos.join("test.RAF"))?, b"test");
    Ok(())
}
#[test]
fn lightroom_table_parser_never_executes_and_reports_unsupported_edits() -> Result<()> {
    let text = r#"s = { Exposure2012 = 1.25, Contrast2012 = 15, ConvertToGrayscale = true, ToneCurvePV2012 = { 0, 12, 255, 255 }, PerspectiveUpright = 1, RetouchInfo = { { x = 0.5, y = 0.4 } }, CameraProfile = "Missing, {profile}" }"#;
    let (r, w) = convert_develop(text, &crate::raw::Metadata::default(), &[], None)?;
    assert_eq!(r.exposure, 1.25);
    assert!(r.effects.monochrome);
    assert_eq!(r.curve.points[0], [0., 12. / 255.]);
    assert!(w.iter().any(|s| s.contains("Missing")));
    assert!(w.iter().any(|s| s.contains("PerspectiveUpright")));
    // An incomplete spot is reported, not rendered.
    assert!(w.iter().any(|s| s.starts_with("Spot 1")), "{w:?}");
    assert!(r.retouch.is_empty());
    assert!(
        convert_develop(
            "s = { Exposure2012 = os.execute(\"bad\") }",
            &crate::raw::Metadata::default(),
            &[],
            None
        )
        .is_err()
    );
    assert!(develop_fields("s = { a = 1, a = 2 }").is_err());
    Ok(())
}
#[test]
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
fn lightroom_edits_fall_back_to_rawmakase_profiles() -> Result<()> {
    use crate::camera_profiles::open;
    let m = crate::raw::Metadata {
        make: "Fujifilm".into(),
        model: "X100F".into(),
        wb: [2.0198677, 1., 1.8874172],
        cam_xyz: [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.5235],
        ],
        ..Default::default()
    };
    let profiles: Vec<_> = [open::standard(&m), open::color(&m)]
        .into_iter()
        .flatten()
        .map(std::sync::Arc::new)
        .collect();
    for (asked, used) in [
        ("Adobe Standard", open::STANDARD),
        ("Adobe Color", open::COLOR),
    ] {
        let text = format!(r#"s = {{ Exposure2012 = 0.5, CameraProfile = "{asked}" }}"#);
        let (r, w) = convert_develop(&text, &m, &profiles, None)?;
        assert_eq!(r.exposure, 0.5);
        assert_eq!(r.profile.as_ref().unwrap().name, used);
        assert!(w.iter().any(|s| s.contains(used)), "{w:?}");
    }
    // Other Adobe looks aren't substituted.
    let text = r#"s = { CameraProfile = "Adobe Vivid" }"#;
    let (r, w) = convert_develop(text, &m, &profiles, None)?;
    assert_eq!(r.profile.as_ref().unwrap().name, open::COLOR);
    assert!(w.iter().any(|s| s.contains("Adobe Vivid")), "{w:?}");
    Ok(())
}
#[test]
fn lightroom_history_text_decodes_plain_and_compressed() {
    use std::io::Write;
    let text = "s = { Exposure2012 = 0.5 }";
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(text.as_bytes()).unwrap();
    let mut blob = (text.len() as u32).to_be_bytes().to_vec();
    blob.extend(z.finish().unwrap());
    assert_eq!(super::decode_history_text(&blob).as_deref(), Some(text));
    assert_eq!(
        super::decode_history_text(text.as_bytes()).as_deref(),
        Some(text)
    );
}
#[test]
fn process_version_2010_edits_keep_exposure_and_report_the_rest() -> Result<()> {
    let text = r#"s = { ProcessVersion = "5.7", Exposure = 0.75, Contrast = 40, Brightness = 50, Clarity = 0 }"#;
    let (r, w) = convert_develop(text, &crate::raw::Metadata::default(), &[], None)?;
    assert_eq!(r.exposure, 0.75);
    assert!(w.iter().any(|s| s.starts_with("Contrast")));
    // Controls at their legacy defaults are not reported.
    assert!(
        !w.iter()
            .any(|s| s.starts_with("Brightness") || s.starts_with("Clarity"))
    );
    // With 2012 keys present the legacy ones are ignored.
    let text = r#"s = { ProcessVersion = "11.0", Exposure = 0.75, Exposure2012 = 0.25 }"#;
    let (r, _) = convert_develop(text, &crate::raw::Metadata::default(), &[], None)?;
    assert_eq!(r.exposure, 0.25);
    Ok(())
}
#[test]
fn bitmaps_are_stored_once_by_hash() -> Result<()> {
    let d = tempfile::tempdir()?;
    let catalog = Catalog::create(&d.path().join("bitmaps.rawmakase"))?;
    let bitmap = crate::storage::bitmaps::Bitmap {
        width: 4,
        height: 2,
        channels: 1,
        depth: 1,
        data: vec![0, 64, 128, 255, 1, 2, 3, 4],
    };
    let hash = catalog.put_bitmap(&bitmap)?;
    assert_eq!(catalog.put_bitmap(&bitmap)?, hash);
    assert_eq!(catalog.bitmap(&hash)?, Some(bitmap));
    assert_eq!(catalog.bitmap("missing")?, None);
    Ok(())
}
#[test]
fn catalog_keeps_spots_and_masks_out_of_the_recipe_column() -> Result<()> {
    use crate::develop::masks;
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"identity fixture")?;
    let mut c = Catalog::create(&d.path().join("local.rawmakase"))?;
    c.add_folder(&photos)?;
    let id = c.photos()?[0].id;
    let mut r = Recipe::default();
    r.masks.push(masks::MaskGroup {
        components: vec![masks::MaskComponent::new(masks::MaskShape::Radial {
            center: [0.5, 0.5],
            radii: [0.2, 0.1],
            angle: 0.,
            feather: 0.5,
        })],
        adjust: masks::LocalAdjust {
            shadows: 0.5,
            ..Default::default()
        },
        ..Default::default()
    });
    c.save_edit(id, &photo, &r, &ExportOptions::default())?;
    let column: String =
        c.db.query_row("SELECT recipe FROM photos WHERE id=?", [id], |row| {
            row.get(0)
        })?;
    assert!(!column.contains("masks"));
    assert_eq!(c.load_edit(id, &photo)?.unwrap().recipe, r);
    let (text, _) = c.edit_texts(id)?;
    assert_eq!(serde_json::from_str::<Recipe>(&text.unwrap())?, r);
    c.save_edit(id, &photo, &Recipe::default(), &ExportOptions::default())?;
    assert!(c.load_edit(id, &photo)?.unwrap().recipe.masks.is_empty());
    Ok(())
}
#[test]
fn adding_a_folder_imports_sidecar_edits() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    let edited = folder.join("edited.dng");
    let plain = folder.join("plain.dng");
    std::fs::write(&edited, b"synthetic raw identity")?;
    std::fs::write(&plain, b"another synthetic raw")?;
    let edit = Recipe {
        exposure: 0.75,
        ..Default::default()
    };
    crate::storage::save(&edited, &edit, &ExportOptions::default())?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    assert_eq!(cat.add_folder(&folder)?, 2);
    let photos = cat.photos()?;
    let find = |name: &str| photos.iter().find(|p| p.path.ends_with(name)).unwrap();
    let (edited_photo, plain_photo) = (find("edited.dng"), find("plain.dng"));
    assert_eq!(
        cat.load_edit(edited_photo.id, &edited_photo.path)?
            .unwrap()
            .recipe,
        edit
    );
    assert!(cat.load_edit(plain_photo.id, &plain_photo.path)?.is_none());
    // The sidecar is left as it was.
    assert!(crate::storage::sidecar_path(&edited).exists());
    Ok(())
}
