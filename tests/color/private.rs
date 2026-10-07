//! Tests that need private data in RAWMAKASE_CORPUS; see tests/corpus/README.md.
//!
//! Photos are compared as a grid of block averages (48 blocks across), which keeps
//! references small and ignores detail, noise and sharpening.
use super::*;

fn private_corpus() -> PathBuf {
    PathBuf::from(std::env::var_os("RAWMAKASE_CORPUS").expect("Set RAWMAKASE_CORPUS"))
}

/// The embedded profile plus every DCP in RAWMAKASE_PROFILES whose file name
/// mentions the camera model, and every XMP look there.
fn imported_profiles(im: &CameraImage) -> Vec<Arc<CameraProfile>> {
    let dir = PathBuf::from(
        std::env::var_os("RAWMAKASE_PROFILES")
            .expect("Set RAWMAKASE_PROFILES to a folder of DCP/XMP profiles"),
    );
    let model = im.metadata.model.to_ascii_lowercase();
    let mut files: Vec<PathBuf> = walk(&dir)
        .into_iter()
        .filter(|p| {
            let name = p
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_ascii_lowercase();
            (name.ends_with(".dcp") && name.contains(&model)) || name.ends_with(".xmp")
        })
        .collect();
    // Base profiles first: XMP looks resolve against them.
    files.sort_by_key(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xmp")));
    let mut profiles = embedded_profiles(im);
    for f in files {
        if let Ok(p) = rawmakase::camera_profiles::load(&f, &im.metadata)
            && p.ensure_camera(&im.metadata).is_ok()
        {
            profiles.push(p);
        }
    }
    profiles
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Per-camera charts rendered with the camera's Adobe Standard profile, against
/// Camera Raw references in $RAWMAKASE_CORPUS/camera-raw-adobe.
#[test]
#[ignore = "Needs RAWMAKASE_CORPUS with Adobe-profile references and RAWMAKASE_PROFILES"]
fn adobe_profile_parity_does_not_regress() {
    let dir = private_corpus().join("camera-raw-adobe");
    let references: Vec<(String, PatchFile)> = chart_specs()
        .into_iter()
        .filter_map(|spec| {
            PatchFile::read(&dir.join(format!("{}.json", spec.name))).map(|f| (spec.name, f))
        })
        .collect();
    assert!(!references.is_empty(), "No references in {}", dir.display());
    check_parity(
        "Adobe Standard",
        &references,
        &dir.join("baseline.json"),
        &[("CameraProfile", "Adobe Standard")],
        imported_profiles,
    );
}

pub const COLUMNS: u32 = 48;

/// The block grid for an image: `COLUMNS` across, square-ish blocks.
pub fn blocks(width: u32, height: u32) -> Vec<Patch> {
    let rows = ((COLUMNS as f64 * height as f64 / width as f64).round() as u32).max(1);
    let edge = |i: u32, n: u32, size: u32| (i as u64 * size as u64 / n as u64) as u32;
    (0..rows)
        .flat_map(|r| {
            (0..COLUMNS).map(move |c| {
                let (x, y) = (edge(c, COLUMNS, width), edge(r, rows, height));
                Patch {
                    name: format!("block {c},{r}"),
                    group: "block".into(),
                    x,
                    y,
                    w: edge(c + 1, COLUMNS, width) - x,
                    h: edge(r + 1, rows, height) - y,
                }
            })
        })
        .collect()
}

const RAW_EXTENSIONS: &[&str] = &[
    "arw", "raf", "nef", "nrw", "cr2", "cr3", "dng", "rw2", "orf", "ori", "pef", "rwl", "fff",
    "3fr",
];
const PHOTO_EDGE: u32 = 1200;

fn raws(corpus: &Path) -> Vec<PathBuf> {
    walk(&corpus.join("raws"))
        .into_iter()
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| RAW_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        })
        .collect()
}

fn profiles_for(im: &CameraImage) -> Vec<Arc<CameraProfile>> {
    if std::env::var_os("RAWMAKASE_PROFILES").is_some() {
        imported_profiles(im)
    } else {
        embedded_profiles(im)
    }
}

/// RAWmakase's accepted block values for one photo.
#[derive(Default, Serialize, Deserialize)]
struct Accepted {
    width: u32,
    height: u32,
    cases: BTreeMap<String, PatchValues>,
}

