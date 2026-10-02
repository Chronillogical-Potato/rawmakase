use super::*;

fn xmp(description: &str, body: &str) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"
          xmlns:xmp="http://ns.adobe.com/xap/1.0/" {description}>{body}</rdf:Description></rdf:RDF></x:xmpmeta>"#
    )
}
/// A tiny JPEG with `packet` as its XMP.
fn jpeg_with(packet: &str) -> Result<Vec<u8>> {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode(
        &[0u8; 3],
        1,
        1,
        image::ExtendedColorType::Rgb8,
    )?;
    let mut payload = b"http://ns.adobe.com/xap/1.0/\0".to_vec();
    payload.extend_from_slice(packet.as_bytes());
    let mut out = jpeg[..2].to_vec();
    out.extend([0xff, 0xe1]);
    out.extend(((payload.len() + 2) as u16).to_be_bytes());
    out.extend(payload);
    out.extend_from_slice(&jpeg[2..]);
    Ok(out)
}
fn title(cat: &Catalog, id: i64) -> Result<Option<Value<LangAlt>>> {
    Ok(cat.descriptive(id)?.title)
}
fn set(text: &str) -> Option<Value<LangAlt>> {
    Some(Value::Set(LangAlt::new(text)))
}
fn id_of(cat: &Catalog, name: &str) -> Result<i64> {
    Ok(cat
        .photos()?
        .into_iter()
        .find(|p| p.filename == name)
        .map(|p| p.id)
        .unwrap())
}

