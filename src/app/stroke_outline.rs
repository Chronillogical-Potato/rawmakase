//! The outline of a brush stroke, as Lightroom draws a Remove brush: the edge of
//! everything within the brush radius of the stroke's path, with round ends and
//! corners, traced on a grid with marching squares.
use eframe::egui::{Pos2, Vec2};

/// Most grid cells across the stroke's bounds, so a long stroke stays cheap to trace.
const MAX_CELLS: f32 = 240.;

/// The outline of the area within `radius` of the path through `points` (screen
/// space), as line segments about `step` points long or longer for large strokes.
pub(super) fn outline(points: &[Pos2], radius: f32, step: f32) -> Vec<[Pos2; 2]> {
    if points.is_empty() || radius <= 0. {
        return Vec::new();
    }
    let (mut min, mut max) = (points[0], points[0]);
    for p in points {
        min = min.min(*p);
        max = max.max(*p);
    }
    let pad = Vec2::splat(radius + 2. * step);
    let (min, max) = (min - pad, max + pad);
    let size = max - min;
    let step = step.max(size.x.max(size.y) / MAX_CELLS);
    let (nx, ny) = (
        (size.x / step).ceil() as usize + 1,
        (size.y / step).ceil() as usize + 1,
    );
    // Distance from each grid point to the path, less the radius: negative inside.
    let mut field = vec![f32::INFINITY; nx * ny];
    let at = |i: usize, j: usize| min + Vec2::new(i as f32 * step, j as f32 * step);
    let segments: Vec<(Pos2, Pos2)> = if points.len() == 1 {
        vec![(points[0], points[0])]
    } else {
        points.windows(2).map(|w| (w[0], w[1])).collect()
    };
    for (a, b) in segments {
        let lo = a.min(b) - Vec2::splat(radius + 2. * step) - min;
        let hi = a.max(b) + Vec2::splat(radius + 2. * step) - min;
        let i0 = (lo.x / step).floor().max(0.) as usize;
        let j0 = (lo.y / step).floor().max(0.) as usize;
        let i1 = ((hi.x / step).ceil() as usize).min(nx - 1);
        let j1 = ((hi.y / step).ceil() as usize).min(ny - 1);
        for j in j0..=j1 {
            for i in i0..=i1 {
                let d = distance_to_segment(at(i, j), a, b) - radius;
                let cell = &mut field[j * nx + i];
                *cell = cell.min(d);
            }
        }
    }
    // Marching squares: where the field crosses zero along each cell edge.
    let mut out = Vec::new();
    let cross = |p: Pos2, q: Pos2, dp: f32, dq: f32| p + (q - p) * (dp / (dp - dq));
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            let corners = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
            let d = corners.map(|(i, j)| field[j * nx + i]);
            let p = corners.map(|(i, j)| at(i, j));
            let mut crossings = Vec::with_capacity(4);
            for k in 0..4 {
                let n = (k + 1) % 4;
                if (d[k] < 0.) != (d[n] < 0.) && d[k].is_finite() && d[n].is_finite() {
                    crossings.push(cross(p[k], p[n], d[k], d[n]));
                }
            }
            match crossings.len() {
                2 => out.push([crossings[0], crossings[1]]),
                // A saddle: pair the crossings so the inside stays joined.
                4 => {
                    out.push([crossings[0], crossings[1]]);
                    out.push([crossings[2], crossings[3]]);
                }
                _ => {}
            }
        }
    }
    out
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let len = ab.length_sq();
    let t = if len > 0. {
        ((p - a).dot(ab) / len).clamp(0., 1.)
    } else {
        0.
    };
    p.distance(a + ab * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn distance_to_path(p: Pos2, path: &[Pos2]) -> f32 {
        if path.len() == 1 {
            return p.distance(path[0]);
        }
        path.windows(2)
            .map(|w| distance_to_segment(p, w[0], w[1]))
            .fold(f32::INFINITY, f32::min)
    }

    #[test]
    fn the_outline_follows_the_brush_edge_with_round_corners_and_ends() {
        let r = 20.;
        for path in [
            vec![Pos2::new(100., 100.)],
            vec![Pos2::new(100., 100.), Pos2::new(300., 100.)],
            // A sharp turn, where a wide line draws a miter spike.
            vec![
                Pos2::new(100., 100.),
                Pos2::new(200., 300.),
                Pos2::new(300., 110.),
            ],
            // Doubling back over itself.
            vec![
                Pos2::new(100., 100.),
                Pos2::new(300., 100.),
                Pos2::new(110., 105.),
            ],
        ] {
            let edges = outline(&path, r, 2.);
            assert!(!edges.is_empty());
            for p in edges.iter().flatten() {
                let d = distance_to_path(*p, &path);
                assert!((d - r).abs() < 1., "{p:?} is {d} from the path, not {r}");
            }
        }
    }

    #[test]
    fn a_stroke_inside_another_part_leaves_no_inner_edge() {
        // Back and forth along one line: one outline around both passes.
        let path = [
            Pos2::new(100., 100.),
            Pos2::new(200., 100.),
            Pos2::new(105., 100.),
        ];
        let edges = outline(&path, 20., 2.);
        assert!(
            edges
                .iter()
                .flatten()
                .all(|p| distance_to_path(*p, &path) > 19.)
        );
    }
}