/// Real photos in $RAWMAKASE_CORPUS/raws against RAWmakase's last accepted renders
/// in $RAWMAKASE_CORPUS/accepted. Uses RAWMAKASE_PROFILES when set.
#[test]
#[ignore = "Needs RAWMAKASE_CORPUS with RAW files"]
fn photos_match_accepted_renders() {
    let corpus = private_corpus();
    let raws = raws(&corpus);
    assert!(
        !raws.is_empty(),
        "No RAW files in {}",
        corpus.join("raws").display()
    );
    let cases = cases();
    let failures = std::sync::Mutex::new(Vec::new());
    let skipped = std::sync::Mutex::new(Vec::new());
    // One photo at a time (LibRaw develops with its own threads).
    raws.iter().for_each(|raw| {
        let relative = raw
            .strip_prefix(corpus.join("raws"))
            .unwrap()
            .with_extension("");
        let path = corpus
            .join("accepted")
            .join(relative.with_extension("json"));
        let accepted: Option<Accepted> = std::fs::read(&path)
            .ok()
            .map(|b| serde_json::from_slice(&b).expect("accepted renders"));
        let im = match rawmakase::photo::open(raw).and_then(|r| {
            r.develop(
                rawmakase::camera_data::Decode::full(Default::default()),
                &AtomicBool::new(false),
            )
        }) {
            Ok(im) => im,
            Err(e) if accepted.is_none() => {
                skipped
                    .lock()
                    .unwrap()
                    .push(format!("{}: {e}", relative.display()));
                return;
            }
            Err(e) => {
                failures
                    .lock()
                    .unwrap()
                    .push(format!("{}: {e}", relative.display()));
                return;
            }
        };
        let profiles = profiles_for(&im);
        let mut current = Accepted::default();
        for case in cases.cases.iter().filter(|c| c.photos) {
            let name = &case.name;
            match render(&im, &profiles, &case.xmp(&cases.base, &[]), PHOTO_EDGE) {
                Ok(out) => {
                    (current.width, current.height) = (out.width, out.height);
                    let values =
                        measure::patches(out.width, &out.pixels, &blocks(out.width, out.height));
                    current.cases.insert(name.clone(), values);
                }
                Err(e) => failures
                    .lock()
                    .unwrap()
                    .push(format!("{} / {name}: {e}", relative.display())),
            }
        }
        if bless() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, serde_json::to_vec(&current).unwrap()).unwrap();
            return;
        }
        let Some(accepted) = accepted else {
            failures
                .lock()
                .unwrap()
                .push(format!("{}: no accepted render", relative.display()));
            return;
        };
        if (accepted.width, accepted.height) != (current.width, current.height) {
            failures.lock().unwrap().push(format!(
                "{}: size changed from {}×{} to {}×{}",
                relative.display(),
                accepted.width,
                accepted.height,
                current.width,
                current.height
            ));
            return;
        }
        for (name, values) in &current.cases {
            let Some(expected) = accepted.cases.get(name) else {
                continue;
            };
            let c = compare(expected, values, |_| true);
            if c.mean > 0.3 || c.p95 > 1.0 {
                failures.lock().unwrap().push(format!(
                    "{} / {name}: mean ΔE00 {:.2}, p95 {:.2}, max {:.2}",
                    relative.display(),
                    c.mean,
                    c.p95,
                    c.max
                ));
            }
        }
    });
    for s in skipped.into_inner().unwrap() {
        println!("skipped, cannot develop: {s}");
    }
    let mut failures = failures.into_inner().unwrap();
    failures.sort();
    assert!(
        failures.is_empty(),
        "Photos changed (rerun with RAWMAKASE_BLESS=1 to accept):\n{}",
        failures.join("\n")
    );
}

/// A Camera Raw render of a photo, reduced to blocks by scripts/corpus.
#[derive(Deserialize)]
struct PhotoReference {
    /// Relative to $RAWMAKASE_CORPUS/raws.
    raw: String,
    settings: String,
    width: u32,
    height: u32,
    #[serde(default)]
    camera_raw: String,
    blocks: PatchValues,
}

