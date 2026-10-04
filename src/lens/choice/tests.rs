use super::*;
use crate::lens::lcp::{Library, test_profile};

pub(crate) const ADOBE: &str = "Testcam (35mm F2) - RAW.lcp";
pub(crate) const MINE: &str = "Mine 35mm F2.lcp";
pub(crate) const OTHER: &str = "Lensco (50mm F1.4) - RAW.lcp";

/// Imported profiles: two of the test lens (Adobe's and one the user made) and one
/// of another lens and make.
pub(crate) fn library() -> Library {
    let texts = [
        (
            ADOBE,
            test_profile("Testcam", "35mm F2", "Adobe (Testcam 35mm F2)", -0.05, -0.5),
        ),
        (
            MINE,
            test_profile("Testcam", "35mm F2", "Mine (Testcam 35mm F2)", 0., -0.2),
        ),
        (
            OTHER,
            test_profile(
                "Lensco",
                "50mm F1.4",
                "Adobe (Lensco 50mm F1.4)",
                0.02,
                -0.8,
            ),
        ),
    ];
    Library::from_texts(texts.iter().map(|(f, t)| (*f, t.as_str())))
}
/// A photo from a synthetic camera and lens, with the profiles imported for it.
pub(crate) fn photo() -> Metadata {
    let mut m = Metadata {
        make: "Testcam".into(),
        model: "T1".into(),
        lens_model: "35mm F2".into(),
        focal: 35.,
        aperture: 2.,
        width: 600,
        height: 400,
        ..Default::default()
    };
    m.lens_profiles = library().for_photo(&m);
    m
}
fn id(filename: &str) -> Option<LensProfileId> {
    Some(LensProfileId {
        filename: filename.into(),
        ..Default::default()
    })
}
fn choice(setup: LensProfileSetup, id: Option<LensProfileId>) -> LensProfileChoice {
    LensProfileChoice { setup, id }
}
fn used(c: &LensProfileChoice, m: &Metadata) -> Option<String> {
    c.resolve(&m.lens_profiles, m)
        .used
        .map(|c| c.profile.filename.clone())
}

#[test]
fn default_and_auto_match_the_lens() {
    let m = photo();
    assert_eq!(m.lens_profiles.all().len(), 3);
    for setup in [LensProfileSetup::Default, LensProfileSetup::Auto] {
        let c = choice(setup, None);
        assert_eq!(used(&c, &m).as_deref(), Some(ADOBE), "{setup:?}");
        assert!(c.resolve(&m.lens_profiles, &m).missing.is_none());
    }
    // No imported profile of the lens: nothing automatic, whatever else is imported.
    let mut other_lens = photo();
    other_lens.lens_model = "85mm F1.8".into();
    other_lens.lens_profiles = library().for_photo(&other_lens);
    assert_eq!(used(&LensProfileChoice::default(), &other_lens), None);
}
#[test]
fn an_edit_keeps_the_profile_it_names() {
    let m = photo();
    // Default and Auto keep a named profile of this lens over the best match.
    assert_eq!(
        used(&choice(LensProfileSetup::Auto, id(MINE)), &m).as_deref(),
        Some(MINE)
    );
    assert_eq!(
        used(&choice(LensProfileSetup::Default, id(MINE)), &m).as_deref(),
        Some(MINE)
    );
    // A profile of another lens only under Custom, as for an adapted lens.
    assert_eq!(
        used(&choice(LensProfileSetup::Custom, id(OTHER)), &m).as_deref(),
        Some(OTHER)
    );
    assert_eq!(
        used(&choice(LensProfileSetup::Auto, id(OTHER)), &m).as_deref(),
        Some(ADOBE)
    );
    // Without a file name, the profile name identifies it.
    let by_name = LensProfileId {
        name: "Mine (Testcam 35mm F2)".into(),
        ..Default::default()
    };
    assert_eq!(
        used(&choice(LensProfileSetup::Custom, Some(by_name)), &m).as_deref(),
        Some(MINE)
    );
    // File names compare without case, as on macOS.
    assert_eq!(
        used(
            &choice(LensProfileSetup::Custom, id(&MINE.to_uppercase())),
            &m
        )
        .as_deref(),
        Some(MINE)
    );
}

