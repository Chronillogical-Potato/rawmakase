//! Automatic Subject and Sky selection from what the models proposed.
//!
//! Segment Anything 2 answers a prompt with an object's outline but does not know which
//! object matters; IS-Net knows what is salient but draws a soft matte. Subject therefore
//! keeps the outlines SAM 2 drew that lie inside the saliency and fills any gap with the
//! saliency itself, both edges moved onto the photo's. Sky has no model of its own: the
//! outlines that reach the top of the photo and look like sky (smooth, and bright or
//! blue) are kept, down to where the sky ends. Everything here is pure: the proposals
//! and the saliency come in, coverage in the photo's frame goes out.
use crate::error::InferenceError;
use crate::process::{Coverage, Rect, RgbImage, output_size, resample};
use crate::refine::{guided, mean, smooth};

/// Side of the square grid the proposals and the saliency live on: the photo
/// stretched to a square, as the models see it.
pub const SIDE: usize = 256;

/// One outline SAM 2 drew: a mask on the [`SIDE`] grid and the model's own estimate of
/// its quality.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub mask: Vec<bool>,
    pub score: f32,
}

impl Proposal {
    fn area(&self) -> f32 {
        self.mask.iter().filter(|m| **m).count() as f32 / (SIDE * SIDE) as f32
    }
}

/// Subject: the outlines inside the saliency, with the saliency filling what they miss.
/// All zero when nothing is salient.
pub fn subject(
    image: &RgbImage,
    proposals: &[Proposal],
    saliency: &[f32],
) -> Result<Coverage, InferenceError> {
    check(saliency.len())?;
    let (width, height) = output_size(image);
    let mass: f32 = saliency.iter().sum();
    if mass < 0.002 * (SIDE * SIDE) as f32 {
        return Ok(empty(width, height));
    }
    // Where an outline may reach: near the salient area.
    let near = dilate(&saliency.iter().map(|s| *s > 0.3).collect::<Vec<_>>(), 4);
    let mut union = vec![0f32; SIDE * SIDE];
    for p in proposals {
        let inside: f32 = p
            .mask
            .iter()
            .zip(saliency)
            .filter(|(m, _)| **m)
            .map(|(_, s)| *s)
            .sum();
        let count = p.mask.iter().filter(|m| **m).count() as f32;
        if count < 0.003 * (SIDE * SIDE) as f32 || inside / count <= 0.8 {
            continue;
        }
        for (i, m) in p.mask.iter().enumerate() {
            if *m && near[i] {
                union[i] = 1.0;
            }
        }
    }
    let guide = luma(image, width, height);
    let radius = |div: usize| (width.max(height) / div).max(3);
    let soft = expand(saliency, width, height);
    let from_saliency = guided(&guide, &soft, width, height, radius(160), 2e-3);
    let from_outlines = guided(
        &guide,
        &expand(&union, width, height),
        width,
        height,
        radius(200),
        1e-3,
    );
    let data = from_saliency
        .iter()
        .zip(&from_outlines)
        .map(|(a, b)| (smooth(a.max(*b)) * 255.0 + 0.5) as u8)
        .collect();
    Ok(Coverage {
        width,
        height,
        data,
    })
}

/// Sky: the outlines that reach the top and look like sky, cut where the sky ends.
/// All zero when there is none.
pub fn sky(image: &RgbImage, proposals: &[Proposal]) -> Result<Coverage, InferenceError> {
    let (width, height) = output_size(image);
    let small = grid(image);
    let lum: Vec<f32> = (0..SIDE * SIDE)
        .map(|i| (small[0][i] + small[1][i] + small[2][i]) / 3.0)
        .collect();
    let blur = mean(&lum, SIDE, SIDE, 2);
    let texture = gradient(&blur);
    let mut union = vec![false; SIDE * SIDE];
    for p in proposals {
        let area = p.area();
        if area < 0.02 {
            continue;
        }
        let top = p.mask[..6 * SIDE].iter().filter(|m| **m).count() as f32 / (6 * SIDE) as f32;
        if top < 0.5 {
            continue;
        }
        let (mut n, mut cy, mut tex, mut l, mut blue) = (0f32, 0f32, 0f32, 0f32, 0f32);
        for (i, _) in p.mask.iter().enumerate().filter(|(_, m)| **m) {
            n += 1.0;
            cy += (i / SIDE) as f32;
            tex += texture[i];
            l += lum[i];
            blue += small[2][i] - small[0][i];
        }
        let (cy, tex, l, blue) = (cy / n / (SIDE - 1) as f32, tex / n, l / n, blue / n);
        if cy < 0.55 && tex < 0.02 && (l > 0.5 || blue > 0.03) {
            for (u, m) in union.iter_mut().zip(&p.mask) {
                *u |= *m;
            }
        }
    }
    let union = to_horizon(&union);
    if union.iter().filter(|u| **u).count() < (0.01 * (SIDE * SIDE) as f32) as usize {
        return Ok(empty(width, height));
    }
    let guide = luma(image, width, height);
    let soft = expand(
        &union
            .iter()
            .map(|u| f32::from(u8::from(*u)))
            .collect::<Vec<_>>(),
        width,
        height,
    );
    let refined = guided(
        &guide,
        &soft,
        width,
        height,
        (width.max(height) / 200).max(3),
        1e-3,
    );
    Ok(Coverage {
        width,
        height,
        data: refined
            .iter()
            .map(|v| (smooth(*v) * 255.0 + 0.5) as u8)
            .collect(),
    })
}

