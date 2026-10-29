use super::*;
use crate::lens::lensfun::Database;

fn calibration(xml: &str) -> Calibration {
    let db = Database::from_texts([("test.xml", xml)]).unwrap();
    db.lenses[0].calibration.clone()
}
fn whole(w: f32, h: f32, crop: f32) -> Frame {
    Frame {
        size: [w, h],
        rect: [0., 0., w, h],
        crop,
    }
}
/// Where `c` samples each channel for the output pixel at index `x`, `y` (Lensfun's
/// coordinates: pixel `i`'s centre at `i`), with no fill scale.
fn source(c: &LensCorrection, size: [f32; 2], x: f32, y: f32) -> [[f32; 2]; 3] {
    let [cx, cy] = c.centre_in(size);
    let half = size[0].hypot(size[1]) * 0.5;
    let (dx, dy) = (x + 0.5 - cx, y + 0.5 - cy);
    let s = c.radial_scale_ca(dx.hypot(dy) / half, 1., None);
    s.map(|s| [cx + dx * s - 0.5, cy + dy * s - 0.5])
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    xml: String,
    crop: f32,
    width: f32,
    height: f32,
    points: Vec<[f32; 2]>,
    shots: Vec<RefShot>,
}
#[derive(serde::Deserialize)]
struct RefShot {
    focal: f32,
    aperture: f32,
    distance: f32,
    d: Mode,
    t: Mode,
    dt: Mode,
    v: Mode,
}
#[derive(serde::Deserialize)]
struct Mode {
    values: Vec<Vec<f64>>,
}

/// Every synthetic calibration, evaluated as RAWmakase renders it, against what
/// Lensfun computes for the same photo (`tests/data/lensfun/reference.json`, from
/// Lensfun at revision bc5615d6): source positions within 0.02 px for distortion,
/// lateral CA and both, and vignetting gains within 0.01%.
#[test]
fn evaluations_match_lensfun() {
    let cases: Vec<Case> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/lensfun/reference.json"
    )))
    .unwrap();
    let mut worst = [0f64; 4];
    for case in &cases {
        let c = calibration(&case.xml);
        let frame = whole(case.width, case.height, case.crop);
        let size = frame.size;
        for shot in &case.shots {
            let e = c.evaluate(
                &frame,
                &Shot {
                    focal: shot.focal,
                    aperture: shot.aperture,
                    distance: shot.distance,
                },
                "test",
            );
            let full = e
                .correction
                .unwrap_or_else(|| panic!("{} at {}: {:?}", case.name, shot.focal, e.shortfalls));
            assert!(e.shortfalls.is_empty(), "{}: {:?}", case.name, e.shortfalls);
            let distortion_only = LensCorrection {
                chromatic: None,
                ..full.clone()
            };
            let chromatic_only = LensCorrection {
                distortion: None,
                ..full.clone()
            };
            for (i, [x, y]) in case.points.iter().copied().enumerate() {
                for (k, (correction, mode)) in [
                    (&distortion_only, &shot.d),
                    (&chromatic_only, &shot.t),
                    (&full, &shot.dt),
                ]
                .into_iter()
                .enumerate()
                {
                    let ours = source(correction, size, x, y);
                    let theirs = &mode.values[i];
                    for ch in 0..3 {
                        for axis in 0..2 {
                            let off = (f64::from(ours[ch][axis]) - theirs[ch * 2 + axis]).abs();
                            worst[k] = worst[k].max(off);
                            assert!(
                                off < 0.02,
                                "{} f={} mode {k} at ({x}, {y}) channel {ch}: {:?} against {:?}",
                                case.name,
                                shot.focal,
                                ours,
                                theirs
                            );
                        }
                    }
                }
                let [cx, cy] = full.centre_in(size);
                let r = (x + 0.5 - cx).hypot(y + 0.5 - cy) / (size[0].hypot(size[1]) * 0.5);
                let gain = f64::from(full.vignetting.as_ref().unwrap().eval(r));
                let theirs = shot.v.values[i][0];
                let off = (gain / theirs - 1.).abs();
                worst[3] = worst[3].max(off);
                assert!(
                    off < 1e-4,
                    "{} f={} at ({x}, {y}): gain {gain} against {theirs}",
                    case.name,
                    shot.focal
                );
            }
        }
    }
    eprintln!(
        "worst: distortion {:.4} px, CA {:.4} px, both {:.4} px, vignetting {:.2e}",
        worst[0], worst[1], worst[2], worst[3]
    );
}

