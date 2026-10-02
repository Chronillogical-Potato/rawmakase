use super::*;

/// A sidecar as digiKam writes one, by the property names its library uses
/// (Xmp.digiKam.TagsList, ColorLabel, PickLabel, Xmp.lr.hierarchicalSubject…).
pub(crate) const DIGIKAM: &str = r#"<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="XMP Core 4.4.0-Exiv2">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:exif="http://ns.adobe.com/exif/1.0/"
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:digiKam="http://www.digikam.org/ns/1.0/"
    xmlns:MicrosoftPhoto="http://ns.microsoft.com/photo/1.0/"
    xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
   exif:DateTimeOriginal="2024-05-01T12:30:15.120456+02:00"
   exif:GPSLatitude="50,3.7302N"
   exif:GPSLongitude="19,56.2122E"
   exif:GPSAltitude="2195/10"
   exif:GPSAltitudeRef="0"
   xmp:Rating="4"
   digiKam:ColorLabel="4"
   digiKam:PickLabel="3"
   MicrosoftPhoto:Rating="75"
   photoshop:DateCreated="2024-05-01T12:30:15">
   <digiKam:TagsList>
    <rdf:Seq>
     <rdf:li>Places/Poland/Kraków</rdf:li>
     <rdf:li>People/Zoë</rdf:li>
     <rdf:li>猫</rdf:li>
    </rdf:Seq>
   </digiKam:TagsList>
   <dc:subject>
    <rdf:Bag>
     <rdf:li>Kraków</rdf:li>
     <rdf:li>Zoë</rdf:li>
     <rdf:li>猫</rdf:li>
    </rdf:Bag>
   </dc:subject>
   <dc:title>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Market square</rdf:li>
     <rdf:li xml:lang="pl-PL">Rynek</rdf:li>
    </rdf:Alt>
   </dc:title>
   <dc:description>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">Morning, before the crowds 🌅</rdf:li>
    </rdf:Alt>
   </dc:description>
   <dc:creator>
    <rdf:Seq>
     <rdf:li>Zoë Example</rdf:li>
     <rdf:li>Second Person</rdf:li>
    </rdf:Seq>
   </dc:creator>
   <dc:rights>
    <rdf:Alt>
     <rdf:li xml:lang="x-default">© 2024 Example</rdf:li>
    </rdf:Alt>
   </dc:rights>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

#[test]
fn reads_a_digikam_sidecar() -> Result<()> {
    let r = read(DIGIKAM)?;
    assert_eq!(
        r.title,
        Some(Value::Set(LangAlt(vec![
            ("x-default".into(), "Market square".into()),
            ("pl-PL".into(), "Rynek".into()),
        ])))
    );
    assert_eq!(
        r.caption,
        Some(Value::Set(LangAlt::new("Morning, before the crowds 🌅")))
    );
    assert_eq!(
        r.creator,
        Some(Value::Set(vec![
            "Zoë Example".into(),
            "Second Person".into()
        ]))
    );
    assert_eq!(
        r.copyright,
        Some(Value::Set(LangAlt::new("© 2024 Example")))
    );
    assert_eq!(
        r.keywords,
        Some(vec![
            vec!["Places".to_string(), "Poland".into(), "Kraków".into()],
            vec!["People".to_string(), "Zoë".into()],
            vec!["猫".to_string()],
        ])
    );
    assert_eq!(r.rating, Some(4));
    assert_eq!(r.label.as_deref(), Some("Green"));
    assert_eq!(r.flag, Some(1));
    assert_eq!(
        r.capture,
        Some(Capture {
            captured: "2024-05-01T12:30:15".into(),
            subsec: Some("120456".into()),
            offset: Some("+02:00".into()),
        })
    );
    let Some(Location::At { lat, lon, alt }) = r.location else {
        panic!("no location");
    };
    assert!((lat - (50. + 3.7302 / 60.)).abs() < 1e-9);
    assert!((lon - (19. + 56.2122 / 60.)).abs() < 1e-9);
    assert_eq!(alt, Some(219.5));
    Ok(())
}

