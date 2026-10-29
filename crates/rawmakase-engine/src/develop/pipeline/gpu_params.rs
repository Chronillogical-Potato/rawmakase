//! Parameters the GPU port of the pipeline takes, built from the same recipe and image.
use super::*;

/// Lens correction for `gpu/local.wgsl`, from `S_LENS` to `S_GAIN_MAP`, with radial
/// tables appended to `tables` (each: knots, then values) at offsets counted from
/// `base`, and the GainMap's (rows, columns, origin, spacing, the image's width and
/// height, the amount, the area, 1 when it applies after the warp, then three gains per
/// point). Offsets are -1 for absent tables.
pub(crate) fn lens_gpu_params(
    im: &CameraImage,
    r: &Recipe,
    base: usize,
    tables: &mut Vec<f32>,
) -> [f32; 16] {
    let mut push = |radial: Option<&crate::optics::Radial>| match radial {
        Some(radial) => {
            let at = base + tables.len();
            tables.extend(&radial.knots);
            tables.extend(&radial.values);
            [at as f32, radial.knots.len() as f32]
        }
        None => [-1., 0.],
    };
    let mut out = [0.; 16];
    let Some(warp) = LensWarp::new(im, r) else {
        out[5..13].copy_from_slice(&[-1., 0., -1., 0., -1., 0., -1., 0.]);
        out[15] = -1.;
        return out;
    };
    let lens = &*warp.map.lens;
    let distortion = push(lens.distortion.as_ref());
    let [red, blue] = match warp.map.chromatic.or(lens.chromatic.as_ref()) {
        Some([red, blue]) => [push(Some(red)), push(Some(blue))],
        None => [[-1., 0.]; 2],
    };
    let vignetting = push(warp.vignetting.as_ref().map(VignetteField::table));
    let m = &warp.map;
    // 2 marks an aberration evaluated at the distorted radius: a measured one, or lens
    // data given as a function of it (Lensfun's).
    let mode = if m.chromatic.is_some() || lens.chromatic_at_source {
        2.
    } else {
        1.
    };
    out[..5].copy_from_slice(&[mode, m.center[0], m.center[1], m.half, m.fill]);
    out[5] = m.amount;
    out[6..8].copy_from_slice(&distortion);
    out[8..10].copy_from_slice(&red);
    out[10..12].copy_from_slice(&blue);
    out[12..14].copy_from_slice(&vignetting);
    out[14] = warp.vignetting.as_ref().map_or(0., |v| v.amount);
    out[15] = warp.gain_map.as_ref().map_or(-1., |g| {
        let at = base + tables.len();
        let grid = &*g.grid;
        tables.extend([grid.points[0] as f32, grid.points[1] as f32]);
        tables.extend(grid.origin);
        tables.extend(grid.spacing);
        tables.extend(g.size);
        tables.push(g.amount);
        tables.extend(grid.area);
        tables.push(if grid.after_warp { 1. } else { 0. });
        tables.extend((0..grid.points[0] * grid.points[1]).flat_map(|i| {
            std::array::from_fn::<f32, 3, _>(|c| {
                grid.gains[i * grid.planes + c.min(grid.planes - 1)]
            })
        }));
        at as f32
    });
    out
}