const ZOOM: &str = r#"<lensdatabase version="2"><lens><maker>T</maker><model>T 10-80mm</model><mount>Generic</mount><cropfactor>1</cropfactor><calibration>
<distortion model="poly3" focal="10" k1="-0.08"/><distortion model="poly3" focal="20" k1="-0.02"/><distortion model="poly3" focal="40" k1="0.01"/><distortion model="poly3" focal="80" k1="0.02"/>
<tca model="poly3" focal="10" vr="1.001" vb="0.999" cr="0.01" cb="-0.01" br="0.02" bb="-0.02"/><tca model="poly3" focal="20" vr="1.003" vb="0.997" cr="0.03" cb="-0.03" br="0.04" bb="-0.04"/>
</calibration></lens></lensdatabase>"#;

/// Lensfun's interpolated coefficients for the same calibration (from its
/// `InterpolateDistortion` and `InterpolateTCA`): a Hermite spline in focal length,
/// held beyond the ends.
#[test]
fn focal_interpolation_is_lensfuns() {
    let c = calibration(ZOOM);
    for (focal, k1) in [
        (5., -0.08),
        (10., -0.08),
        (12., -0.060_533_326),
        (15., -0.041_666_664),
        (25., -0.009_5),
        (30., -0.001_666_666_6),
        (50., 0.013_437_5),
        (60., 0.016_25),
        (100., 0.02),
    ] {
        let Some(Distortion::Poly3([got])) = c.distortion_at(focal) else {
            panic!("{focal}");
        };
        assert!((got - k1).abs() < 2e-7, "{focal}: {got} against {k1}");
    }
    let Some(Tca::Poly3(t)) = c.tca_at(15.) else {
        panic!()
    };
    let lensfun = [
        1.002,
        0.998,
        0.023_333_33,
        -0.023_333_33,
        0.033_333_33,
        -0.033_333_33,
    ];
    for (a, b) in t.iter().zip(lensfun) {
        assert!((a - b).abs() < 1e-6, "{t:?}");
    }
}

/// Lensfun's interpolated vignetting (`InterpolateVignetting`): weighted by inverse
/// distance over focal length within the calibrated range, 4/N and 0.1/distance, none
/// beyond a distance of 1.
#[test]
fn vignetting_interpolation_is_lensfuns() {
    let c = calibration(
        r#"<lensdatabase version="2"><lens><maker>T</maker><model>T 10-20mm</model><mount>Generic</mount><cropfactor>1.5</cropfactor><calibration>
<vignetting model="pa" focal="10" aperture="2" distance="1000" k1="-0.4" k2="0.1" k3="-0.01"/>
<vignetting model="pa" focal="20" aperture="2" distance="1000" k1="-0.2" k2="0.05" k3="0"/>
<vignetting model="pa" focal="10" aperture="4" distance="1000" k1="-0.3" k2="0.0" k3="0.0"/>
<vignetting model="pa" focal="10" aperture="2" distance="1" k1="-0.5" k2="0.0" k3="0.0"/>
</calibration></lens></lensdatabase>"#,
    );
    for (q, k) in [
        ([10., 2., 1000.], Some([-0.4, 0.1, -0.01])),
        (
            [15., 2., 1000.],
            Some([-0.362_385_72, 0.050_106_8, -0.003_340_453]),
        ),
        (
            [10., 2.8, 1000.],
            Some([-0.358_915_98, 0.021_857_966, -0.002_095_66]),
        ),
        (
            [15., 3., 1000.],
            Some([-0.331_895_68, 0.024_526_618, -0.001_635_108]),
        ),
        (
            [10., 2., 10.],
            Some([-0.400_044_1, 0.099_955_86, -0.009_995_586]),
        ),
        ([10., 2., 1.], Some([-0.5, 0., 0.])),
        (
            [5., 2., 1000.],
            Some([-0.441_251_4, 0.050_158_676, -0.004_962_806]),
        ),
        ([20., 22., 1000.], None),
        ([40., 2., 1000.], None),
    ] {
        let got = c.vignetting_at(q[0], q[1], q[2]);
        match (got, k) {
            (Some(got), Some(k)) => {
                for (a, b) in got.iter().zip(k) {
                    assert!((a - b).abs() < 2e-6, "{q:?}: {got:?} against {k:?}");
                }
            }
            (None, None) => {}
            _ => panic!("{q:?}: {got:?} against {k:?}"),
        }
    }
    // A prime's focal lengths count in millimetres.
    let prime = calibration(
        r#"<lensdatabase version="2"><lens><maker>T</maker><model>T 50mm</model><mount>Generic</mount><cropfactor>1</cropfactor><calibration>
<vignetting model="pa" focal="50" aperture="2" distance="1000" k1="-0.4" k2="0" k3="0"/>
<vignetting model="pa" focal="50" aperture="4" distance="1000" k1="-0.2" k2="0" k3="0"/>
</calibration></lens></lensdatabase>"#,
    );
    assert!((prime.vignetting_at(50., 2.8, 1000.).unwrap()[0] + 0.253_517_93).abs() < 2e-6);
    assert!((prime.vignetting_at(50.5, 2., 1000.).unwrap()[0] + 0.388_712_38).abs() < 2e-6);
    assert!(prime.vignetting_at(60., 2.8, 1000.).is_none());
}