fn check(len: usize) -> Result<(), InferenceError> {
    if len == SIDE * SIDE {
        Ok(())
    } else {
        Err(InferenceError::OutputInvalid(format!(
            "the saliency has {len} values, expected {}",
            SIDE * SIDE
        )))
    }
}

fn empty(width: usize, height: usize) -> Coverage {
    Coverage {
        width,
        height,
        data: vec![0; width * height],
    }
}

/// The photo's three colour planes on the [`SIDE`] grid, 0..=1.
fn grid(image: &RgbImage) -> [Vec<f32>; 3] {
    let whole = Rect {
        x: 0,
        y: 0,
        width: image.width,
        height: image.height,
    };
    let plane = |c: usize| {
        let full: Vec<f32> = image
            .data
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| f32::from(p[c]) / 255.0)
            .collect();
        resample(&full, image.width, whole, SIDE, SIDE)
    };
    [plane(0), plane(1), plane(2)]
}

/// The photo's luminance at `width` x `height` (the coverage's size, which is the
/// photo's own unless it was capped).
fn luma(image: &RgbImage, width: usize, height: usize) -> Vec<f32> {
    let full: Vec<f32> = image
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| {
            (0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])) / 255.0
        })
        .collect();
    if (width, height) == (image.width, image.height) {
        return full;
    }
    let whole = Rect {
        x: 0,
        y: 0,
        width: image.width,
        height: image.height,
    };
    resample(&full, image.width, whole, width, height)
}

/// A [`SIDE`] grid mask bilinearly enlarged to `width` x `height`.
fn expand(grid: &[f32], width: usize, height: usize) -> Vec<f32> {
    let whole = Rect {
        x: 0,
        y: 0,
        width: SIDE,
        height: SIDE,
    };
    resample(grid, SIDE, whole, width, height)
}

/// Gradient magnitude by central differences.
fn gradient(v: &[f32]) -> Vec<f32> {
    let at = |x: usize, y: usize| v[y.min(SIDE - 1) * SIDE + x.min(SIDE - 1)];
    (0..SIDE * SIDE)
        .map(|i| {
            let (x, y) = (i % SIDE, i / SIDE);
            let dx = (at(x + 1, y) - at(x.saturating_sub(1), y)) / 2.0;
            let dy = (at(x, y + 1) - at(x, y.saturating_sub(1))) / 2.0;
            dx.hypot(dy)
        })
        .collect()
}

/// `mask` grown by `radius` cells (a square window).
fn dilate(mask: &[bool], radius: usize) -> Vec<bool> {
    let pass = |src: &[bool], horizontal: bool| -> Vec<bool> {
        (0..SIDE * SIDE)
            .map(|i| {
                let (x, y) = (i % SIDE, i / SIDE);
                (0..=2 * radius).any(|d| {
                    let (xx, yy) = if horizontal {
                        ((x + d).checked_sub(radius), Some(y))
                    } else {
                        (Some(x), (y + d).checked_sub(radius))
                    };
                    match (xx, yy) {
                        (Some(xx), Some(yy)) if xx < SIDE && yy < SIDE => src[yy * SIDE + xx],
                        _ => false,
                    }
                })
            })
            .collect()
    };
    pass(&pass(mask, true), false)
}