#[test]
fn lightroom_paths_win_and_flat_names_added_later_join_at_the_top() -> Result<()> {
    let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"
        xmlns:lr="http://ns.adobe.com/lightroom/1.0/" xmlns:digiKam="http://www.digikam.org/ns/1.0/"
        xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:photoshop="http://ns.adobe.com/photoshop/1.0/"
        dc:title="Plain title" xmp:Label="Red" photoshop:DateCreated="2019-12-31T23:59:58Z">
        <lr:hierarchicalSubject><rdf:Bag>
          <rdf:li>Places|AC/DC</rdf:li><rdf:li>Top</rdf:li>
        </rdf:Bag></lr:hierarchicalSubject>
        <digiKam:TagsList><rdf:Seq><rdf:li>Ignored/Here</rdf:li></rdf:Seq></digiKam:TagsList>
        <dc:subject><rdf:Bag>
          <rdf:li>Places</rdf:li><rdf:li>AC/DC</rdf:li><rdf:li>Top</rdf:li><rdf:li>Added, later</rdf:li>
        </rdf:Bag></dc:subject>
      </rdf:Description></rdf:RDF></x:xmpmeta>"#;
    let r = read(text)?;
    assert_eq!(r.title, Some(Value::Set(LangAlt::new("Plain title"))));
    assert_eq!(r.label.as_deref(), Some("Red"));
    assert_eq!(
        r.keywords,
        Some(vec![
            vec!["Places".to_string(), "AC/DC".into()],
            vec!["Top".to_string()],
            vec!["Added, later".to_string()],
        ])
    );
    assert_eq!(
        r.capture.unwrap(),
        Capture {
            captured: "2019-12-31T23:59:58".into(),
            subsec: None,
            offset: Some("+00:00".into()),
        }
    );
    Ok(())
}

#[test]
fn explicitly_empty_values_are_cleared_and_absent_ones_are_not_read() -> Result<()> {
    let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" dc:rights="">
        <dc:title><rdf:Alt/></dc:title>
        <dc:creator><rdf:Seq></rdf:Seq></dc:creator>
        <dc:subject><rdf:Bag/></dc:subject>
      </rdf:Description></rdf:RDF></x:xmpmeta>"#;
    let r = read(text)?;
    assert_eq!(r.title, Some(Value::Cleared));
    assert_eq!(r.copyright, Some(Value::Cleared));
    assert_eq!(r.creator, Some(Value::Cleared));
    assert_eq!(r.keywords, Some(vec![]));
    assert_eq!(
        (r.caption, r.rating, r.capture, r.location),
        (None, None, None, None)
    );
    Ok(())
}

#[test]
fn malformed_xmp_is_an_error() {
    assert!(read("<x:xmpmeta><rdf:RDF>").is_err());
    assert!(read("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>").is_err());
}

#[test]
fn a_sidecar_value_wins_and_its_empty_ones_suppress_the_files() {
    let sidecar = Read {
        title: Some(Value::Set(LangAlt::new("Sidecar"))),
        caption: Some(Value::Cleared),
        ..Default::default()
    };
    let inside = Read {
        title: Some(Value::Set(LangAlt::new("Embedded"))),
        caption: Some(Value::Set(LangAlt::new("Embedded caption"))),
        copyright: Some(Value::Set(LangAlt::new("© Embedded"))),
        ..Default::default()
    };
    let r = sidecar.or(inside);
    assert_eq!(r.title, Some(Value::Set(LangAlt::new("Sidecar"))));
    assert_eq!(r.caption, Some(Value::Cleared));
    assert_eq!(r.copyright, Some(Value::Set(LangAlt::new("© Embedded"))));
}

#[test]
fn an_empty_rating_is_none_and_an_impossible_date_is_not_read() -> Result<()> {
    let text = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
      <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
        xmlns:exif="http://ns.adobe.com/exif/1.0/" xmp:Rating=""
        exif:DateTimeOriginal="0000-00-00T00:00:00" xmp:CreateDate="2020-02-02T10:00"/>
      </rdf:RDF></x:xmpmeta>"#;
    let r = read(text)?;
    assert_eq!(r.rating, Some(0));
    assert_eq!(r.capture.unwrap().captured, "2020-02-02T10:00:00");
    Ok(())
}

#[test]
fn impossible_days_and_times_are_not_read_and_empty_gps_clears() -> Result<()> {
    let packet = |date: &str, gps: &str| {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:Description rdf:about="" xmlns:exif="http://ns.adobe.com/exif/1.0/"
              exif:DateTimeOriginal="{date}" {gps}/></rdf:RDF></x:xmpmeta>"#
        )
    };
    for bad in [
        "2024-99-99T10:00:00",
        "2023-02-29T10:00:00",
        "2024-01-01T27:70:00",
        "2024-01-01T10:00:00+25:00",
    ] {
        assert_eq!(read(&packet(bad, ""))?.capture, None, "{bad}");
    }
    assert!(read(&packet("2024-02-29T23:59:59", ""))?.capture.is_some());
    let cleared = read(&packet(
        "2024-01-01T10:00:00",
        r#"exif:GPSLatitude="" exif:GPSLongitude="""#,
    ))?;
    assert_eq!(cleared.location, Some(Location::Cleared));
    Ok(())
}