/// The tables RAWmakase samples stay within 0.02 px of the model at 6000 × 4000, for
/// strong distortion too, within the knots a table may have.
#[test]
fn sampled_tables_follow_the_model() {
    let strong = calibration(
        r#"<lensdatabase version="2"><lens><maker>T</maker><model>T 10mm</model><mount>Generic</mount><cropfactor>1</cropfactor><calibration>
<distortion model="ptlens" focal="10" a="0.05" b="-0.16" c="0.04"/>
<tca model="poly3" focal="10" vr="1.0004" vb="0.9995" cr="0.0003" cb="-0.0002" br="-0.0002" bb="0.0003"/>
</calibration></lens></lensdatabase>"#,
    );
    let frame = whole(6000., 4000., 1.);
    let shot = Shot {
        focal: 10.,
        aperture: 0.,
        distance: DISTANCE,
    };
    let c = strong.evaluate(&frame, &shot, "t").correction.unwrap();
    let d = c.distortion.as_ref().unwrap();
    assert!(d.knots.len() <= Radial::MAX_KNOTS);
    let model = strong.distortion_at(10.).unwrap();
    let geometry = Geometry::new(&strong, &frame).unwrap();
    let half = 6000f64.hypot(4000.) * 0.5;
    let worst = (0..=4000)
        .map(|i| {
            let rho = geometry.reach * f64::from(i) / 4000.;
            let ours = f64::from(d.eval(rho as f32));
            (ours - model.ratio(geometry.u(rho))).abs() * rho * half
        })
        .fold(0f64, f64::max);
    assert!(worst < 0.02, "{worst} px with {} knots", d.knots.len());
}

fn simple(extra: &str, crop: f32, body: &str) -> Calibration {
    calibration(&format!(
        r#"<lensdatabase version="2"><lens><maker>T</maker><model>T</model><mount>Generic</mount><cropfactor>{crop}</cropfactor>{extra}<calibration>{body}</calibration></lens></lensdatabase>"#
    ))
}