#[test]
fn a_named_profile_that_is_not_imported_is_reported() {
    let m = photo();
    let gone = choice(LensProfileSetup::Custom, id("Gone.lcp"));
    let r = gone.resolve(&m.lens_profiles, &m);
    assert!(r.used.is_none());
    assert_eq!(r.missing.map(|i| i.filename.as_str()), Some("Gone.lcp"));
    // Auto still matches, and says what it stands in for.
    let auto = choice(LensProfileSetup::Auto, id("Gone.lcp"));
    let r = auto.resolve(&m.lens_profiles, &m);
    assert_eq!(r.used.map(|c| c.profile.filename.as_str()), Some(ADOBE));
    assert!(r.missing.is_some());
}

#[test]
fn choosing_a_profile_sets_custom_and_setup_rematches() {
    let m = photo();
    let other = &m
        .lens_profiles
        .all()
        .iter()
        .find(|c| c.profile.filename == OTHER)
        .unwrap()
        .profile;
    let mut c = LensProfileChoice::default();
    c.choose(other);
    assert_eq!(c.setup, LensProfileSetup::Custom);
    assert_eq!(
        c.id.as_ref().map(|i| i.name.as_str()),
        Some("Adobe (Lensco 50mm F1.4)")
    );
    assert_eq!(used(&c, &m).as_deref(), Some(OTHER));
    // Auto and Default forget it and match again.
    c.set_setup(LensProfileSetup::Auto, Some(other));
    assert_eq!(c, choice(LensProfileSetup::Auto, None));
    assert_eq!(used(&c, &m).as_deref(), Some(ADOBE));
    // Custom keeps the profile in use.
    let adobe = &m.lens_profiles.auto(&m).unwrap().profile;
    c.set_setup(LensProfileSetup::Custom, Some(adobe));
    assert_eq!(c.id.as_ref().map(|i| i.filename.as_str()), Some(ADOBE));
    // Custom with nothing in use keeps what the edit names.
    let mut named = choice(LensProfileSetup::Auto, id("Gone.lcp"));
    named.set_setup(LensProfileSetup::Custom, None);
    assert_eq!(named, choice(LensProfileSetup::Custom, id("Gone.lcp")));
}

#[test]
fn menus_list_makes_models_and_profiles() {
    let m = photo();
    let menus = ProfileMenus::new(&m.lens_profiles, &m);
    assert_eq!(menus.makes(), ["Lensco", "Testcam"]);
    assert_eq!(menus.models("Testcam"), ["Testcam 35mm F2"]);
    let names: Vec<&str> = menus
        .profiles("Testcam", "Testcam 35mm F2")
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(names, ["Adobe (Testcam 35mm F2)", "Mine (Testcam 35mm F2)"]);
    assert_eq!(
        menus.first_of_make("Lensco").map(|p| p.filename.as_str()),
        Some(OTHER)
    );
}

