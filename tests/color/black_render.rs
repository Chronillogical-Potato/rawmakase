//! A profile whose DefaultBlackRender is None (Adobe's camera-matching profiles
//! carry it) keeps the shadows Camera Raw keeps: no default black is subtracted.
use crate::{Layout, cases, chart, develop, dng, embedded_profiles, measure, render};
use rawmakase::camera_profiles::BlackRender;

/// Camera Raw 18.7's gray ramp from −8 to −4 EV (mean encoded sRGB, 0–1) for the
/// synthetic D65 chart with DefaultBlackRender None, at the cases' base settings.
const CAMERA_RAW: [f64; 9] = [
    0.0058, 0.0081, 0.0118, 0.0168, 0.0242, 0.0339, 0.0464, 0.0618, 0.0819,
];

#[test]
fn black_render_none_keeps_camera_raw_shadows() {
    let layout = Layout::new();
    let camera = chart::Camera::synthetic();
    let rendered = chart::render(&layout, &camera, chart::Illuminant::D65);
    let bytes = dng::write(
        &dng::Image {
            width: chart::WIDTH,
            height: chart::HEIGHT,
            camera: &rendered.camera,
            as_shot_neutral: rendered.as_shot_neutral,
            profile: true,
            black_render: dng::BlackRender::None,
        },
        &camera,
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("black-render-none.dng");
    std::fs::write(&path, bytes).unwrap();
    let im = develop(&path);
    let profiles = embedded_profiles(&im);
    assert_eq!(profiles[0].black_render(), BlackRender::None);
    let cases = cases();
    let default = cases.cases.iter().find(|c| c.name == "default").unwrap();
    let out = render(&im, &profiles, &default.xmp(&cases.base, &[]), 0).unwrap();
    let ramp: Vec<_> = layout
        .patches
        .iter()
        .filter(|p| p.group == "ramp")
        .take(CAMERA_RAW.len())
        .cloned()
        .collect();
    let values = measure::patches(out.width, &out.pixels, &ramp);
    for ((patch, v), want) in ramp.iter().zip(values).zip(CAMERA_RAW) {
        let got = v.iter().map(|&c| c as f64).sum::<f64>() / 3. / 65535.;
        assert!(
            (got - want).abs() <= 0.001,
            "{}: {got:.4}, Camera Raw {want:.4}",
            patch.name
        );
    }
}
