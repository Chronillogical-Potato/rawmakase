//! Automatic Subject and Sky selection from what the models proposed.
//!
//! Segment Anything 2 answers a prompt with an object's outline but does not know which
//! object matters; the panoptic model knows where the people, animals and sky are but
//! draws blobs. Subject keeps the outlines SAM 2 drew that lie inside the people and
//! animals and fills any gap with the label itself; Sky keeps the outlines the sky label
//! lies on, down to where the sky ends. Both end with their edges moved onto the photo's.
//! Everything here is pure: the proposals and the labels come in, coverage in the photo's
//! frame goes out.
use crate::error::InferenceError;
use crate::panoptic::Instance;
use crate::process::{Coverage, Point, Prompt, Rect, RgbImage, output_size, resample};
use crate::refine::{guided, smooth};

/// Side of the square grid the proposals and the labels live on: the photo
/// stretched to a square, as the models see it.
pub const SIDE: usize = 256;

/// One outline SAM 2 drew: a mask on the [`SIDE`] grid and the model's own estimate of
/// its quality.
#[derive(Debug, Clone)]
pub struct Proposal {
    pub mask: Vec<bool>,
    pub score: f32,
}

/// Subject: `mask` (the union of what SAM 2 drew for each person and animal, 0..=1 on
/// the [`SIDE`] grid) with its edge moved onto the photo's. All zero when it is empty.
pub fn subject(image: &RgbImage, mask: &[f32]) -> Result<Coverage, InferenceError> {
    check(mask.len())?;
    let (width, height) = output_size(image);
    if mask.iter().sum::<f32>() < 0.002 * (SIDE * SIDE) as f32 {
        return Ok(empty(width, height));
    }
    let guide = luma(image, width, height);
    let soft = expand(mask, width, height);
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

/// The prompt that makes SAM 2 draw `instance`: its box, a few points well inside it
/// and a point inside each other instance, which is not part of it.
pub fn aim(instance: &Instance, others: &[&Instance]) -> Prompt {
    let inside: Vec<bool> = instance.mask.iter().map(|v| *v > 0.5).collect();
    let inner = depth(&inside);
    let cell = |i: usize| ((i % SIDE) as f32 + 0.5) / SIDE as f32;
    let row = |i: usize| ((i / SIDE) as f32 + 0.5) / SIDE as f32;
    let deepest = |d: &[f32]| {
        d.iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map(|(i, v)| (i, *v))
    };
    let mut points = Vec::new();
    let mut chosen: Vec<usize> = Vec::new();
    if let Some((first, peak)) = deepest(&inner) {
        chosen.push(first);
        // Two more, far from each other, still well inside.
        let well: Vec<usize> = (0..SIDE * SIDE)
            .filter(|i| inner[*i] > 0.5 * peak)
            .collect();
        for _ in 0..2 {
            let far = well.iter().copied().max_by_key(|c| {
                chosen
                    .iter()
                    .map(|p| {
                        let (dx, dy) = (
                            (c % SIDE) as i64 - (p % SIDE) as i64,
                            (c / SIDE) as i64 - (p / SIDE) as i64,
                        );
                        dx * dx + dy * dy
                    })
                    .min()
                    .unwrap_or(0)
            });
            if let Some(far) = far.filter(|f| !chosen.contains(f)) {
                chosen.push(far);
            }
        }
    }
    points.extend(chosen.iter().map(|i| Point {
        x: cell(*i),
        y: row(*i),
        positive: true,
    }));
    for other in others {
        let theirs: Vec<bool> = other.mask.iter().map(|v| *v > 0.5).collect();
        if let Some((i, _)) = deepest(&depth(&theirs)).filter(|(i, _)| !inside[*i]) {
            points.push(Point {
                x: cell(i),
                y: row(i),
                positive: false,
            });
        }
    }
    let (mut x0, mut y0, mut x1, mut y1) = (SIDE, SIDE, 0, 0);
    for (i, _) in inside.iter().enumerate().filter(|(_, m)| **m) {
        x0 = x0.min(i % SIDE);
        x1 = x1.max(i % SIDE);
        y0 = y0.min(i / SIDE);
        y1 = y1.max(i / SIDE);
    }
    let unit = |v: usize, up: bool| {
        let edge = if up { v + 1 } else { v };
        (edge as f32 / SIDE as f32).clamp(0.0, 1.0)
    };
    let bounds = (x1 >= x0 && y1 >= y0).then(|| {
        [
            unit(x0, false),
            unit(y0, false),
            unit(x1, true),
            unit(y1, true),
        ]
    });
    Prompt { points, bounds }
}

/// How far inside `mask` each cell is (city-block distance to the nearest outside cell,
/// 0 outside), by two sweeps.
fn depth(mask: &[bool]) -> Vec<f32> {
    let big = (2 * SIDE) as f32;
    let mut d: Vec<f32> = mask.iter().map(|m| if *m { big } else { 0.0 }).collect();
    for y in 0..SIDE {
        for x in 0..SIDE {
            let i = y * SIDE + x;
            if d[i] == 0.0 {
                continue;
            }
            let up = if y > 0 { d[i - SIDE] } else { 0.0 };
            let left = if x > 0 { d[i - 1] } else { 0.0 };
            d[i] = d[i].min(up + 1.0).min(left + 1.0);
        }
    }
    for y in (0..SIDE).rev() {
        for x in (0..SIDE).rev() {
            let i = y * SIDE + x;
            if d[i] == 0.0 {
                continue;
            }
            let down = if y + 1 < SIDE { d[i + SIDE] } else { 0.0 };
            let right = if x + 1 < SIDE { d[i + 1] } else { 0.0 };
            d[i] = d[i].min(down + 1.0).min(right + 1.0);
        }
    }
    d
}

/// How much two masks overlap, intersection over union.
pub fn iou(a: &[bool], b: &[bool]) -> f32 {
    let both = a.iter().zip(b).filter(|(x, y)| **x && **y).count();
    let either = a.iter().zip(b).filter(|(x, y)| **x || **y).count();
    if either == 0 {
        0.0
    } else {
        both as f32 / either as f32
    }
}

/// Points spread over the sky label, to ask SAM 2 for the outlines it lies on.
pub fn sky_points(sky_label: &[f32]) -> Vec<Point> {
    const SPREAD: usize = 6;
    let mut points = Vec::new();
    for j in 0..SPREAD {
        for i in 0..SPREAD {
            let (x, y) = (
                (i as f32 + 0.5) / SPREAD as f32,
                (j as f32 + 0.5) / SPREAD as f32,
            );
            let at = (y * SIDE as f32) as usize * SIDE + (x * SIDE as f32) as usize;
            if sky_label.get(at).is_some_and(|s| *s > 0.8) {
                points.push(Point {
                    x,
                    y,
                    positive: true,
                });
            }
        }
    }
    points
}

/// Sky: the outlines the panoptic model's sky label lies on, cut where the sky ends.
/// All zero when there is none.
pub fn sky(
    image: &RgbImage,
    proposals: &[Proposal],
    sky_label: &[f32],
) -> Result<Coverage, InferenceError> {
    check(sky_label.len())?;
    let (width, height) = output_size(image);
    let labelled: Vec<bool> = sky_label.iter().map(|s| *s > 0.5).collect();
    let cells = labelled.iter().filter(|l| **l).count();
    if cells < (0.01 * (SIDE * SIDE) as f32) as usize {
        return Ok(empty(width, height));
    }
    let mut union = vec![false; SIDE * SIDE];
    for p in proposals {
        let count = p.mask.iter().filter(|m| **m).count() as f32;
        if count < 0.005 * (SIDE * SIDE) as f32 {
            continue;
        }
        let sky: f32 = p
            .mask
            .iter()
            .zip(sky_label)
            .filter(|(m, _)| **m)
            .map(|(_, s)| *s)
            .sum();
        // Mostly sky: a lake that reflects it is not.
        if sky / count > 0.7 {
            for (u, m) in union.iter_mut().zip(&p.mask) {
                *u |= *m;
            }
        }
    }
    // Where no outline fits, the label itself stands in for it.
    let covered = union
        .iter()
        .zip(&labelled)
        .filter(|(u, l)| **u && **l)
        .count();
    if (covered as f32) < 0.6 * cells as f32 {
        for (u, l) in union.iter_mut().zip(&labelled) {
            *u |= *l;
        }
    }
    let union = to_horizon(&union);
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
            "a label map has {len} values, expected {}",
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
    fn the_subject_is_the_drawn_mask_with_its_edge_on_the_photos() {
        // A bright block (pixels 250..470 across); the mask drawn for it, on the coarse
        // grid, is a cell too wide on every side.
        let img = photo(1000, 1000, |x, y| {
            if (250..470).contains(&x) && (300..780).contains(&y) {
                [220, 200, 60]
            } else {
                [30, 40, 50]
            }
        });
        let loose: Vec<f32> = rect(63, 76, 121, 200)
            .iter()
            .map(|m| f32::from(u8::from(*m)))
            .collect();
        let cov = subject(&img, &loose).unwrap();
        let px = |x: usize, y: usize| cov.data[y * cov.width + x];
        assert!(px(350, 500) > 240 && px(800, 200) < 10);
        // Just outside the block, where the loose mask reached, the edge has moved in.
        assert!(px(245, 500) < 100, "{}", px(245, 500));
        assert!(px(475, 500) < 100, "{}", px(475, 500));
    }

    fn instance(mask: Vec<bool>) -> Instance {
        Instance {
            class: 1,
            score: 0.99,
            mask: mask.iter().map(|m| f32::from(u8::from(*m))).collect(),
        }
    }

    #[test]
    fn a_person_is_asked_for_with_a_box_points_inside_and_points_on_the_others() {
        let left = instance(rect(20, 40, 100, 200));
        let right = instance(rect(110, 40, 200, 200));
        let prompt = aim(&left, &[&right]);
        let [l, t, r, b] = prompt.bounds.unwrap();
        assert!((l - 20. / 256.).abs() < 0.01 && (r - 101. / 256.).abs() < 0.01);
        assert!((t - 40. / 256.).abs() < 0.01 && (b - 201. / 256.).abs() < 0.01);
        let (pos, neg): (Vec<&Point>, Vec<&Point>) = prompt.points.iter().partition(|p| p.positive);
        assert!((1..=3).contains(&pos.len()) && neg.len() == 1);
        assert!(pos.iter().all(|p| p.x > 20. / 256. && p.x < 100. / 256.));
        assert!(neg[0].x > 110. / 256.);
        prompt.validate().unwrap();
        // Alone, there is nothing to leave out.
        assert!(aim(&left, &[]).points.iter().all(|p| p.positive));
    }

    #[test]
    fn masks_are_compared_by_overlap_and_the_sky_is_sampled_where_it_is() {
        assert!((iou(&rect(0, 0, 100, 100), &rect(0, 0, 100, 50)) - 0.5).abs() < 1e-6);
        assert_eq!(iou(&rect(0, 0, 10, 10), &rect(100, 100, 110, 110)), 0.0);
        let label: Vec<f32> = rect(0, 0, 256, 100)
            .iter()
            .map(|m| f32::from(u8::from(*m)))
            .collect();
        let points = sky_points(&label);
        assert!(!points.is_empty() && points.iter().all(|p| p.y < 100. / 256.));
        assert!(sky_points(&vec![0.0; SIDE * SIDE]).is_empty());
    }

    #[test]
    fn without_anything_salient_nothing_is_selected() {
        let img = photo(64, 48, |_, _| [100, 100, 100]);
        let cov = subject(&img, &vec![0.0; SIDE * SIDE]).unwrap();
        assert!(cov.data.iter().all(|v| *v == 0));
        assert!(subject(&img, &[0.0; 3]).is_err());
    }

    #[test]
    fn sky_is_the_outline_the_label_lies_on_from_the_top_down_to_the_horizon() {
        // Sky over a textured ground, with smooth sky-coloured water below it.
        let img = photo(256, 256, |x, y| match y {
            0..=99 => [120, 160, 230],
            100..=149 => [(x * 7 % 60) as u8, (x * 13 % 70) as u8, 30],
            _ => [110, 150, 220],
        });
        // The model labels the sky, and also a patch of the water that reflects it.
        let label: Vec<f32> = (0..SIDE * SIDE)
            .map(|i| {
                f32::from(u8::from(
                    i / SIDE < 100 || (i / SIDE > 200 && i % SIDE < 40),
                ))
            })
            .collect();
        let sky_and_water = Proposal {
            mask: (0..SIDE * SIDE)
                .map(|i| !(100..150).contains(&(i / SIDE)))
                .collect(),
            score: 0.95,
        };
        let sky_only = Proposal {
            mask: rect(0, 0, 256, 100),
            score: 0.9,
        };
        let ground = Proposal {
            mask: rect(0, 100, 256, 150),
            score: 0.9,
        };
        let cov = sky(&img, &[sky_and_water, sky_only, ground], &label).unwrap();
        assert!(at(&cov, 0.5, 0.15) > 240);
        assert!(at(&cov, 0.5, 0.45) < 10, "the ground is not sky");
        assert!(
            at(&cov, 0.5, 0.85) < 10,
            "water below the horizon is not sky"
        );
    }

    #[test]
    fn without_a_sky_label_there_is_no_sky() {
        let hill = photo(
            256,
            256,
            |_, y| if y < 128 { [15, 15, 18] } else { [40, 30, 20] },
        );
        let top = Proposal {
            mask: rect(0, 0, 256, 128),
            score: 0.9,
        };
        assert!(
            sky(&hill, &[top], &vec![0.0; SIDE * SIDE])
                .unwrap()
                .data
                .iter()
                .all(|v| *v == 0)
        );
        assert!(sky(&hill, &[], &[0.0; 3]).is_err());
    }

    #[test]
    fn a_label_stands_in_where_no_outline_fits() {
        let img = photo(256, 256, |_, y| {
            if y < 100 {
                [120, 160, 230]
            } else {
                [20, 90, 30]
            }
        });
        let label: Vec<f32> = (0..SIDE * SIDE)
            .map(|i| f32::from(u8::from(i / SIDE < 100)))
            .collect();
        let cov = sky(&img, &[], &label).unwrap();
        assert!(at(&cov, 0.5, 0.2) > 240 && at(&cov, 0.5, 0.7) < 10);
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
