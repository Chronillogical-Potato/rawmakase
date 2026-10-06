//! Rendering for the oldest engines, kept as it was so saved edits still render the same.
use super::*;

pub fn render_legacy(im: &CameraImage, r: &Recipe, max_edge: u32) -> Result<Rendered> {
    r.validate()?;
    let im = legacy_retouched(im, r);
    render_legacy_inner(&im, &r.resolved(&im.metadata), max_edge)
}
/// The camera image with the recipe's red eye corrections and spots applied, for the
/// older engines, which develop without highlight recovery or the retouch cache.
pub(super) fn legacy_retouched<'a>(
    im: &'a CameraImage,
    r: &Recipe,
) -> std::borrow::Cow<'a, CameraImage> {
    let shown = r.as_rendered();
    let ops = crate::develop::retouch::Retouching::of(&shown);
    if ops.is_empty() {
        std::borrow::Cow::Borrowed(im)
    } else {
        std::borrow::Cow::Owned(crate::develop::retouch::apply(im, ops))
    }
}
pub(super) fn render_legacy_inner(im: &CameraImage, r: &Recipe, max_edge: u32) -> Result<Rendered> {
    let matrix = profile_matrix(&im.metadata, r);
    let lut = CurveSet::for_image(im.into(), r, matrix, false);
    let full_geometry = Geometry::new(im, r, 0);
    if max_edge > 0 && full_geometry.width.max(full_geometry.height) > max_edge {
        let mut base = r.clone();
        base.sharpening = 0.;
        let full = render_legacy_inner(im, &base, 0)?;
        let g = Geometry::new(im, r, max_edge);
        let mut pixels = vec![[0.; 3]; g.width as usize * g.height as usize];
        pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
            let x = i as u32 % g.width;
            let y = i as u32 / g.width;
            let x0 = x * full.width / g.width;
            let x1 = ((x + 1) * full.width / g.width).max(x0 + 1);
            let y0 = y * full.height / g.height;
            let y1 = ((y + 1) * full.height / g.height).max(y0 + 1);
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let q = full.pixels[(yy * full.width + xx) as usize];
                    for c in 0..3 {
                        p[c] += q[c];
                    }
                }
            }
            let count = ((x1 - x0) * (y1 - y0)) as f32;
            for v in p {
                *v /= count;
            }
        });
        sharpen(&mut pixels, g.width, g.height, r.sharpening);
        return Ok(Rendered {
            width: g.width,
            height: g.height,
            pixels,
        });
    }
    let g = full_geometry;
    let warp = LensWarp::new(im, r);
    let mut pixels = vec![[0.; 3]; g.width as usize * g.height as usize];
    pixels.par_iter_mut().enumerate().for_each(|(i, out)| {
        let x = i as u32 % g.width;
        let y = i as u32 / g.width;
        let [sx, sy] = g.source(
            (x as f32 + 0.5) / g.width as f32,
            (y as f32 + 0.5) / g.height as f32,
        );
        if g.outside(sx, sy) {
            *out = [1.; 3];
            return;
        }
        let p = match &warp {
            Some(w) => w.sample(im.into(), sx, sy, r, 0.),
            None => detail_sample(im.into(), sx, sy, r),
        };
        *out = process_pixel(p, &im.metadata, r, &lut, matrix, [sx, sy], None);
    });
    sharpen(&mut pixels, g.width, g.height, r.sharpening);
    Ok(Rendered {
        width: g.width,
        height: g.height,
        pixels,
    })
}
pub(super) fn sharpen(pixels: &mut Vec<[f32; 3]>, width: u32, height: u32, amount: f32) {
    if amount <= 0. {
        return;
    }
    let src = &*pixels;
    let mut dst = vec![[0.; 3]; pixels.len()];
    dst.par_iter_mut().enumerate().for_each(|(i, out)| {
        let x = i as u32 % width;
        let y = i as u32 / width;
        let mut avg = [0.; 3];
        let mut n = 0.;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let xx = (x as i64 + dx).clamp(0, width as i64 - 1) as u32;
                let yy = (y as i64 + dy).clamp(0, height as i64 - 1) as u32;
                let q = src[(yy * width + xx) as usize];
                for c in 0..3 {
                    avg[c] += q[c];
                }
                n += 1.;
            }
        }
        for c in 0..3 {
            out[c] = (src[i][c] + (src[i][c] - avg[c] / n) * amount).clamp(0., 1.);
        }
    });
    *pixels = dst;
}

/// Render a rectangle of the full output at one sample per output pixel.
pub fn render_region_legacy(im: &CameraImage, r: &Recipe, region: [u32; 4]) -> Result<Rendered> {
    let im = legacy_retouched(im, r);
    let im = im.as_ref();
    let r = r.resolved(&im.metadata);
    render_region_inner(
        im.into(),
        &r,
        &Geometry::new(im, &r, 0),
        region,
        0.,
        &std::sync::atomic::AtomicBool::new(false),
    )
}
pub(super) fn render_region_inner(
    im: Source,
    r: &Recipe,
    g: &Geometry,
    region: [u32; 4],
    spread: f32,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Rendered> {
    r.validate()?;
    let matrix = profile_matrix(&im.metadata, r);
    let lut = CurveSet::for_image(im, r, matrix, false);
    let [x0, y0, w, h] = region;
    ensure!(
        w > 0
            && h > 0
            && x0.checked_add(w).is_some_and(|r| r <= g.width)
            && y0.checked_add(h).is_some_and(|b| b <= g.height),
        "Invalid viewport region"
    );
    let mut pixels = vec![[0.; 3]; w as usize * h as usize];
    let warp = LensWarp::new(&im, r);
    let at = |x: u32, y: u32| {
        let [sx, sy] = g.source(
            (x as f32 + 0.5) / g.width as f32,
            (y as f32 + 0.5) / g.height as f32,
        );
        if g.outside(sx, sy) {
            return [1.; 3];
        }
        let p = match &warp {
            Some(w) => w.sample(im, sx, sy, r, spread),
            None => footprint_sample(im, sx, sy, r, spread),
        };
        process_pixel(p, &im.metadata, r, &lut, matrix, [sx, sy], None)
    };
    pixels.par_iter_mut().enumerate().for_each(|(i, p)| {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let x = x0 + i as u32 % w;
        let y = y0 + i as u32 / w;
        *p = at(x, y);
        if r.sharpening > 0. {
            let mut avg = [0.; 3];
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let q = at(
                        (x as i64 + dx).clamp(0, g.width as i64 - 1) as u32,
                        (y as i64 + dy).clamp(0, g.height as i64 - 1) as u32,
                    );
                    for c in 0..3 {
                        avg[c] += q[c] / 9.;
                    }
                }
            }
            for c in 0..3 {
                p[c] = (p[c] + (p[c] - avg[c]) * r.sharpening).clamp(0., 1.);
            }
        }
    });
    ensure!(
        !cancel.load(std::sync::atomic::Ordering::Relaxed),
        "Render superseded"
    );
    Ok(Rendered {
        width: w,
        height: h,
        pixels,
    })
}