/// What a calibration can't do for a photo is said, and nothing wrong is applied.
#[test]
fn shortfalls_are_reported_and_nothing_is_guessed() {
    let shot = |focal, aperture| Shot {
        focal,
        aperture,
        distance: DISTANCE,
    };
    let frame = whole(6000., 4000., 1.);
    let zoom = simple(
        "",
        1.,
        r#"<distortion model="poly3" focal="24" k1="-0.02"/><distortion model="poly3" focal="70" k1="0.01"/>
<vignetting model="pa" focal="24" aperture="4" distance="1000" k1="-0.4" k2="0" k3="0"/>"#,
    );
    // A zoom without a focal length: nothing.
    let e = zoom.evaluate(&frame, &shot(0., 4.), "t");
    assert_eq!((e.correction, e.shortfalls), (None, vec![Shortfall::Focal]));
    // Without an aperture: distortion, no vignetting.
    let e = zoom.evaluate(&frame, &shot(24., 0.), "t");
    let c = e.correction.unwrap();
    assert!(c.distortion.is_some() && c.vignetting.is_none());
    assert_eq!(e.shortfalls, vec![Shortfall::Aperture]);
    // Far from any vignetting row: distortion only.
    let e = zoom.evaluate(&frame, &shot(70., 22.), "t");
    assert!(e.correction.unwrap().vignetting.is_none());
    assert_eq!(e.shortfalls, vec![Shortfall::VignettingCoverage]);
    // A prime without a focal length is its own.
    let prime = simple(
        "",
        1.,
        r#"<distortion model="poly3" focal="35" k1="-0.02"/>"#,
    );
    assert!(
        prime
            .evaluate(&frame, &shot(0., 0.), "t")
            .correction
            .is_some()
    );
    // An APS-C calibration on full frame doesn't cover the corners; 4% less is
    // Lensfun's tolerance.
    let aps = simple(
        "",
        1.5,
        r#"<distortion model="poly3" focal="35" k1="-0.02"/>"#,
    );
    assert_eq!(
        aps.evaluate(&frame, &shot(35., 0.), "t").shortfalls,
        vec![Shortfall::Coverage]
    );
    assert!(
        aps.evaluate(&whole(6000., 4000., 1.44), &shot(35., 0.), "t")
            .correction
            .is_some()
    );
    // Not rectilinear.
    let fisheye = simple(
        "<type>fisheye</type>",
        1.,
        r#"<distortion model="poly3" focal="8" k1="-0.02"/>"#,
    );
    let e = fisheye.evaluate(&frame, &shot(8., 0.), "t");
    assert_eq!(e.correction, None);
    assert_eq!(e.shortfalls, vec![Shortfall::Projection(LensType::Fisheye)]);
    // Vignetting whose falloff reaches zero inside the photo is left out; the
    // distortion stays.
    let bad = simple(
        "",
        1.,
        r#"<distortion model="poly3" focal="35" k1="-0.02"/><vignetting model="pa" focal="35" aperture="2" distance="1000" k1="-1.5" k2="0" k3="0"/>"#,
    );
    let e = bad.evaluate(&frame, &shot(35., 2.), "t");
    assert!(e.correction.as_ref().unwrap().distortion.is_some());
    assert!(e.correction.unwrap().vignetting.is_none());
    assert_eq!(e.shortfalls, vec![Shortfall::OutOfRange("vignetting")]);
}

/// The decoded image's margins beyond the default crop: the photo frame sets the
/// scale and centre, the decoded image the radius unit.
#[test]
fn a_default_crop_inside_the_decoded_image_keeps_the_frame_s_scale() {
    let c = simple(
        "",
        1.,
        r#"<distortion model="poly5" focal="50" k1="0.1" k2="0"/>"#,
    );
    let shot = Shot {
        focal: 50.,
        aperture: 0.,
        distance: DISTANCE,
    };
    let bare = c
        .evaluate(&whole(6000., 4000., 1.), &shot, "t")
        .correction
        .unwrap();
    // The same frame inside a decoded image with 40 px left and 24 px top margins.
    let framed = Frame {
        size: [6080., 4048.],
        rect: [40., 24., 6000., 4000.],
        crop: 1.,
    };
    let margins = c.evaluate(&framed, &shot, "t").correction.unwrap();
    for [x, y] in [[0f32, 0f32], [5999., 1999.5], [3000., 3999.]] {
        let a = source(&bare, [6000., 4000.], x, y)[1];
        let b = source(&margins, [6080., 4048.], x + 40., y + 24.)[1];
        assert!(
            (a[0] + 40. - b[0]).abs() < 0.01 && (a[1] + 24. - b[1]).abs() < 0.01,
            "{a:?} {b:?}"
        );
    }
}

/// A saved calibration is small and reads back the same.
#[test]
fn calibrations_round_trip_compactly() {
    let c = calibration(ZOOM);
    let text = serde_json::to_string(&c).unwrap();
    assert_eq!(serde_json::from_str::<Calibration>(&text).unwrap(), c);
    let row = VignettingRow {
        focal: 24.,
        aperture: 4.,
        distance: 1000.,
        k: [-0.5, 0.1, -0.01],
    };
    assert_eq!(
        serde_json::to_string(&row).unwrap(),
        "[24.0,4.0,1000.0,-0.5,0.1,-0.01]"
    );
    assert!(c.validate());
    assert!(!Calibration { crop: 0., ..c }.validate());
}