/// Real photos against Camera Raw references in $RAWMAKASE_CORPUS/camera-raw-photos.
/// Fails when a reference is further from Camera Raw than its baseline. Set
/// RAWMAKASE_PHOTO_FILTER to a substring of the reference path to run a subset.
#[test]
#[ignore = "Needs RAWMAKASE_CORPUS with Camera Raw photo references and RAWMAKASE_PROFILES"]
fn photos_camera_raw_parity_does_not_regress() {
    let corpus = private_corpus();
    let dir = corpus.join("camera-raw-photos");
    let filter = std::env::var("RAWMAKASE_PHOTO_FILTER").unwrap_or_default();
    let mut by_raw: BTreeMap<String, Vec<(String, PhotoReference)>> = BTreeMap::new();
    for path in walk(&dir) {
        let name = path
            .strip_prefix(&dir)
            .unwrap()
            .with_extension("")
            .display()
            .to_string();
        if path.extension().is_none_or(|e| e != "json")
            || name == "baseline"
            || !name.contains(&filter)
        {
            continue;
        }
        let r: PhotoReference = serde_json::from_slice(&std::fs::read(&path).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        by_raw.entry(r.raw.clone()).or_default().push((name, r));
    }
    assert!(!by_raw.is_empty(), "No references in {}", dir.display());
    let baseline_path = dir.join("baseline.json");
    let mut baseline: BTreeMap<String, [f64; 2]> = std::fs::read(&baseline_path)
        .ok()
        .map(|b| serde_json::from_slice(&b).expect("baseline"))
        .unwrap_or_default();
    // One photo at a time (LibRaw develops with its own threads), cases in parallel.
    let results: Vec<(String, Result<Comparison, String>, String)> = by_raw
        .iter()
        .flat_map(|(raw, references)| {
            let im = match rawmakase::photo::open(&corpus.join("raws").join(raw)).and_then(|r| {
                r.develop(
                    rawmakase::camera_data::Decode::full(Default::default()),
                    &AtomicBool::new(false),
                )
            }) {
                Ok(im) => im,
                Err(e) => {
                    return references
                        .iter()
                        .map(|(name, r)| (name.clone(), Err(format!("{e}")), r.camera_raw.clone()))
                        .collect::<Vec<_>>();
                }
            };
            let profiles = imported_profiles(&im);
            references
                .par_iter()
                .map(|(name, r)| {
                    let result = render(&im, &profiles, &r.settings, PHOTO_EDGE)
                        .map_err(|e| e.to_string())
                        .and_then(|out| {
                            let ratio = (out.width as f64 / out.height as f64)
                                / (r.width as f64 / r.height as f64);
                            if (ratio - 1.).abs() > 0.005 {
                                return Err(format!(
                                    "aspect ratio differs by {:.2}%",
                                    (ratio - 1.).abs() * 100.
                                ));
                            }
                            let ours = measure::patches(
                                out.width,
                                &out.pixels,
                                &blocks(out.width, out.height),
                            );
                            if ours.len() != r.blocks.len() {
                                return Err("block grid differs".into());
                            }
                            // RAWmakase's blocks for scripts/corpus/parity-report.py.
                            if let Some(dir) = std::env::var_os("RAWMAKASE_PARITY_DUMP") {
                                let path =
                                    Path::new(&dir).join("photos").join(format!("{name}.json"));
                                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                                std::fs::write(path, serde_json::to_string(&ours).unwrap())
                                    .unwrap();
                            }
                            Ok(compare(&r.blocks, &ours, |_| true))
                        });
                    (name.clone(), result, r.camera_raw.clone())
                })
                .collect()
        })
        .collect();
    let mut failures = Vec::new();
    println!("\n{:60} {:>6} {:>6}", "reference", "ΔE00", "p95");
    for (name, result, _) in &results {
        match result {
            Err(e) => {
                println!("{name:60} {e}");
                failures.push(format!("{name}: {e}"));
            }
            Ok(c) => {
                println!("{name:60} {:6.2} {:6.2}", c.mean, c.p95);
                if bless() {
                    baseline.insert(name.clone(), [round(c.mean), round(c.p95)]);
                } else if let Some([mean, p95]) = baseline.get(name) {
                    if c.mean > mean + PARITY_MEAN_MARGIN || c.p95 > p95 + PARITY_P95_MARGIN {
                        failures.push(format!(
                            "{name}: mean ΔE00 {:.2} (baseline {mean:.2}), p95 {:.2} (baseline {p95:.2})",
                            c.mean, c.p95
                        ));
                    }
                } else {
                    failures.push(format!("{name}: no baseline"));
                }
            }
        }
    }
    if bless() {
        std::fs::write(
            &baseline_path,
            serde_json::to_string_pretty(&baseline).unwrap() + "\n",
        )
        .unwrap();
    }
    assert!(
        failures.is_empty(),
        "Further from Camera Raw than the baseline (rerun with RAWMAKASE_BLESS=1 to accept):\n{}",
        failures.join("\n")
    );
}
