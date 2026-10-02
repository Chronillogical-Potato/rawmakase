use super::*;
#[test]
fn icc_export_and_sixteen_bit_precision() -> Result<()> {
    use image::ImageDecoder;
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.ARW");
    fs::write(&source, b"source")?;
    let image = Rendered {
        width: 1024,
        height: 1,
        pixels: (0..1024).map(|i| [i as f32 / 1023.; 3]).collect(),
    };
    let m = Metadata {
        make: "Sony".into(),
        model: "test".into(),
        iso: 100.,
        aperture: 2.8,
        shutter: 0.01,
        ..Default::default()
    };
    let jpg = dir.path().join("out.jpg");
    export(&jpg, &source, &image, &m, &ExportOptions::default(), false)?;
    let mut decoder =
        image::codecs::jpeg::JpegDecoder::new(std::io::BufReader::new(fs::File::open(jpg)?))?;
    assert!(decoder.icc_profile()?.unwrap().len() > 100);
    assert!(
        decoder
            .exif_metadata()?
            .unwrap()
            .windows(4)
            .any(|w| w == b"Sony")
    );
    let tif = dir.path().join("out.tiff");
    export(&tif, &source, &image, &m, &ExportOptions::default(), false)?;
    let out = image::open(tif)?.to_rgb16();
    let unique: std::collections::HashSet<_> = out.pixels().map(|p| p[0]).collect();
    assert_eq!(unique.len(), 1024);
    Ok(())
}
#[test]
fn develop_settings_round_trip_through_the_exported_xmp() -> Result<()> {
    use crate::develop::Recipe;
    let m = Metadata {
        make: "Sony".into(),
        model: "ILCE-7M2".into(),
        lens_model: "FE 55mm F1.8 ZA".into(),
        wb: [2., 1., 1.5],
        daylight_wb: [2., 1., 1.5],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let mut r = Recipe {
        exposure: 0.4,
        contrast: 0.12,
        shadows: -0.3,
        vibrance: 0.25,
        straighten: 1.5,
        crop: [0.1, 0.05, 0.9, 0.95],
        sharpening: 0.4,
        lens_ca: true,
        ..Default::default()
    };
    r.hsl[3][0] = 0.26;
    r.hsl[5][1] = -0.72;
    r.grading[0] = [220. / 360., 0.2, -0.1];
    r.effects.clarity = 0.15;
    r.curve.points = vec![[0., 0.1], [0.5, 0.55], [1., 1.]];
    let identity = [1., 0., 0., 0., 1., 0., 0., 0., 1.];
    r.upright = crate::develop::Upright {
        mode: crate::develop::UprightMode::Level,
        corrections: vec![
            identity,
            identity,
            identity,
            [1.02, -0.01, 0.005, 0.02, 1.02, -0.02, 0., 0., 1.],
        ],
        lightroom: [("UprightVersion".into(), "151388160".into())].into(),
    };
    let photo = crate::xmp::write::Photo {
        raw_name: "DSC07924.ARW".into(),
        captured: Some("2018:08:26 10:39:33".into()),
        now: "2026-09-27T06:12:22Z".into(),
        rating: 3,
        keywords: vec!["coffee & books".into()],
        settings: true,
        format: "image/jpeg".into(),
        ..Default::default()
    };
    let packet = crate::xmp::write::packet(&r, &m, &photo);
    assert!(packet.contains("crs:Exposure2012=\"+0.40\""));
    assert!(packet.contains("crs:AutoLateralCA=\"1\""));
    assert!(packet.contains("xmp:CreateDate=\"2018-08-26T10:39:33\""));
    assert!(packet.contains("coffee &amp; books"));
    let preset = crate::xmp::parse(Path::new("export.xmp"), &packet)?;
    let back = preset.apply(&Recipe::default(), &m, &[], None)?;
    for (a, b) in [
        (back.exposure, r.exposure),
        (back.contrast, r.contrast),
        (back.shadows, r.shadows),
        (back.vibrance, r.vibrance),
        (back.straighten, r.straighten),
        (back.sharpening, r.sharpening),
        (back.hsl[3][0], r.hsl[3][0]),
        (back.hsl[5][1], r.hsl[5][1]),
        (back.grading[0][0], r.grading[0][0]),
        (back.grading[0][2], r.grading[0][2]),
        (back.effects.clarity, r.effects.clarity),
    ] {
        assert!((a - b).abs() < 0.006, "{a} != {b}");
    }
    assert_eq!(back.crop, r.crop);
    assert_eq!(back.curve.points.len(), 3);
    assert_eq!(back.upright, r.upright);
    assert!(back.lens_ca);
    Ok(())
}
#[test]
fn jpeg_carries_camera_exif_gps_and_xmp() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.ARW");
    fs::write(&source, b"source")?;
    let image = Rendered {
        width: 8,
        height: 4,
        pixels: vec![[0.5; 3]; 32],
    };
    let camera = exif::CameraExif {
        main: vec![
            exif::Field::ascii(0x010f, "SONY"),
            exif::Field::ascii(0x0110, "ILCE-7M2"),
        ],
        exif: vec![
            exif::Field::ascii(0x9003, "2018:08:26 10:39:33"),
            exif::Field::ascii(0xa434, "FE 55mm F1.8 ZA"),
            exif::Field::ascii(0x927c, "maker note"),
        ],
        gps: vec![exif::Field::rational(0x0002, 52, 1)],
    };
    let embed = Embed {
        camera: Some(camera.clone()),
        xmp: Some("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"/>".into()),
        ..Default::default()
    };
    let jpg = dir.path().join("out.jpg");
    let m = Metadata::default();
    export_with(
        &jpg,
        &source,
        &image,
        &m,
        &ExportOptions::default(),
        &embed,
        false,
    )?;
    let bytes = fs::read(&jpg)?;
    let has = |needle: &[u8]| bytes.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"2018:08:26 10:39:33"));
    assert!(has(b"FE 55mm F1.8 ZA"));
    assert!(has(b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta"));
    assert!(image::open(&jpg).is_ok());
    let tif = dir.path().join("out.tif");
    let without_location = Embed {
        location: false,
        ..embed
    };
    export_with(
        &tif,
        &source,
        &image,
        &m,
        &ExportOptions::default(),
        &without_location,
        false,
    )?;
    assert!(image::open(&tif).is_ok());
    Ok(())
}
#[test]
fn capture_times_take_lightroom_form_with_three_digit_subseconds() {
    use exif::lightroom_time;
    let t = |date, sub| lightroom_time(date, sub);
    assert_eq!(
        t("2018:08:26 10:39:33", Some("12")).as_deref(),
        Some("2018-08-26T10:39:33.120")
    );
    assert_eq!(
        t("2018:08:26 10:39:33", Some("1234")).as_deref(),
        Some("2018-08-26T10:39:33.123")
    );
    assert_eq!(
        t("2018:08:26 10:39:33", None).as_deref(),
        Some("2018-08-26T10:39:33.000")
    );
    assert_eq!(
        t("2018-08-26T10:39:33", Some(" 5 ")).as_deref(),
        Some("2018-08-26T10:39:33.500")
    );
    for blank in [
        "",
        "    :  :     :  :  ",
        "0000:00:00 00:00:00",
        "2018:08:26",
    ] {
        assert_eq!(t(blank, None), None, "{blank:?}");
    }
    // Lightroom's own values, with or without a fraction, sort with these.
    let mut times = [
        "2021-06-06T10:00:01",
        "2021-06-06T10:00:00.500",
        "2021-06-06T10:00:00.000",
        "2021-06-06T10:00:00",
    ];
    times.sort();
    assert_eq!(
        times,
        [
            "2021-06-06T10:00:00",
            "2021-06-06T10:00:00.000",
            "2021-06-06T10:00:00.500",
            "2021-06-06T10:00:01",
        ]
    );
}
#[test]
fn capture_time_is_read_from_tiff_and_jpeg_files() -> anyhow::Result<()> {
    let directory = tempfile::tempdir()?;
    for (jpeg, name) in [(false, "a.tif"), (true, "b.jpg")] {
        let path = directory.path().join(name);
        std::fs::write(&path, exif::dated_file(jpeg, "2019:05:04 03:02:01", "7"))?;
        assert_eq!(
            exif::capture_time(&path).as_deref(),
            Some("2019-05-04T03:02:01.700"),
            "{name}"
        );
    }
    let undated = directory.path().join("c.jpg");
    std::fs::write(&undated, [0xff, 0xd8, 0xff, 0xd9])?;
    assert_eq!(exif::capture_time(&undated), None);
    Ok(())
}
