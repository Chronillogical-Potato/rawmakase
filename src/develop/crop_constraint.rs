//! Lightroom's Constrain Crop: the crop shrinks, keeping its aspect, until it holds
//! only positions with a source pixel, so Transform, Upright and manual Distortion leave
//! no white in the result (docs/transform.md#constrain-crop).

/// Positions on each edge of a candidate crop that must have a source pixel, besides
/// the corners, unless [`Covers::straight_edges`]. Manual Distortion bends the edges;
/// between these samples one strays by far less than the margin the geometry keeps.
const EDGE_SAMPLES: usize = 32;
/// Centres tried on each axis before the search closes in on the best one.
const GRID: usize = 9;
/// The finest grid, whose step (under 0.4% of the crop) is below the 1% smallest crop.
const MAX_GRID: usize = 257;
const BISECTIONS: usize = 24;
/// Smallest step, in crop-space units, of the search for the best centre.
const MIN_STEP: f32 = 1e-4;
/// Scales closer than this, relatively, count as the same.
const SAME_SCALE: f32 = 1e-3;

/// Whether a crop-space position (0–1 over the straightened, uncropped photo) has a
/// source pixel.
pub(crate) trait Covers {
    fn covers(&self, x: f32, y: f32) -> bool;
    /// Whether the covered area's edges are straight lines in crop space (the area is
    /// convex), so a crop whose corners are covered is covered entirely.
    fn straight_edges(&self) -> bool {
        false
    }
}

/// The largest crop with the aspect of `crop`, inside it, that `area` covers entirely:
/// `crop` itself when it is covered, otherwise shrunk about its centre, or moved where
/// that keeps noticeably more of it. When no crop of at least 1% of each side fits
/// inside it (it lies wholly in the white), the largest one at its aspect anywhere in
/// the photo. `None` when there is none either.
pub(crate) fn largest_covered(crop: [f32; 4], area: &impl Covers) -> Option<[f32; 4]> {
    let size = [crop[2] - crop[0], crop[3] - crop[1]];
    let inside = Search {
        bounds: crop,
        size,
        preferred: [(crop[0] + crop[2]) / 2., (crop[1] + crop[3]) / 2.],
        area,
    };
    if inside.fits(1., inside.preferred) {
        return Some(crop);
    }
    let fill = (1. / size[0]).min(1. / size[1]);
    let anywhere = Search {
        bounds: [0., 0., 1., 1.],
        size: size.map(|s| s * fill),
        preferred: [0.5; 2],
        area,
    };
    inside.largest().or_else(|| anywhere.largest())
}

/// A crop at `scale` of the search's size, centred at `centre`.
#[derive(Clone, Copy, Debug)]
struct Fit {
    scale: f32,
    centre: [f32; 2],
}

