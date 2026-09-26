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
    assert!(w.contains(&"RetouchInfo".into()));
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