/// Keeps each column's sky from the top down to where it stops, bridging gaps of a few
/// cells (a branch, a wire), so water or a window below the horizon is not sky.
fn to_horizon(mask: &[bool]) -> Vec<bool> {
    const GAP: usize = 6;
    let mut out = vec![false; SIDE * SIDE];
    for x in 0..SIDE {
        let mut last = None::<usize>;
        for y in 0..SIDE {
            if mask[y * SIDE + x] {
                last = Some(y);
                out[y * SIDE + x] = true;
            } else if last.is_none_or(|l| y - l > GAP) {
                // Before any sky only the first rows may be missing it.
                if last.is_some() || y > GAP {
                    break;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A photo of `w` x `h`: `paint(x, y)` colours each pixel.
    fn photo(w: usize, h: usize, paint: impl Fn(usize, usize) -> [u8; 3]) -> RgbImage {
        let mut data = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                data.extend(paint(x, y));
            }
        }
        RgbImage {
            width: w,
            height: h,
            data,
        }
    }
    fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> Vec<bool> {
        (0..SIDE * SIDE)
            .map(|i| (x0..x1).contains(&(i % SIDE)) && (y0..y1).contains(&(i / SIDE)))
            .collect()
    }
    fn at(c: &Coverage, fx: f32, fy: f32) -> u8 {
        c.data[(fy * c.height as f32) as usize * c.width + (fx * c.width as f32) as usize]
    }

    #[test]
    fn the_subject_is_the_outlines_inside_the_saliency_and_not_the_ones_outside() {
        // A bright block on a dark ground, and a second, equally sharp block that is not salient.
        let img = photo(256, 256, |x, y| {
            if (60..120).contains(&x) && (80..200).contains(&y)
                || (170..230).contains(&x) && (30..90).contains(&y)
            {
                [220, 200, 60]
            } else {
                [30, 40, 50]
            }
        });
        let saliency: Vec<f32> = rect(50, 70, 130, 210)
            .iter()
            .map(|m| f32::from(u8::from(*m)))
            .collect();
        let proposals = vec![
            Proposal {
                mask: rect(60, 80, 120, 200),
                score: 0.9,
            },
            Proposal {
                mask: rect(170, 30, 230, 90),
                score: 0.9,
            },
        ];
        let cov = subject(&img, &proposals, &saliency).unwrap();
        assert!(at(&cov, 0.35, 0.55) > 240, "{}", at(&cov, 0.35, 0.55));
        assert!(at(&cov, 0.78, 0.23) < 10, "{}", at(&cov, 0.78, 0.23));
        assert!(at(&cov, 0.05, 0.9) < 10);
    }

    #[test]
    fn without_anything_salient_nothing_is_selected() {
        let img = photo(64, 48, |_, _| [100, 100, 100]);
        let cov = subject(&img, &[], &vec![0.0; SIDE * SIDE]).unwrap();
        assert!(cov.data.iter().all(|v| *v == 0));
        assert!(subject(&img, &[], &[0.0; 3]).is_err());
    }

    #[test]
    fn sky_is_the_smooth_bright_outline_from_the_top_down_to_the_horizon() {
        // Blue sky over a textured dark ground, with water-like smooth blue below it.
        let img = photo(256, 256, |x, y| match y {
            0..=99 => [120, 160, 230],
            100..=149 => [(x * 7 % 60) as u8, (x * 13 % 70) as u8, 30],
            _ => [110, 150, 220],
        });
        let sky_and_water = Proposal {
            mask: (0..SIDE * SIDE)
                .map(|i| !(100..150).contains(&(i / SIDE)))
                .collect(),
            score: 0.95,
        };
        let ground = Proposal {
            mask: rect(0, 100, 256, 150),
            score: 0.9,
        };
        let cov = sky(&img, &[sky_and_water, ground]).unwrap();
        assert!(at(&cov, 0.5, 0.15) > 240);
        assert!(at(&cov, 0.5, 0.45) < 10, "the ground is not sky");
        assert!(
            at(&cov, 0.5, 0.85) < 10,
            "water below the horizon is not sky"
        );
    }

    #[test]
    fn a_dark_hill_or_a_photo_without_sky_has_no_sky() {
        let hill = photo(
            256,
            256,
            |_, y| if y < 128 { [15, 15, 18] } else { [40, 30, 20] },
        );
        let top = Proposal {
            mask: rect(0, 0, 256, 128),
            score: 0.9,
        };
        assert!(sky(&hill, &[top]).unwrap().data.iter().all(|v| *v == 0));
        assert!(sky(&hill, &[]).unwrap().data.iter().all(|v| *v == 0));
    }

    #[test]
    fn the_horizon_cut_bridges_a_branch_but_not_a_band_of_ground() {
        let mut mask = rect(0, 0, 256, 60);
        // A thin branch across the sky.
        for y in 20..23 {
            for x in 0..SIDE {
                mask[y * SIDE + x] = false;
            }
        }
        // Sky-like water well below the horizon.
        mask.iter_mut()
            .skip(120 * SIDE)
            .take(40 * SIDE)
            .for_each(|m| *m = true);
        let cut = to_horizon(&mask);
        assert!(cut[10 * SIDE + 5] && cut[40 * SIDE + 5] && cut[59 * SIDE + 5]);
        assert!(!cut[130 * SIDE + 5]);
    }
}
