//! Dense synthetic charts for fitting operators to Camera Raw (scripts/corpus): not
//! committed, written on request by the scripts.
use crate::chart::{Camera, Illuminant, SRGB_TO_XYZ, adapt, apply, mul};
use crate::dng;

/// Writes the chart RAWMAKASE_FIT_CHART describes, a JSON file
/// `{"columns": n, "patch": px, "gap": px, "colors": [[r, g, b] linear sRGB, ...]}`,
/// as the synthetic camera under D65: `<spec>.dng`, and `<spec>.patches.json` with
/// the area averaged in each patch.
#[test]
#[ignore = "Writes a fitting chart for scripts/corpus; needs RAWMAKASE_FIT_CHART"]
fn write_fit_chart() {
    let spec_path = std::path::PathBuf::from(std::env::var("RAWMAKASE_FIT_CHART").unwrap());
    let spec: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&spec_path).unwrap()).unwrap();
    let field = |k: &str| spec[k].as_u64().unwrap() as u32;
    let (columns, patch, gap) = (field("columns"), field("patch"), field("gap"));
    let colors: Vec<[f64; 3]> = serde_json::from_value(spec["colors"].clone()).unwrap();
    let rows = (colors.len() as u32).div_ceil(columns);
    let margin = 16;
    let cell = patch + gap;
    // Even sizes keep the mosaic phase identical in every patch.
    let w = (2 * margin + columns * cell).next_multiple_of(2);
    let h = (2 * margin + rows * cell).next_multiple_of(2);
    let camera = Camera::synthetic();
    let white = Illuminant::D65.white();
    let m = camera.matrix_for(Illuminant::D65.temperature());
    let to_camera = mul(
        &m,
        &mul(&adapt(Illuminant::D65.white(), white), &SRGB_TO_XYZ),
    );
    let neutral = apply(&m, white);
    let max = neutral.into_iter().fold(0., f64::max);
    let background = apply(&to_camera, [0.09; 3]).map(|v| v * 0.8);
    let mut pixels = vec![background; (w * h) as usize];
    let mut patches = Vec::new();
    for (i, c) in colors.iter().enumerate() {
        let (x0, y0) = (
            margin + i as u32 % columns * cell,
            margin + i as u32 / columns * cell,
        );
        let v = apply(&to_camera, *c).map(|v| (v * 0.8).clamp(0., 1.));
        for y in y0..y0 + patch {
            for x in x0..x0 + patch {
                pixels[(y * w + x) as usize] = v;
            }
        }
        patches.push(serde_json::json!({"x": x0 + patch / 4, "y": y0 + patch / 4, "w": patch / 2, "h": patch / 2}));
    }
    let bytes = dng::write(
        &dng::Image {
            width: w,
            height: h,
            camera: &pixels,
            as_shot_neutral: neutral.map(|v| v / max),
            profile: true,
            black_render: dng::BlackRender::Auto,
        },
        &camera,
    );
    std::fs::write(spec_path.with_extension("dng"), bytes).unwrap();
    std::fs::write(
        spec_path.with_extension("patches.json"),
        serde_json::to_vec(&patches).unwrap(),
    )
    .unwrap();
}