/// Crops of `size` (at scale 1) or smaller, at its aspect, inside `bounds`.
struct Search<'a, A> {
    bounds: [f32; 4],
    size: [f32; 2],
    preferred: [f32; 2],
    area: &'a A,
}
impl<A: Covers> Search<'_, A> {
    fn largest(&self) -> Option<[f32; 4]> {
        let smallest = (0.01 / self.size[0]).max(0.01 / self.size[1]);
        if smallest > 1. {
            return None;
        }
        let centred = self.grow(self.preferred, smallest);
        // Over a grid of centres, then closing in on the best one. With straight edges
        // the largest scale at each centre is concave, so this finds its maximum.
        // A covered area too small to hold a crop at any grid centre gets a finer grid.
        let mut best = centred;
        let b = self.bounds;
        let mut grid = GRID;
        let mut step;
        loop {
            step = [
                (b[2] - b[0]) / (grid - 1) as f32,
                (b[3] - b[1]) / (grid - 1) as f32,
            ];
            for i in 0..grid * grid {
                let centre = [
                    b[0] + step[0] * (i % grid) as f32,
                    b[1] + step[1] * (i / grid) as f32,
                ];
                best = self.better(best, centre, smallest);
            }
            if best.is_some() || grid >= MAX_GRID {
                break;
            }
            grid = 2 * grid - 1;
        }
        let mut step = step.map(|s| s / 2.);
        while step[0].max(step[1]) > MIN_STEP {
            let Some(at) = best else { break };
            let mut moved = false;
            for (dx, dy) in [
                (1., 0.),
                (-1., 0.),
                (0., 1.),
                (0., -1.),
                (1., 1.),
                (-1., -1.),
                (1., -1.),
                (-1., 1.),
            ] {
                let centre = [at.centre[0] + dx * step[0], at.centre[1] + dy * step[1]];
                let next = self.better(best, centre, smallest);
                if next.map(|n| n.scale) > best.map(|b| b.scale) {
                    best = next;
                    moved = true;
                }
            }
            if !moved {
                step = step.map(|s| s / 2.);
            }
        }
        let best = best?;
        // Moved only for a real gain, not one from where the edges happen to be checked.
        let best = match centred {
            Some(c) if c.scale >= best.scale * (1. - SAME_SCALE) => c,
            _ => best,
        };
        // A hair smaller, so the crop found counts as covered when checked again.
        Some(self.rect(best.scale * (1. - SAME_SCALE / 10.), best.centre))
    }
    /// `best`, or the crop at `centre` when one larger than `best` fits there.
    fn better(&self, best: Option<Fit>, centre: [f32; 2], smallest: f32) -> Option<Fit> {
        let floor = best.map_or(smallest, |b| b.scale * (1. + 1e-6));
        if floor > 1. || !self.fits(floor, centre) {
            return best;
        }
        self.grow(centre, floor).or(best)
    }
    /// The largest scale from `low` (which fits) to 1 that fits at `centre`, by
    /// bisection; `None` when `low` does not fit.
    fn grow(&self, centre: [f32; 2], low: f32) -> Option<Fit> {
        if !self.fits(low, centre) {
            return None;
        }
        let (mut low, mut high) = (low, 1.);
        if self.fits(1., centre) {
            low = 1.;
        }
        for _ in 0..BISECTIONS {
            let scale = 0.5 * (low + high);
            if self.fits(scale, centre) {
                low = scale;
            } else {
                high = scale;
            }
        }
        Some(Fit { scale: low, centre })
    }
    fn rect(&self, scale: f32, [x, y]: [f32; 2]) -> [f32; 4] {
        let (w, h) = (self.size[0] * scale / 2., self.size[1] * scale / 2.);
        [x - w, y - h, x + w, y + h]
    }
    /// Whether the crop at `scale` around `centre` lies in the bounds and the area
    /// covers it: corners first, as they usually leave the covered area first, then
    /// points along the edges.
    fn fits(&self, scale: f32, centre: [f32; 2]) -> bool {
        let [l, t, r, b] = self.rect(scale, centre);
        let bounds = self.bounds;
        let slack = 1e-6;
        if l < bounds[0] - slack
            || t < bounds[1] - slack
            || r > bounds[2] + slack
            || b > bounds[3] + slack
        {
            return false;
        }
        let corners = [[l, t], [r, t], [r, b], [l, b]];
        if !corners.iter().all(|&[x, y]| self.area.covers(x, y)) {
            return false;
        }
        self.area.straight_edges()
            || (1..EDGE_SAMPLES).all(|i| {
                let f = i as f32 / EDGE_SAMPLES as f32;
                let (x, y) = (l + (r - l) * f, t + (b - t) * f);
                self.area.covers(x, t)
                    && self.area.covers(x, b)
                    && self.area.covers(l, y)
                    && self.area.covers(r, y)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A disc of radius 0.3 around (0.5, 0.5).
    struct Disc;
    impl Covers for Disc {
        fn covers(&self, x: f32, y: f32) -> bool {
            (x - 0.5).hypot(y - 0.5) <= 0.3
        }
    }
    /// The half plane x ≥ 0.4.
    struct Right;
    impl Covers for Right {
        fn covers(&self, x: f32, _: f32) -> bool {
            x >= 0.4
        }
    }
    struct Nothing;
    impl Covers for Nothing {
        fn covers(&self, _: f32, _: f32) -> bool {
            false
        }
    }
    #[test]
    fn a_covered_crop_is_kept_as_it_is() {
        let crop = [0.3, 0.35, 0.7, 0.6];
        assert_eq!(largest_covered(crop, &Disc), Some(crop));
    }
    #[test]
    fn a_centred_crop_shrinks_about_its_centre_keeping_its_aspect() {
        let c = largest_covered([0., 0., 1., 1.], &Disc).unwrap();
        // The square inscribed in the disc: half side 0.3 / √2.
        let half = 0.3 / 2f32.sqrt();
        for (v, e) in c
            .iter()
            .zip([0.5 - half, 0.5 - half, 0.5 + half, 0.5 + half])
        {
            assert!((v - e).abs() < 2e-3, "{c:?}");
        }
    }
    #[test]
    fn a_crop_moves_into_the_covered_part_rather_than_shrinking_more() {
        // Without moving, a centred crop would shrink to 0.2 wide; moving it right
        // keeps 0.6 of its width.
        let crop = [0., 0.2, 1., 0.8];
        let c = largest_covered(crop, &Right).unwrap();
        assert!(c[0] >= 0.4 - 1e-6 && c[2] <= 1. + 1e-6, "{c:?}");
        assert!((c[2] - c[0] - 0.6).abs() < 0.01, "{c:?}");
        let aspect = (c[2] - c[0]) / (c[3] - c[1]);
        assert!((aspect - 1. / 0.6).abs() < 1e-3, "{c:?}");
        // It stays inside the original crop.
        assert!(c[1] >= 0.2 - 1e-6 && c[3] <= 0.8 + 1e-6, "{c:?}");
    }
    #[test]
    fn nothing_covered_leaves_no_crop() {
        assert_eq!(largest_covered([0., 0., 1., 1.], &Nothing), None);
    }
}
