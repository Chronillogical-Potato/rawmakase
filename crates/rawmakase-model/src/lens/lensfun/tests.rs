use super::matching::{Outcome, Photo, find, frame_crop};
use super::names::{stated, words};
use super::*;

/// A small database in Lensfun's format, made up for the tests.
const DB: &str = r#"<!DOCTYPE lensdatabase SYSTEM "lensfun-database.dtd">
<lensdatabase version="2">
  <mount><name>Canon EF</name><compat>M42</compat></mount>
  <mount><name>Canon EF-M</name><compat>Canon EF</compat><compat>Canon EF-S</compat></mount>
  <mount><name>Micro 4/3 System</name></mount>
  <camera><maker>Canon</maker><model>Canon EOS 5D Mark III</model><model lang="en">EOS 5D Mark III</model><mount>Canon EF</mount><cropfactor>1</cropfactor></camera>
  <camera><maker>Canon</maker><model>Canon EOS M</model><mount>Canon EF-M</mount><cropfactor>1.6</cropfactor></camera>
  <camera><maker>Nikon Corporation</maker><maker lang="en">Nikon</maker><model>Nikon D6</model><mount>Nikon F AF</mount><cropfactor>1</cropfactor></camera>
  <camera><maker>Panasonic</maker><model>DMC-G6</model><mount>Micro 4/3 System</mount><cropfactor>2</cropfactor><aspect-ratio>4:3</aspect-ratio></camera>
  <camera><maker>Canon</maker><model>Canon PowerShot G12</model><mount>canonG12</mount><cropfactor>4.6</cropfactor><aspect-ratio>4:3</aspect-ratio></camera>
  <camera><maker>Sony</maker><model>ILCE-1M2</model><mount>Sony E</mount><cropfactor>1</cropfactor></camera>
  <lens><maker>Canon</maker><model>Canon EF 70-200mm f/2.8L IS II USM</model><mount>Canon EF</mount><cropfactor>1</cropfactor>
    <calibration><distortion model="ptlens" focal="70" a="0" b="0.01" c="0"/><distortion model="ptlens" focal="200" a="0" b="0.02" c="0"/></calibration></lens>
  <lens><maker>Canon</maker><model>Canon EF 70-200mm f/2.8L IS USM</model><mount>Canon EF</mount><cropfactor>1</cropfactor>
    <calibration><distortion model="poly3" focal="70" k1="0.01"/><distortion model="poly3" focal="200" k1="0.02"/></calibration></lens>
  <lens><maker>Canon</maker><model>Canon EF 24-70mm f/2.8L USM</model><mount>Canon EF</mount><cropfactor>1</cropfactor>
    <calibration><distortion model="poly3" focal="24" k1="-0.02"/><distortion model="poly3" focal="70" k1="0.01"/></calibration></lens>
  <lens><maker>Canon</maker><model>Canon EF-M 15-45mm f/3.5-6.3 IS STM</model><mount>Canon EF-M</mount><cropfactor>1.6</cropfactor>
    <calibration><distortion model="poly3" focal="15" k1="-0.03"/><distortion model="poly3" focal="45" k1="0.01"/></calibration></lens>
  <lens><maker>Sigma</maker><model>Sigma 17-50mm f/2.8 EX DC OS HSM</model><mount>Canon EF</mount><cropfactor>1.6</cropfactor>
    <calibration><distortion model="poly3" focal="17" k1="-0.03"/><distortion model="poly3" focal="50" k1="0.01"/></calibration></lens>
  <lens><maker>Nikon</maker><model>Nikon AF-S Nikkor 50mm f/1.4G</model><mount>Nikon F AF</mount><cropfactor>1</cropfactor>
    <calibration><distortion model="poly3" focal="50" k1="-0.01"/></calibration></lens>
  <lens><maker>Sigma</maker><model>Sigma 19mm f/2.8 DN</model><mount>Sony E</mount><mount>Micro 4/3 System</mount><cropfactor>1.534</cropfactor>
    <calibration><distortion model="ptlens" focal="19" a="0.015" b="-0.046" c="0.024"/></calibration></lens>
  <lens><maker>Sigma</maker><model>Sigma 19mm f/2.8 DN</model><mount>Micro 4/3 System</mount><mount>Sony E</mount><cropfactor>2</cropfactor><aspect-ratio>4:3</aspect-ratio>
    <calibration><distortion model="poly3" focal="19" k1="-0.00656"/></calibration></lens>
  <lens><maker>Canon</maker><model>PowerShot G12</model><mount>canonG12</mount><cropfactor>4.6</cropfactor><aspect-ratio>4:3</aspect-ratio>
    <calibration><distortion model="poly5" focal="6.1" k1="-0.05" k2="0.01"/><distortion model="poly5" focal="30.5" k1="0.01" k2="0"/></calibration></lens>
  <lens><maker>Sony</maker><model>FE 70-200mm f/2.8 GM OSS</model><mount>Sony E</mount><cropfactor>1</cropfactor>
    <calibration><distortion model="poly3" focal="70" k1="0.01"/></calibration></lens>
  <lens><maker>Samyang</maker><model>Samyang 8mm f/3.5 Fish-Eye CS</model><mount>Canon EF</mount><cropfactor>1.6</cropfactor><type>stereographic</type>
    <calibration><distortion model="poly3" focal="8" k1="0.01"/></calibration></lens>
  <lens><maker>Test</maker><model>Test 35mm f/2 ACM</model><mount>Canon EF</mount><cropfactor>1</cropfactor>
    <calibration><distortion model="acm" focal="35" k1="0.01"/></calibration></lens>
