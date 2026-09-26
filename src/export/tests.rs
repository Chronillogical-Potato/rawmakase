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
        image::codecs::jpeg::JpegDecoder::new(std::io::BufReader::new(File::open(jpg)?))?;
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