/// Every lens of a real Lensfun database parses, and every calibration evaluates
/// within the knots a table may have and 0.02 px of its model at 6000 × 4000, at
/// both ends of its focal range. Reads the database's `data/db` folder.
#[test]
#[ignore = "Read-only local Lensfun database; set RAWMAKASE_LENSFUN_DB to its data/db folder"]
fn every_real_calibration_evaluates() {
    let dir = std::path::PathBuf::from(std::env::var_os("RAWMAKASE_LENSFUN_DB").unwrap());
    let db = crate::lens::lensfun::installed::read_folder(&dir).unwrap();
    let (mut most, mut evaluated, mut worst) = (0, 0, 0f64);
    for lens in &db.lenses {
        let c = &lens.calibration;
        if lens.refusal().is_some() {
            continue;
        }
        let frame = whole(6000., 4000., c.crop);
        for focal in c.focal {
            let shot = Shot {
                focal,
                aperture: 4.,
                distance: DISTANCE,
            };
            let e = c.evaluate(&frame, &shot, "t");
            let Some(correction) = e.correction else {
                continue;
            };
            evaluated += 1;
            let tables = correction
                .distortion
                .iter()
                .chain(correction.vignetting.iter())
                .chain(correction.chromatic.iter().flatten());
            most = most.max(tables.map(|t| t.knots.len()).max().unwrap_or(0));
            if let (Some(d), Some(model)) = (&correction.distortion, c.distortion_at(focal)) {
                let g = Geometry::new(c, &frame).unwrap();
                let half = 6000f64.hypot(4000.) * 0.5;
                for i in 0..=2000 {
                    let rho = g.reach * f64::from(i) / 2000.;
                    let off =
                        (f64::from(d.eval(rho as f32)) - model.ratio(g.u(rho))).abs() * rho * half;
                    worst = worst.max(off);
                    assert!(
                        off < 0.02,
                        "{} at {focal}: {off} px at {rho} of {}, {} knots",
                        lens.model.id,
                        g.reach,
                        d.knots.len()
                    );
                }
            }
        }
    }
    eprintln!(
        "{} lenses, {evaluated} evaluated, at most {most} knots, worst {worst:.4} px",
        db.lenses.len()
    );
}

/// A calibration the table's knots can't follow within 0.02 px at the photo's size is
/// refused, not applied less accurately than the model.
#[test]
fn a_table_that_can_t_follow_the_model_is_refused() {
    let c = simple(
        "",
        1.,
        r#"<distortion model="poly5" focal="50" k1="-0.61538464" k2="0.18934911"/>"#,
    );
    let e = c.evaluate(
        &whole(6000., 4000., 1.),
        &Shot {
            focal: 50.,
            aperture: 0.,
            distance: DISTANCE,
        },
        "t",
    );
    match &e.correction {
        Some(c) => {
            // Within budget after all: then within 0.02 px.
            let d = c.distortion.as_ref().unwrap();
            assert!(d.knots.len() <= Radial::MAX_KNOTS);
        }
        None => assert_eq!(e.shortfalls, vec![Shortfall::OutOfRange("distortion")]),
    }
    assert!(
        e.correction.is_none() || e.shortfalls.is_empty(),
        "{:?}",
        e.shortfalls
    );
    let g = Geometry::new(&c, &whole(6000., 4000., 1.)).unwrap();
    if let Some(corr) = &e.correction {
        let d = corr.distortion.as_ref().unwrap();
        let model = c.distortion_at(50.).unwrap();
        let half = 6000f64.hypot(4000.) * 0.5;
        let worst = (0..=4000)
            .map(|i| {
                let rho = g.reach * f64::from(i) / 4000.;
                (f64::from(d.eval(rho as f32)) - model.ratio(g.u(rho))).abs() * rho * half
            })
            .fold(0f64, f64::max);
        assert!(worst <= 0.02, "{worst}");
    }
}