#[test]
fn profiles_that_do_not_fit_the_camera_are_not_offered() {
    // A profile made on a smaller sensor of another make does not cover this one,
    // and a non-raw profile gives way to the raw one for the same lens.
    let small = test_profile("Tinycam", "12mm F2", "Adobe (Tinycam 12mm F2)", 0., -0.3)
        .replace(r#"SensorFormatFactor="1""#, r#"SensorFormatFactor="2""#);
    let jpeg = test_profile("Testcam", "35mm F2", "Adobe (Testcam 35mm F2)", 0., -0.3)
        .replace(r#"CameraRawProfile="True""#, r#"CameraRawProfile="False""#);
    let raw = test_profile("Testcam", "35mm F2", "Adobe (Testcam 35mm F2)", 0., -0.3);
    let mut m = photo();
    m.focal_35mm = 35.;
    let library = Library::from_texts([
        ("small.lcp", small.as_str()),
        ("jpeg.lcp", jpeg.as_str()),
        ("raw.lcp", raw.as_str()),
    ]);
    let offered = library.for_photo(&m);
    let names: Vec<&str> = offered
        .all()
        .iter()
        .map(|c| c.profile.filename.as_str())
        .collect();
    assert_eq!(names, ["raw.lcp"]);
}

#[test]
fn switching_to_custom_keeps_the_digest_of_the_same_profile() {
    let m = photo();
    let adobe = &m.lens_profiles.auto(&m).unwrap().profile;
    let recorded = LensProfileId {
        name: adobe.name.clone(),
        filename: adobe.filename.clone(),
        digest: "0123ABCD".into(),
        embedded: false,
    };
    let mut c = choice(LensProfileSetup::Auto, Some(recorded.clone()));
    c.set_setup(LensProfileSetup::Custom, Some(adobe));
    assert_eq!(c.id.as_ref(), Some(&recorded));
    // Picking the profile already named keeps it too; another one does not.
    c.choose(adobe);
    assert_eq!(c.id.as_ref(), Some(&recorded));
    let mine = &m
        .lens_profiles
        .all()
        .iter()
        .find(|c| c.profile.filename == MINE)
        .unwrap()
        .profile;
    c.choose(mine);
    assert!(c.id.as_ref().unwrap().digest.is_empty());
}

#[test]
fn default_and_auto_render_alike() {
    assert_eq!(
        choice(LensProfileSetup::Default, None).rendering(),
        choice(LensProfileSetup::Auto, None).rendering()
    );
    assert_eq!(
        choice(LensProfileSetup::Default, id(MINE)).rendering(),
        choice(LensProfileSetup::Auto, id(MINE)).rendering()
    );
    let mut digest = id(MINE);
    digest.as_mut().unwrap().digest = "0123ABCD".into();
    assert_eq!(
        choice(LensProfileSetup::Custom, digest).rendering(),
        choice(LensProfileSetup::Custom, id(MINE)).rendering()
    );
    assert_ne!(
        choice(LensProfileSetup::Auto, id(MINE)).rendering(),
        choice(LensProfileSetup::Custom, id(MINE)).rendering()
    );
}

#[test]
fn profiles_without_a_correction_model_are_not_offered() {
    let empty = test_profile("Testcam", "28mm F2", "Adobe (Testcam 28mm F2)", 0., 0.).replace(
        r#"<stCamera:PerspectiveModel><rdf:Description stCamera:RadialDistortParam1="0">
  <stCamera:VignetteModel stCamera:VignetteModelParam1="0"/>
 </rdf:Description></stCamera:PerspectiveModel>"#,
        "",
    );
    assert!(!empty.contains("PerspectiveModel"));
    let library = Library::from_texts([("empty.lcp", empty.as_str())]);
    assert!(library.for_photo(&photo()).all().is_empty());
}

#[test]
fn choosing_an_imported_profile_replaces_the_cameras_own() {
    let m = photo();
    let adobe = &m.lens_profiles.auto(&m).unwrap().profile;
    // An embedded identity with the same name as an imported profile.
    let mut c = choice(
        LensProfileSetup::Default,
        Some(LensProfileId {
            name: adobe.name.clone(),
            embedded: true,
            ..Default::default()
        }),
    );
    assert!(c.resolve(&m.lens_profiles, &m).used.is_none());
    c.choose(adobe);
    assert!(!c.id.as_ref().unwrap().embedded);
    assert_eq!(used(&c, &m).as_deref(), Some(ADOBE));
}

#[test]
fn a_profile_whose_entries_for_this_lens_have_no_model_is_not_its_match() {
    // One file with an empty entry for the photo's lens and a usable one for another.
    let usable = test_profile("Testcam", "50mm F2", "Adobe (Testcam 35/50)", 0.01, -0.4);
    let empty = test_profile("Testcam", "35mm F2", "Adobe (Testcam 35/50)", 0., 0.).replace(
        r#"<stCamera:PerspectiveModel><rdf:Description stCamera:RadialDistortParam1="0">
  <stCamera:VignetteModel stCamera:VignetteModelParam1="0"/>
 </rdf:Description></stCamera:PerspectiveModel>"#,
        "",
    );
    let entry = |t: &str| {
        let start = t.find("<rdf:li>").unwrap();
        let end = t.rfind("</rdf:li>").unwrap() + "</rdf:li>".len();
        t[start..end].to_string()
    };
    let both = usable.replace(&entry(&usable), &(entry(&empty) + &entry(&usable)));
    let mut m = photo();
    m.lens_profiles = Library::from_texts([("both.lcp", both.as_str())]).for_photo(&m);
    assert_eq!(m.lens_profiles.all().len(), 1);
    assert!(m.lens_profiles.auto(&m).is_none());
    let custom = choice(LensProfileSetup::Custom, id("both.lcp"));
    let candidate = custom.resolve(&m.lens_profiles, &m).used.unwrap();
    assert!(candidate.correction(&m).is_some());
}

#[test]
fn picking_one_of_two_files_with_the_same_name_records_its_file() {
    let text = test_profile("Testcam", "35mm F2", "Adobe (Testcam 35mm F2)", -0.05, -0.5);
    let mut m = photo();
    m.lens_profiles =
        Library::from_texts([("a.lcp", text.as_str()), ("b.lcp", text.as_str())]).for_photo(&m);
    let b = &m.lens_profiles.all()[1].profile;
    let mut c = choice(
        LensProfileSetup::Auto,
        Some(LensProfileId {
            name: b.name.clone(),
            digest: "0123ABCD".into(),
            ..Default::default()
        }),
    );
    c.choose(b);
    assert_eq!(used(&c, &m).as_deref(), Some("b.lcp"));
    assert_eq!(c.id.as_ref().unwrap().digest, "0123ABCD");
}

#[test]
fn entries_without_a_model_do_not_outrank_usable_ones() {
    // The photo's lens profiled on this make without a model, and on another make
    // with one: the usable entry corrects.
    let empty = test_profile("Testcam", "35mm F2", "Adobe (Testcam 35mm F2)", 0., 0.).replace(
        r#"<stCamera:PerspectiveModel><rdf:Description stCamera:RadialDistortParam1="0">
  <stCamera:VignetteModel stCamera:VignetteModelParam1="0"/>
 </rdf:Description></stCamera:PerspectiveModel>"#,
        "",
    );
    let other_make = test_profile("Lensco", "35mm F2", "Adobe (Testcam 35mm F2)", 0., -0.5);
    let entry = |t: &str| {
        let start = t.find("<rdf:li>").unwrap();
        let end = t.rfind("</rdf:li>").unwrap() + "</rdf:li>".len();
        t[start..end].to_string()
    };
    let both = other_make.replace(&entry(&other_make), &(entry(&empty) + &entry(&other_make)));
    let mut m = photo();
    m.lens_profiles = Library::from_texts([("both.lcp", both.as_str())]).for_photo(&m);
    let auto = m.lens_profiles.auto(&m).expect("the usable entry matches");
    assert!(auto.correction(&m).unwrap().vignetting_gain(1.) > 1.01);
}

#[test]
fn a_recorded_file_is_not_stood_in_for_by_another_of_the_same_name() {
    let m = photo();
    let other_file = LensProfileId {
        name: "Adobe (Testcam 35mm F2)".into(),
        filename: "Testcam (35mm F2) - copy.lcp".into(),
        ..Default::default()
    };
    let c = choice(LensProfileSetup::Custom, Some(other_file));
    let r = c.resolve(&m.lens_profiles, &m);
    assert!(r.used.is_none() && r.missing.is_some());
}

#[test]
fn profiles_whose_correction_does_not_validate_are_not_offered() {
    // A distortion model so strong its radial scale leaves the valid range.
    let wild = test_profile("Testcam", "28mm F2", "Adobe (Testcam 28mm F2)", 1000., 0.);
    let mut m = photo();
    m.lens_profiles = Library::from_texts([("wild.lcp", wild.as_str())]).for_photo(&m);
    let menus = ProfileMenus::new(&m.lens_profiles, &m);
    assert!(menus.profiles("Testcam", "Testcam 28mm F2").is_empty());
    assert!(menus.first_of_make("Testcam").is_none());
}