</lensdatabase>"#;

fn db() -> Database {
    Database::from_texts([("test.xml", DB)]).unwrap()
}
fn photo<'a>(make: &'a str, model: &'a str, lens: &'a str, focal: f32) -> Photo<'a> {
    Photo {
        make,
        model,
        lens,
        focal,
        focal_35mm: 0.,
        frame: [6000., 4000.],
    }
}
fn model_of(db: &Database, outcome: &Outcome) -> Vec<String> {
    match outcome {
        Outcome::Unique(i) => vec![db.lenses[*i].model.id.clone()],
        Outcome::Ambiguous(all) => all.iter().map(|i| db.lenses[*i].model.id.clone()).collect(),
        _ => vec![],
    }
}

#[test]
fn reads_lensfun_s_format() {
    let db = db();
    assert_eq!(db.mounts.len(), 3);
    assert_eq!(
        db.mount("canon ef-m").unwrap().compat,
        ["Canon EF", "Canon EF-S"]
    );
    let d6 = &db.cameras[2];
    assert_eq!(
        (d6.maker.id.as_str(), d6.maker.shown.as_str()),
        ("Nikon Corporation", "Nikon")
    );
    assert_eq!(db.cameras[0].model.shown, "EOS 5D Mark III");
    assert_eq!(db.cameras[3].aspect, Some(4. / 3.));
    let zoom = &db.lenses[0].calibration;
    // The focal range from the calibration, the aspect ratio Lensfun's default.
    assert_eq!((zoom.focal, zoom.aspect), ([70., 200.], 1.5));
    assert!(db.lenses[10].refusal().unwrap().contains("stereographic"));
    assert!(db.lenses[11].refusal().unwrap().contains("acm"));
    assert!(db.lenses[0].refusal().is_none());
    // Formats this reader doesn't know, and files that aren't Lensfun's, are refused.
    assert!(Database::from_texts([("x", r#"<lensdatabase version="3"/>"#)]).is_err());
    assert!(Database::from_texts([("x", "<lcp/>")]).is_err());
    assert!(
        Database::from_texts([(
            "x",
            "<lensdatabase><lens><maker>A</maker></lens></lensdatabase>"
        )])
        .is_err()
    );
}

#[test]
fn names_split_into_lensfun_s_words() {
    assert_eq!(
        words("smc PENTAX-DA 12-24mm F4 ED AL [IF]"),
        [
            "smc", "pentax", "da", "12", "24", "mm", "4", "ed", "al", "if"
        ]
    );
    assert_eq!(words("OM 12-45mm F4.0 PRO"), words("OM 12-45mm f/4 PRO"));
    assert_eq!(
        words("EF 70-200mm f/2.8L IS II USM"),
        ["ef", "70", "200", "mm", "2.8", "l", "is", "#2", "usm"]
    );
    assert_eq!(words("EOS 5D Mark III"), words("EOS 5D Mk3"));
    assert_eq!(
        words("Sigma 18-35mm F1.8 DC HSM | A"),
        ["sigma", "18", "35", "mm", "1.8", "dc", "hsm", "a"]
    );
    assert_eq!(
        words("Teleconverter 1.4x +"),
        ["teleconverter", "1.4", "x", "+"]
    );
    // "I/II" names both generations; Tamron's "Di III" is a lens line.
    use super::names::same_generation;
    let lens = words("E PZ 16-50mm f/3.5-5.6 OSS I/II");
    assert!(same_generation(&words("E PZ 16-50mm F3.5-5.6 OSS"), &lens));
    assert!(same_generation(
        &words("E PZ 16-50mm F3.5-5.6 OSS II"),
        &lens
    ));
    assert!(!same_generation(
        &words("FE 70-200mm F2.8 GM OSS"),
        &words("FE 70-200mm F2.8 GM OSS II")
    ));
    assert!(!same_generation(
        &words("FE 70-200mm F2.8 GM OSS II"),
        &words("FE 70-200mm F2.8 GM OSS")
    ));
    assert!(same_generation(
        &words("E 28-75mm F2.8 A063"),
        &words("Tamron E 28-75mm F/2.8 Di III VXD G2 A063Z")
    ));
    let s = stated("EF70-200mm f/2.8L IS II USM");
    assert_eq!((s.focal, s.aperture), (Some([70., 200.]), Some(2.8)));
    assert_eq!(stated("Summicron-M 1:2/35 ASPH.").aperture, Some(2.));
    let s = stated("2.8-3.5/25-45");
    assert_eq!((s.focal, s.aperture), (Some([25., 45.]), Some(2.8)));
    assert_eq!(stated("18-55 3.5-5.6").focal, Some([18., 55.]));
    assert!(!stated("35mm").agree(&stated("Sigma 18-35mm f/1.8")));
}

/// Cameras are found by the names the file writes, ignoring case, and by RAWmakase's
/// names when those are all a photo has.
#[test]
fn cameras_by_their_exif_names() {
    let db = db();
    let found = |make, model| {
        matching::camera(&db, make, model, [6000., 4000.]).map(|c| c.model.id.clone())
    };
    assert_eq!(
        found("Canon", "Canon EOS 5D Mark III").as_deref(),
        Some("Canon EOS 5D Mark III")
    );
    assert_eq!(
        found("NIKON CORPORATION", "NIKON D6").as_deref(),
        Some("Nikon D6")
    );
    // RAWmakase's names leave the maker out of Canon's model.
    assert_eq!(
        found("Canon", "EOS 5D Mark III").as_deref(),
        Some("Canon EOS 5D Mark III")
    );
    assert_eq!(found("Nikon", "D6"), None);
}

#[test]
fn a_lens_matches_by_its_words_mount_and_coverage() {
    let db = db();
    let m = find(
        &db,
        &photo(
            "Canon",
            "Canon EOS 5D Mark III",
            "EF70-200mm f/2.8L IS II USM",
            70.,
        ),
    );
    assert_eq!(
        model_of(&db, &m.outcome),
        ["Canon EF 70-200mm f/2.8L IS II USM"]
    );
    assert!(matches!(m.outcome, Outcome::Unique(_)));
    // The first version is not the second: the generation must agree both ways.
    let m = find(
        &db,
        &photo(
            "Canon",
            "Canon EOS 5D Mark III",
            "EF70-200mm f/2.8L IS USM",
            70.,
        ),
    );
    assert_eq!(
        model_of(&db, &m.outcome),
        ["Canon EF 70-200mm f/2.8L IS USM"]
    );
    let m = find(
        &db,
        &photo("SONY", "ILCE-1M2", "FE 70-200mm F2.8 GM OSS II", 100.),
    );
    assert_eq!(m.outcome, Outcome::NoMatch);
    // A focal length outside the lens's range.
    let m = find(
        &db,
        &photo(
            "Canon",
            "Canon EOS 5D Mark III",
            "EF70-200mm f/2.8L IS II USM",
            24.,
        ),
    );
    assert_eq!(m.outcome, Outcome::NoMatch);
    // A Nikon name without the maker's words.
    let m = find(
        &db,
        &photo("NIKON CORPORATION", "NIKON D6", "50mm f/1.4G", 50.),
    );
    assert_eq!(model_of(&db, &m.outcome), ["Nikon AF-S Nikkor 50mm f/1.4G"]);
}

/// "24-70mm" says too little to pick a lens: never one alone, however few fit.
#[test]
fn bare_and_tied_names_stay_a_choice() {
    let db = db();
    let m = find(
        &db,
        &photo("Canon", "Canon EOS 5D Mark III", "24-70mm", 40.),
    );
    assert_eq!(m.outcome, Outcome::Ambiguous(vec![2]));
    // The same lens calibrated on two cameras: both fit a Micro Four Thirds body.
    let m = find(
        &db,
        &Photo {
            frame: [4608., 3456.],
            ..photo("Panasonic", "DMC-G6", "SIGMA 19mm F2.8 DN", 19.)
        },
    );
    assert_eq!(m.outcome, Outcome::Ambiguous(vec![6, 7]));
    // Another maker's lens is never this one.
    let m = find(
        &db,
        &photo("Canon", "Canon EOS M", "TAMRON 17-50mm F/2.8", 20.),
    );
    assert_eq!(m.outcome, Outcome::NoMatch);
    assert_eq!(
        find(&db, &photo("Canon", "Canon EOS M", "", 20.)).outcome,
        Outcome::NoLensName
    );
    assert_eq!(
        find(&db, &photo("Leica", "M10", "Summicron", 35.)).outcome,
        Outcome::UnknownCamera
    );
}

/// Lensfun's mount compatibility goes one way: EF lenses fit an EF-M camera, not the
/// reverse; an APS-C calibration doesn't cover a full-frame photo.
#[test]
fn mounts_fit_one_way_and_coverage_counts() {
    let db = db();
    let m = find(
        &db,
        &photo("Canon", "Canon EOS M", "EF-M15-45mm f/3.5-6.3 IS STM", 15.),
    );
    assert_eq!(
        model_of(&db, &m.outcome),
        ["Canon EF-M 15-45mm f/3.5-6.3 IS STM"]
    );
    let m = find(
        &db,
        &photo("Canon", "Canon EOS M", "EF70-200mm f/2.8L IS II USM", 70.),
    );
    assert_eq!(
        model_of(&db, &m.outcome),
        ["Canon EF 70-200mm f/2.8L IS II USM"]
    );
    let m = find(
        &db,
        &photo(
            "Canon",
            "Canon EOS 5D Mark III",
            "EF-M15-45mm f/3.5-6.3 IS STM",
            15.,
        ),
    );
    assert_eq!(m.outcome, Outcome::NoMatch);
    // The Sigma APS-C zoom fits the EOS M (1.6) by name and mount, not the 5D.
    let m = find(
        &db,
        &photo(
            "Canon",
            "Canon EOS 5D Mark III",
            "17-50mm F2.8 EX DC OS HSM",
            20.,
        ),
    );
    assert_eq!(m.outcome, Outcome::NoMatch);
    let (crop, all) = matching::choices(&db, &photo("Canon", "Canon EOS 5D Mark III", "", 50.));
    assert_eq!(crop, Some(1.));
    assert!(all.iter().all(|i| db.lenses[*i].calibration.crop <= 1.04));
}

/// A compact's lens is its camera's own mount; the name doesn't matter.
#[test]
fn a_fixed_lens_camera_needs_no_lens_name() {
    let db = db();
    let m = find(
        &db,
        &Photo {
            frame: [3648., 2736.],
            ..photo("Canon", "Canon PowerShot G12", "", 6.1)
        },
    );
    assert_eq!(model_of(&db, &m.outcome), ["PowerShot G12"]);
    assert_eq!(m.crop, Some(4.6));
}

/// A readout of another aspect ratio is a smaller part of the sensor: a 1:1 photo
/// from a 4:3 sensor of crop factor 2 has the diagonal of a 2.357 crop.
#[test]
fn readouts_of_another_aspect_ratio_have_their_own_crop_factor() {
    let one = frame_crop(2., Some(4. / 3.), [3456., 3456.]);
    assert!((one - 2. * (5. / 3.) / 2f32.sqrt()).abs() < 1e-4, "{one}");
    assert_eq!(frame_crop(2., Some(4. / 3.), [4608., 3456.]), 2.);
    // 16:9 from 3:2 keeps the width.
    let wide = frame_crop(1., Some(1.5), [6000., 3375.]);
    assert!((wide - 43.2666 / 36f32.hypot(20.25)).abs() < 1e-3, "{wide}");
    // Without the sensor's aspect ratio, the frame is the sensor, as Lensfun has it.
    assert_eq!(frame_crop(4.63, None, [3648., 2736.]), 4.63);
    // Lensfun names no aspect ratio for most Micro Four Thirds bodies; they are 4:3.
    let db = db();
    let g6 = db.cameras.iter().find(|c| c.model.id == "DMC-G6").unwrap();
    assert_eq!(matching::native_aspect(g6), Some(4. / 3.));
    let mut no_aspect = g6.clone();
    no_aspect.aspect = None;
    assert_eq!(matching::native_aspect(&no_aspect), Some(4. / 3.));
    no_aspect.mount = "Canon EF".into();
    assert_eq!(matching::native_aspect(&no_aspect), None);
}

/// A default crop the decoder centres (it gives no position) is read where
/// `ImageFrame` puts it.
#[test]
fn a_centred_default_crop_is_the_frame() {
    let m = crate::camera_data::Metadata {
        width: 4608,
        height: 3072,
        crop_left: 65535,
        crop_top: 65535,
        crop_width: 4592,
        crop_height: 3056,
        ..Default::default()
    };
    assert_eq!(choice::frame_rect(&m), [8., 8., 4592., 3056.]);
    let whole = crate::camera_data::Metadata {
        width: 600,
        height: 400,
        ..Default::default()
    };
    assert_eq!(choice::frame_rect(&whole), [0., 0., 600., 400.]);
}

/// An edit's copy of a calibration renders without the database, and is found again
/// by its identity, apart from a same-named lens calibrated elsewhere.
#[test]
fn choices_copy_their_calibration() {
    let db = db();
    let sigma = &db.lenses[7];
    let choice = LensfunChoice::of(sigma, &db, None);
    assert_eq!(db.find(&choice.lens).map(|l| l.calibration.crop), Some(2.));
    assert_eq!(choice.calibration.as_deref(), Some(&sigma.calibration));
    assert_eq!(
        choice.provenance.license_uri,
        "https://creativecommons.org/licenses/by-sa/3.0/"
    );
    assert!(choice.validate());
    let text = serde_json::to_string(&choice).unwrap();
    assert_eq!(
        serde_json::from_str::<LensfunChoice>(&text).unwrap(),
        choice
    );
}
