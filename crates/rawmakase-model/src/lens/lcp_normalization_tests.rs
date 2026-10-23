//! Invented polynomials; no installed profile data belongs in these regressions.
use super::*;

// Each model describes the same image-space polynomial despite using different
// focal normalizations. The sensor is 3:2, so (half diagonal / long edge)^2 = 13/36.
fn profile(explicit: bool, nested: bool) -> String {
    let mut entries = String::new();
    for (focal, fx) in [(36., 1.), (144., 4.)] {
        for (av, aperture_scale) in [(2., 1.), (4., 0.5)] {
            let fx = if explicit { fx * (1. + av / 10.) } else { fx };
            let coordinate = |f: f32| {
                if explicit {
                    format!(r#" stCamera:FocalLengthX="{f}""#)
                } else {
                    String::new()
                }
            };
            let params = |prefix: &str, f: f32, k: [f32; 3]| {
                (0..3)
                    .map(|i| {
                        format!(
                            r#" stCamera:{prefix}{}="{}""#,
                            i + 1,
                            k[i] * (f * f * 36. / 13.).powi(i as i32 + 1)
                        )
                    })
                    .collect::<String>()
            };
            let vfx = if nested {
                fx * 2.
            } else if explicit {
                focal / 36.
            } else {
                fx
            };
            let rfx = if nested { fx * 3. } else { focal / 36. };
            let bfx = if nested { fx * 4. } else { focal / 36. };
            let nested_coordinate = |f| if nested { coordinate(f) } else { String::new() };
            entries.push_str(&format!(r#"
<rdf:li><rdf:Description stCamera:Make="Synthetic" stCamera:Lens="Zoom" stCamera:CameraRawProfile="True"
 stCamera:FocalLength="{focal}" stCamera:ApertureValue="{av}" stCamera:SensorFormatFactor="1">
 <stCamera:PerspectiveModel><rdf:Description{}{}>
  <stCamera:VignetteModel{}{} />
  <stCamera:ChromaticRedGreenModel stCamera:ScaleFactor="1.001"{}{} />
  <stCamera:ChromaticBlueGreenModel stCamera:ScaleFactor="0.999"{}{} />
 </rdf:Description></stCamera:PerspectiveModel>
</rdf:Description></rdf:li>"#,
                coordinate(fx), params("RadialDistortParam", fx, [0.02, -0.01, 0.005]),
                nested_coordinate(vfx), params("VignetteModelParam", vfx, [-0.6 * aperture_scale, 0.2 * aperture_scale, -0.1 * aperture_scale]),
                nested_coordinate(rfx), params("RadialDistortParam", rfx, [0.002, -0.001, 0.0005]),
                nested_coordinate(bfx), params("RadialDistortParam", bfx, [-0.002, 0.001, -0.0005]),
            ));
        }
    }
    format!(
        r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:stCamera="http://ns.adobe.com/photoshop/1.0/camera-profile">{entries}</rdf:RDF>"#
    )
}

#[test]
fn zoom_normalization_survives_focal_and_aperture_interpolation() {
    for (explicit, nested) in [(false, false), (true, false), (true, true)] {
        let entries = parse(&profile(explicit, nested)).unwrap();
        for focal in [18., 36., 63., 90., 144., 288.] {
            for (av, strength) in [(0., 1.), (2., 1.), (3., 0.75), (4., 0.5), (6., 0.5)] {
                for crop in [1., 1.5] {
                    let m = Metadata {
                        make: "Synthetic".into(),
                        lens_model: "Zoom".into(),
                        focal,
                        focal_35mm: focal * crop,
                        aperture: 2f32.powf(av / 2.),
                        width: 6000,
                        height: 4000,
                        ..Default::default()
                    };
                    let c = correction(&entries, &m).expect("valid zoom correction");
                    for i in 0..=32 {
                        let r = i as f32 / 32.;
                        let r2 = (r / crop).powi(2);
                        let d = 0.02 * r2 - 0.01 * r2.powi(2) + 0.005 * r2.powi(3);
                        let falloff =
                            1. + strength * (-0.6 * r2 + 0.2 * r2.powi(2) - 0.1 * r2.powi(3));
                        let context = format!(
                            "explicit={explicit} nested={nested} focal={focal} av={av} crop={crop} r={r}"
                        );
                        assert!(
                            (c.distortion.as_ref().unwrap().eval(r) - (1. + d)).abs() < 2e-6,
                            "distortion {context}"
                        );
                        assert!(
                            (c.vignetting_gain(r) - 1. / falloff).abs() < 2e-6,
                            "vignetting {context}"
                        );
                        let ca = c.chromatic.as_ref().unwrap();
                        assert!(
                            (ca[0].eval(r) - 1.001 * (1. + d / 10.)).abs() < 2e-6,
                            "red {context}"
                        );
                        assert!(
                            (ca[1].eval(r) - 0.999 * (1. - d / 10.)).abs() < 2e-6,
                            "blue {context}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn zoom_normalization_is_independent_of_entry_order_and_frame_orientation() {
    let mut entries = parse(&profile(true, true)).unwrap();
    let mut m = Metadata {
        make: "Synthetic".into(),
        lens_model: "Zoom".into(),
        focal: 90.,
        aperture: 2f32.powf(1.5),
        width: 6000,
        height: 4000,
        ..Default::default()
    };
    let expected = correction(&entries, &m).unwrap();
    entries.reverse();
    // Distortion/CA vary only in normalization across aperture samples; either
    // selected sample must describe the same correction.
    m.width = 4000;
    m.height = 6000;
    let actual = correction(&entries, &m).unwrap();
    for (a, b) in actual
        .vignetting
        .unwrap()
        .values
        .iter()
        .zip(expected.vignetting.unwrap().values)
    {
        assert!((a - b).abs() < 2e-6);
    }
    let portrait = correction(&entries, &m).unwrap();
    m.width = 6200;
    m.height = 4200;
    m.crop_width = 4000;
    m.crop_height = 6000;
    assert_eq!(correction(&entries, &m).unwrap(), portrait);
}

#[test]
fn zoom_interpolates_attenuation_before_inverting_to_gain() {
    let mut entries = parse(&profile(true, true)).unwrap();
    for e in &mut entries {
        if e.focal == 144. {
            for k in &mut e.vignette.as_mut().unwrap().params {
                *k *= 0.5;
            }
        }
    }
    let m = Metadata {
        make: "Synthetic".into(),
        lens_model: "Zoom".into(),
        focal: 90.,
        aperture: 2f32.powf(1.5),
        width: 6000,
        height: 4000,
        ..Default::default()
    };
    let c = correction(&entries, &m).unwrap();
    // Halfway in focal and APEX aperture: bilinear attenuation strength .75².
    assert!((c.vignetting_gain(1.) - 1. / (1. - 0.5 * 0.75 * 0.75)).abs() < 2e-6);
}

#[test]
#[ignore = "Read-only local LCP audit; set RAWMAKASE_LCP to an installed profile"]
fn installed_zoom_profile_matches_its_calibration_samples() {
    let path = std::env::var_os("RAWMAKASE_LCP").expect("set RAWMAKASE_LCP");
    let entries = parse(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut compared = 0;
    for e in &entries {
        let m = Metadata {
            make: e.make.clone(),
            lens_model: e.lens[0].clone(),
            focal: e.focal,
            aperture: e.aperture.map_or(0., |av| 2f32.powf(av / 2.)),
            width: 6000,
            height: 4000,
            ..Default::default()
        };
        let single = correction(std::slice::from_ref(e), &m).expect("valid calibration sample");
        let polynomial = |p: RadialModel, r: f64| {
            let fx = p
                .focal_x
                .map(f64::from)
                .unwrap_or(e.focal as f64 * e.sensor_factor as f64 / 36.);
            let r2 = r * r * 13. / (36. * fx * fx);
            1. + p.params[0] as f64 * r2
                + p.params[1] as f64 * r2.powi(2)
                + p.params[2] as f64 * r2.powi(3)
        };
        for i in 0..=32 {
            let r = i as f64 / 32.;
            if let Some(p) = e.vignette {
                let expected = 1. / polynomial(p, r).max(0.2);
                assert!((single.vignetting_gain(r as f32) as f64 - expected).abs() < 2e-5);
            }
            if let Some(p) = e.distortion {
                assert!(
                    (single.distortion.as_ref().unwrap().eval(r as f32) as f64 - polynomial(p, r))
                        .abs()
                        < 2e-5
                );
            }
        }
        // The full zoom profile must select the same farthest-focus vignette at
        // its exact focal/aperture, regardless of where that sample is in the file.
        if e.vignette.is_some()
            && !entries.iter().any(|other| {
                other.vignette.is_some()
                    && other.focal == e.focal
                    && other.distance.unwrap_or(f32::INFINITY) > e.distance.unwrap_or(f32::INFINITY)
            })
        {
            let full = correction(&entries, &m).expect("valid full zoom correction");
            for i in 0..=32 {
                let r = i as f32 / 32.;
                assert!((full.vignetting_gain(r) - single.vignetting_gain(r)).abs() < 2e-5);
            }
            compared += 1;
        }
    }
    assert!(compared > 0);
    eprintln!(
        "Checked {} calibration samples and {compared} full-profile vignette endpoints",
        entries.len()
    );
}
