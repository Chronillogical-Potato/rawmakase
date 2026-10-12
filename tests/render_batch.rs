//! `rawmakase render-batch`, which `scripts/corpus/gate.py render` drives: one RAW
//! developed once and rendered with several XMP sidecars.
use std::{path::Path, process::Command};

fn xmp(dir: &Path, name: &str, exposure: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:Exposure2012="{exposure}"/>
</rdf:RDF></x:xmpmeta>"#
        ),
    )
    .unwrap();
    path
}

#[test]
fn renders_each_job_with_its_own_settings_and_skips_existing_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let raw = dir.path().join("chart.dng");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/corpus/charts/synthetic-d65.dng"
        ),
        &raw,
    )
    .unwrap();
    let out = |name: &str| dir.path().join(name);
    let broken = dir.path().join("broken.xmp");
    std::fs::write(&broken, "not XMP").unwrap();
    std::fs::write(out("kept.tif"), b"kept").unwrap();
    let jobs = serde_json::json!([
        {"xmp": xmp(dir.path(), "dark.xmp", "-1.00"), "output": out("dark.tif"), "max_edge": 256},
        {"xmp": broken, "output": out("broken.tif"), "max_edge": 256},
        {"xmp": xmp(dir.path(), "bright.xmp", "+1.00"), "output": out("bright.tif"), "max_edge": 256},
        {"xmp": xmp(dir.path(), "kept.xmp", "0"), "output": out("kept.tif")},
    ]);
    let listing = dir.path().join("jobs.json");
    std::fs::write(&listing, jobs.to_string()).unwrap();
    let run = Command::new(env!("CARGO_BIN_EXE_rawmakase"))
        .args([
            "render-batch".as_ref(),
            raw.as_os_str(),
            listing.as_os_str(),
        ])
        // No profiles from this machine's library.
        .env("RAWMAKASE_DATA_DIR", dir.path().join("data"))
        .output()
        .unwrap();
    assert!(run.status.success(), "{run:?}");
    // A job that fails is reported and the others still render.
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("broken.tif"), "{stderr}");
    assert!(!out("broken.tif").exists());
    // An existing output is left alone.
    assert_eq!(std::fs::read(out("kept.tif")).unwrap(), b"kept");
    let mean = |name: &str| {
        let image = image::open(out(name)).unwrap().to_rgb16();
        assert_eq!(image.width().max(image.height()), 256);
        image.pixels().map(|p| f64::from(p[1])).sum::<f64>()
            / f64::from(image.width() * image.height())
    };
    // Each job renders with its own sidecar: two stops apart.
    let (dark, bright) = (mean("dark.tif"), mean("bright.tif"));
    assert!(bright > dark * 1.5, "{dark} / {bright}");
}
