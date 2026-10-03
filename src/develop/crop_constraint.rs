//! Lightroom's Constrain Crop: the crop shrinks, keeping its aspect, until it holds
//! only positions with a source pixel, so Transform, Upright and manual Distortion leave
//! no white in the result (docs/transform.md#constrain-crop).

/// Positions on each edge of a candidate crop that must have a source pixel, besides
/// the corners, unless [`Covers::straight_edges`]. Manual Distortion bends the edges;
/// between these samples one strays by far less than the margin the geometry keeps.
const EDGE_SAMPLES: usize = 32;
/// Centres tried on each axis at each size.
const GRID: usize = 9;
const BISECTIONS: usize = 24;
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
/// `crop` itself when it is covered, otherwise its aspect shrunk to the largest size
/// that fits, centred as near to its centre as fits. `None` when not even the smallest
/// valid crop (1% of each side) fits anywhere inside it.
pub(crate) fn largest_covered(crop: [f32; 4], area: &impl Covers) -> Option<[f32; 4]> {
    let search = Search { crop, area };
    if search.fits(1., search.centre()) {
        return Some(crop);
    }
    let size = [crop[2] - crop[0], crop[3] - crop[1]];
    let smallest = (0.01 / size[0]).max(0.01 / size[1]).min(1.);
    let start = Fit {
        scale: smallest,
        centre: search.centre(),
        span: [0.; 2],
    };
    // Shrunk about its centre, then moved over everything the crop allows and on a finer
    // grid around the best place.
    let centred = search.grow(start);
    let coarse = search.grow(Fit {
        span: [f32::INFINITY; 2],
        ..start
    })?;
    let fine = search
        .grow(Fit {
            span: coarse.span.map(|s| s / (GRID - 1) as f32),
            ..coarse
        })
        .unwrap_or(coarse);
    // Moved only for a real gain, not one from where the edges happen to be checked.
    let best = match centred {
        Some(c) if c.scale >= fine.scale * (1. - SAME_SCALE) => c,
        _ => fine,
    };
    // A hair smaller, so the crop found counts as covered when checked again.
    Some(search.rect(best.scale * (1. - SAME_SCALE / 10.), best.centre))
}

/// A crop of the original's aspect at `scale` of its size, centred at `centre`, and the
/// half-extent of the grid of centres it was found on.
#[derive(Clone, Copy, Debug)]
struct Fit {
    scale: f32,
    centre: [f32; 2],
    span: [f32; 2],
}

struct Search<'a, A> {
    crop: [f32; 4],
    area: &'a A,
}
impl<A: Covers> Search<'_, A> {
    fn centre(&self) -> [f32; 2] {
        let c = self.crop;
        [(c[0] + c[2]) / 2., (c[1] + c[3]) / 2.]
    }
    fn rect(&self, scale: f32, [x, y]: [f32; 2]) -> [f32; 4] {
        let c = self.crop;
        let (w, h) = ((c[2] - c[0]) * scale / 2., (c[3] - c[1]) * scale / 2.);
        [x - w, y - h, x + w, y + h]
    }
    /// The largest scale from `start` (whose scale fits) to 1 with a fitting centre on
    /// the grid around `start.centre`, by bisection.
    fn grow(&self, start: Fit) -> Option<Fit> {
        let mut best = self.place(start.scale, start.centre, start.span)?;
        let mut high = 1.;
        for _ in 0..BISECTIONS {
            let scale = 0.5 * (best.scale + high);
            match self.place(scale, start.centre, start.span) {
                Some(fit) => best = fit,
                None => high = scale,
            }
        }
        Some(best)
    }
    /// A centre where a crop at `scale` fits: the original centre if it does, otherwise
    /// the one nearest to it on a grid of `GRID`² centres within `span` of `around`,
    /// kept inside the original crop.
    fn place(&self, scale: f32, around: [f32; 2], span: [f32; 2]) -> Option<Fit> {
        let c = self.crop;
        let half = [(c[2] - c[0]) * scale / 2., (c[3] - c[1]) * scale / 2.];
        let allowed = [
            [c[0] + half[0], c[2] - half[0]],
            [c[1] + half[1], c[3] - half[1]],
        ];
        let range: [[f32; 2]; 2] = std::array::from_fn(|axis| {
            let [lo, hi] = allowed[axis];
            [
                (around[axis] - span[axis]).max(lo),
                (around[axis] + span[axis]).min(hi),
            ]
        });
        let preferred = self.centre();
        let step = |[lo, hi]: [f32; 2], i: usize| lo + (hi - lo) * i as f32 / (GRID - 1) as f32;
        let mut centres: Vec<[f32; 2]> = (0..GRID * GRID)
            .map(|i| [step(range[0], i % GRID), step(range[1], i / GRID)])
            .collect();
        let distance = |p: &[f32; 2]| (p[0] - preferred[0]).hypot(p[1] - preferred[1]);
        centres.sort_by(|a, b| distance(a).total_cmp(&distance(b)));
        let inside = |v: f32, [lo, hi]: [f32; 2]| (lo..=hi).contains(&v);
        let original =
            (inside(preferred[0], range[0]) && inside(preferred[1], range[1])).then_some(preferred);
        let step = [
            (range[0][1] - range[0][0]) / (GRID - 1) as f32,
            (range[1][1] - range[1][0]) / (GRID - 1) as f32,
        ];
        original
            .into_iter()
            .chain(centres)
            .find(|&centre| self.fits(scale, centre))
            .map(|centre| Fit {
                scale,
                centre,
                span: step,
            })
    }
    /// Whether the area covers the crop at `scale` around `centre`: corners first, as
    /// they usually leave the covered area first, then points along the edges.
    fn fits(&self, scale: f32, centre: [f32; 2]) -> bool {
        let [l, t, r, b] = self.rect(scale, centre);
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