#[test]
fn folder_import_reads_sidecars_by_digikams_names_and_the_jpegs_own_xmp() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    let w = |name: &str, bytes: &[u8]| std::fs::write(folder.join(name), bytes);
    // A RAW with both sidecars: digiKam's NEF.xmp wins.
    w("A.NEF", b"synthetic raw a")?;
    w(
        "A.NEF.xmp",
        xmp(r#"dc:title="From A.NEF.xmp""#, "").as_bytes(),
    )?;
    w("A.xmp", xmp(r#"dc:title="From A.xmp""#, "").as_bytes())?;
    // A RAW+JPEG pair: B.xmp is the RAW's; the JPEG reads its own XMP and
    // B.JPG.xmp, field by field.
    w("B.NEF", b"synthetic raw b")?;
    w("B.xmp", xmp(r#"dc:title="RAW's""#, "").as_bytes())?;
    w(
        "B.JPG",
        &jpeg_with(&xmp(
            r#"dc:title="Embedded" xmp:Rating="2""#,
            "<dc:description><rdf:Alt><rdf:li xml:lang=\"x-default\">Embedded caption</rdf:li></rdf:Alt></dc:description>\
             <dc:rights><rdf:Alt><rdf:li xml:lang=\"x-default\">© Embedded</rdf:li></rdf:Alt></dc:rights>",
        ))?,
    )?;
    w(
        "B.JPG.xmp",
        xmp(
            r#"dc:title="Sidecar""#,
            "<dc:description><rdf:Alt/></dc:description>",
        )
        .as_bytes(),
    )?;
    // A malformed sidecar is reported, and the rest goes on.
    w("C.NEF", b"synthetic raw c")?;
    w("C.NEF.xmp", b"<x:xmpmeta><rdf:RDF>")?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    let (added, report) = cat.add_folder_reporting(&folder)?;
    // Photos are kept by their canonical paths.
    let folder = folder.canonicalize()?;
    assert_eq!(added, 4);
    let (a, b_raw, b_jpg) = (
        id_of(&cat, "A.NEF")?,
        id_of(&cat, "B.NEF")?,
        id_of(&cat, "B.JPG")?,
    );
    assert_eq!(title(&cat, a)?, set("From A.NEF.xmp"));
    assert_eq!(report.ignored, [folder.join("A.xmp")]);
    assert_eq!(title(&cat, b_raw)?, set("RAW's"));
    let jpg = cat.descriptive(b_jpg)?;
    assert_eq!(jpg.title, set("Sidecar"));
    assert_eq!(jpg.caption, Some(Value::Cleared));
    assert_eq!(jpg.copyright, set("© Embedded"));
    assert_eq!(
        cat.photos()?.iter().find(|p| p.id == b_jpg).unwrap().rating,
        2
    );
    assert_eq!(report.unreadable.len(), 1);
    assert_eq!(report.unreadable[0].0, folder.join("C.NEF.xmp"));
    assert_eq!(
        report.summary().as_deref(),
        Some("1 sidecar could not be read · 1 sidecar ignored")
    );
    // Adding the folder again reads nothing of photos already there.
    std::fs::write(folder.join("A.NEF.xmp"), xmp(r#"dc:title="Changed""#, ""))?;
    cat.add_folder_reporting(&folder)?;
    assert_eq!(title(&cat, a)?, set("From A.NEF.xmp"));
    Ok(())
}

#[test]
fn read_metadata_from_files_overwrites_what_the_file_has_and_skips_copies() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("A.NEF"), b"synthetic raw")?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let id = id_of(&cat, "A.NEF")?;
    cat.set_text(&[id], TextField::Title, "My edit")?;
    cat.set_text(&[id], TextField::Caption, "My caption")?;
    let copy = cat.create_virtual_copy(id)?;
    std::fs::write(
        folder.join("A.NEF.xmp"),
        xmp(
            r#"dc:title="From the file" xmp:Rating="5""#,
            "<dc:subject><rdf:Bag><rdf:li>Read</rdf:li></rdf:Bag></dc:subject>",
        ),
    )?;
    let report = cat.read_metadata_from_files(&[id, copy])?;
    assert_eq!(report, SidecarReport::default());
    assert_eq!(title(&cat, id)?, set("From the file"));
    // A field the file lacks keeps the edit.
    assert_eq!(cat.descriptive(id)?.caption, set("My caption"));
    assert_eq!(cat.keywords(id)?[0].name, "Read");
    // The copy keeps its own.
    assert_eq!(title(&cat, copy)?, set("My edit"));
    assert!(cat.keywords(copy)?.is_empty());
    Ok(())
}

#[test]
fn utf16_sidecars_are_read() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("A.NEF");
    std::fs::write(&file, b"synthetic raw")?;
    let text = xmp(r#"dc:title="Zażółć""#, "");
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    std::fs::write(dir.path().join("A.NEF.xmp"), bytes)?;
    let (read, report) = read_file(&file);
    assert_eq!(report, SidecarReport::default());
    assert_eq!(read.unwrap().title, set("Zażółć"));
    Ok(())
}

#[test]
fn a_photo_whose_values_cant_be_written_is_reported_and_the_rest_imported() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("A.NEF"), b"synthetic raw a")?;
    std::fs::write(folder.join("B.NEF"), b"synthetic raw b")?;
    cat.add_folder(&folder)?;
    let (a, b) = (id_of(&cat, "A.NEF")?, id_of(&cat, "B.NEF")?);
    let bad = crate::xmp::descriptive::Read {
        // Not a date the catalog can sort by.
        capture: Some(crate::catalog::Capture {
            captured: "garbage".into(),
            subsec: None,
            offset: None,
        }),
        title: set("Bad"),
        ..Default::default()
    };
    let good = crate::xmp::descriptive::Read {
        title: set("Good"),
        ..Default::default()
    };
    let report = cat.apply_file_metadata(
        &[
            (a, folder.join("A.NEF.xmp"), bad),
            (b, folder.join("B.NEF.xmp"), good),
        ],
        false,
    )?;
    assert_eq!(report.unreadable.len(), 1);
    assert_eq!(title(&cat, a)?, None);
    assert_eq!(title(&cat, b)?, set("Good"));
    Ok(())
}

#[test]
fn jpeg_fill_bytes_before_a_marker_are_skipped() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let mut jpeg = jpeg_with(&xmp(r#"dc:title="Filled""#, ""))?;
    // Fill bytes before the XMP segment's marker.
    jpeg.splice(2..2, [0xff, 0xff, 0xff]);
    let file = dir.path().join("A.JPG");
    std::fs::write(&file, jpeg)?;
    let (read, _) = read_file(&file);
    assert_eq!(read.unwrap().title, set("Filled"));
    Ok(())
}
