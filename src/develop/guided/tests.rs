use super::*;
use crate::develop::{Geometry, Upright};

/// A 3000 × 2000 photo at the default focal length, turned by the camera `flip`.
fn metadata(flip: i32) -> Metadata {
    Metadata {
        width: 3000,
        height: 2000,
        flip,
        wb: [1.; 3],
        ..Default::default()
    }
}

/// A camera turned by `tilt` (up, degrees), `pan` (right) and `roll` (clockwise) looks
/// at a scene; `project` gives where a scene point lands on the photo as recorded, in
/// 0–1 coordinates, for a landscape photo shown as recorded.
struct Camera {
    turn: Mat,
    focal: f32,
}
impl Camera {
    fn new(tilt: f32, pan: f32, roll: f32) -> Self {
        let r = |axis: [f32; 3], deg: f32| upright::rotation(axis, deg.to_radians());
        Self {
            turn: upright::mat(
                r([0., 0., 1.], roll),
                upright::mat(r([1., 0., 0.], tilt), r([0., 1., 0.], pan)),
            ),
            focal: upright::focal(&metadata(0)),
        }
    }
    fn project(&self, p: [f32; 3]) -> [f32; 2] {
        let c = apply(self.turn, p);
        let (x, y) = (self.focal * c[0] / c[2], self.focal * c[1] / c[2]);
        [x + 0.5, y / (2. / 3.) + 0.5]
    }
    /// A guide along the scene line from `a` to `b`.
    fn guide(&self, a: [f32; 3], b: [f32; 3]) -> UprightGuide {
        UprightGuide {
            a: self.project(a),
            b: self.project(b),
        }
    }
}

/// A scene's vertical edge at (`x`, `z`), from 1 below the camera to 1 above it.
fn vertical(x: f32, z: f32) -> ([f32; 3], [f32; 3]) {
    ([x, -1., z], [x, 1., z])
}
/// A horizontal edge running across at height `y` and depth `z`.
fn horizontal(y: f32, z: f32) -> ([f32; 3], [f32; 3]) {
    ([-1.2, y, z], [1.2, y, z])
}

/// The recipe with the guides solved, as the editor stores them.
fn solved(guides: &[UprightGuide], base: &Recipe, m: &Metadata) -> (Recipe, Solution) {
    let mut r = base.clone();
    r.upright = Upright {
        mode: UprightMode::Guided,
        corrections: vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 5],
        guides: guides.to_vec(),
        ..Default::default()
    };
    let solution = solve(guides, &GuideFrame::new(m, &r));
    assert_eq!(store(&mut r, m), solution.issue);
    (r, solution)
}

/// How far, in pixels at 2000 px, the scene line from `a` to `b` is from vertical
/// (`Kind::Vertical`) or level on the rendered photo.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Vertical,
    Level,
}
fn off(camera: &Camera, r: &Recipe, m: &Metadata, line: ([f32; 3], [f32; 3]), kind: Kind) -> f32 {
    let g = Geometry::for_metadata(m, r);
    let shown = |p: [f32; 3]| {
        let [u, v] = camera.project(p);
        let [x, y] = g.view(u * m.width as f32 - 0.5, v * m.height as f32 - 0.5);
        let scale = 2000. / g.width.max(g.height) as f32;
        [x * g.width as f32 * scale, y * g.height as f32 * scale]
    };
    let (a, b) = (shown(line.0), shown(line.1));
    // Scaled to a 1000 px run along the line, so the measure doesn't depend on its length.
    let (along, across) = match kind {
        Kind::Vertical => (b[1] - a[1], b[0] - a[0]),
        Kind::Level => (b[0] - a[0], b[1] - a[1]),
    };
    (across / along * 1000.).abs()
}

#[test]
fn two_verticals_make_every_vertical_plumb() {
    let m = metadata(0);
    let camera = Camera::new(12., 4., 2.);
    let guides = [vertical(-1., 5.), vertical(1.2, 6.)].map(|(a, b)| camera.guide(a, b));
    let (r, solution) = solved(&guides, &Recipe::default(), &m);
    assert_eq!(solution.issue, None);
    // Before: the verticals converge.
    let before = off(
        &camera,
        &Recipe::default(),
        &m,
        vertical(-1.5, 5.),
        Kind::Vertical,
    );
    assert!(before > 5., "{before}");
    for x in [-1.5f32, -0.3, 0.4, 1.6] {
        let px = off(&camera, &r, &m, vertical(x, 5. + x), Kind::Vertical);
        assert!(px < 1., "{x}: {px} px off plumb");
    }
}

#[test]
fn two_horizontals_level_every_horizontal() {
    let m = metadata(0);
    let camera = Camera::new(1., 20., -3.);
    let guides = [horizontal(-0.6, 4.), horizontal(0.7, 4.)].map(|(a, b)| camera.guide(a, b));
    let (r, solution) = solved(&guides, &Recipe::default(), &m);
    assert_eq!(solution.issue, None);
    for y in [-0.9f32, 0., 0.5] {
        let px = off(&camera, &r, &m, horizontal(y, 4.), Kind::Level);
        assert!(px < 1., "{y}: {px} px off level");
    }
}

#[test]
fn three_and_four_guides_fix_both_directions() {
    let m = metadata(0);
    let camera = Camera::new(10., -18., 3.);
    let v = [vertical(-1., 5.), vertical(1., 5.)].map(|(a, b)| camera.guide(a, b));
    let h = [horizontal(-0.8, 5.), horizontal(0.9, 5.)].map(|(a, b)| camera.guide(a, b));
    for guides in [vec![v[0], v[1], h[0]], vec![v[0], h[0], v[1], h[1]]] {
        let (r, solution) = solved(&guides, &Recipe::default(), &m);
        assert_eq!(solution.issue, None, "{}", guides.len());
        for x in [-1.4f32, 0.2, 1.3] {
            let px = off(&camera, &r, &m, vertical(x, 5.), Kind::Vertical);
            assert!(px < 1., "{} guides, vertical {x}: {px} px", guides.len());
        }
        for y in [-0.5f32, 0.3] {
            let px = off(&camera, &r, &m, horizontal(y, 5.), Kind::Level);
            assert!(px < 1., "{} guides, horizontal {y}: {px} px", guides.len());
        }
    }
}

#[test]
fn one_vertical_and_one_horizontal_make_both_right() {
    let m = metadata(0);
    let camera = Camera::new(6., -9., 2.);
    let guides = [
        camera.guide(vertical(-0.8, 5.).0, vertical(-0.8, 5.).1),
        camera.guide(horizontal(0.6, 5.).0, horizontal(0.6, 5.).1),
    ];
    let (r, solution) = solved(&guides, &Recipe::default(), &m);
    assert_eq!(solution.issue, None);
    assert!(off(&camera, &r, &m, vertical(-0.8, 5.), Kind::Vertical) < 1.);
    assert!(off(&camera, &r, &m, horizontal(0.6, 5.), Kind::Level) < 1.);
}

const IDENTITY_9: [f32; 9] = [1., 0., 0., 0., 1., 0., 0., 0., 1.];

#[test]
fn too_few_short_and_collinear_guides_are_said_not_crashed_on() {
    let m = metadata(0);
    let frame = GuideFrame::new(&m, &Recipe::default());
    let a = UprightGuide {
        a: [0.3, 0.1],
        b: [0.32, 0.9],
    };
    let b = UprightGuide {
        a: [0.7, 0.1],
        b: [0.68, 0.9],
    };
    // None, or one: nothing to go by.
    for guides in [vec![], vec![a]] {
        let s = solve(&guides, &frame);
        assert_eq!((s.correction, s.issue), (IDENTITY_9, Some(Issue::TooFew)));
    }
    // A guide too short to measure (or not a number) is left out, and said.
    let short = UprightGuide {
        a: [0.5, 0.5],
        b: [0.505, 0.505],
    };
    let broken = UprightGuide {
        a: [f32::NAN, 0.5],
        b: [0.5, 0.9],
    };
    let s = solve(&[a, short, b, broken], &frame);
    assert_eq!(s.issue, Some(Issue::TooShort));
    assert_eq!(s.correction, solve(&[a, b], &frame).correction);
    assert_ne!(s.correction, IDENTITY_9);
    // Two guides along one line fix no direction.
    let along = UprightGuide {
        a: [0.31, 0.5],
        b: [0.315, 0.7],
    };
    let s = solve(&[a, along], &frame);
    assert_eq!((s.correction, s.issue), (IDENTITY_9, Some(Issue::SameLine)));
    // Lines crossing in the photo would need the camera turned on its back.
    let crossing = UprightGuide {
        a: [0.2, 0.1],
        b: [0.8, 0.9],
    };
    let other = UprightGuide {
        a: [0.8, 0.1],
        b: [0.2, 0.9],
    };
    let s = solve(&[crossing, other], &frame);
    assert_eq!((s.correction, s.issue), (IDENTITY_9, Some(Issue::TooSteep)));
}

#[test]
fn guides_that_disagree_are_corrected_as_nearly_as_they_allow() {
    let m = metadata(0);
    let camera = Camera::new(10., -18., 3.);
    let mut guides: Vec<UprightGuide> = [vertical(-1., 5.), vertical(1., 5.)]
        .into_iter()
        .chain([horizontal(-0.8, 5.), horizontal(0.9, 5.)])
        .map(|(a, b)| camera.guide(a, b))
        .collect();
    // One horizontal drawn several degrees off its edge.
    guides[3].b[1] += 0.06;
    let (r, solution) = solved(&guides, &Recipe::default(), &m);
    assert_eq!(solution.issue, Some(Issue::Disagree));
    // The verticals still come first.
    let px = off(&camera, &r, &m, vertical(0.3, 5.), Kind::Vertical);
    assert!(px < 1., "{px}");
}

/// Guides stay on the photo as recorded: a crop leaves the correction alone, and a turn
/// of the photo solves to a correction that still makes the same edges straight.
#[test]
fn guides_hold_after_crop_and_rotation() {
    let m = metadata(0);
    let camera = Camera::new(12., 4., 2.);
    let guides = [vertical(-1., 5.), vertical(1.2, 6.)].map(|(a, b)| camera.guide(a, b));
    let (plain, _) = solved(&guides, &Recipe::default(), &m);
    let cropped = Recipe {
        crop: [0.1, 0.2, 0.7, 0.9],
        ..Default::default()
    };
    let (r, _) = solved(&guides, &cropped, &m);
    assert_eq!(r.upright.corrections, plain.upright.corrections);
    let px = off(&camera, &r, &m, vertical(0.4, 5.), Kind::Vertical);
    assert!(px < 1., "cropped: {px}");
    // Turned a quarter: the edges show across, and come out level.
    for rotation in [1, 3] {
        let turned = Recipe {
            rotation,
            ..Default::default()
        };
        let (r, solution) = solved(&guides, &turned, &m);
        assert_eq!(solution.issue, None);
        let px = off(&camera, &r, &m, vertical(0.4, 5.), Kind::Level);
        assert!(px < 1., "turned {rotation}: {px}");
        // The same correction of the photo as recorded.
        for (a, b) in r.upright.corrections[5]
            .iter()
            .zip(&plain.upright.corrections[5])
        {
            assert!((a - b).abs() < 1e-3, "{:?}", r.upright.corrections[5]);
        }
    }
    // A camera that recorded the photo in portrait shows it turned, as `rotation` does.
    let portrait = metadata(6);
    let (r, solution) = solved(&guides, &Recipe::default(), &portrait);
    assert_eq!(solution.issue, None);
    let px = off(&camera, &r, &portrait, vertical(0.4, 5.), Kind::Level);
    assert!(px < 1., "portrait: {px}");
}

/// Lens corrections apply before Upright: guides keep their place in the corrected
/// photo, and with manual Distortion they solve to the same correction.
#[test]
fn guides_solve_the_same_through_lens_corrections() {
    let m = metadata(0);
    let camera = Camera::new(12., 4., 2.);
    let guides = [vertical(-1., 5.), vertical(1.2, 6.)].map(|(a, b)| camera.guide(a, b));
    let (plain, _) = solved(&guides, &Recipe::default(), &m);
    let lens = Recipe {
        lens_manual_distortion: 0.3,
        ..Default::default()
    };
    let (r, _) = solved(&guides, &lens, &m);
    assert_eq!(r.upright.corrections[5], plain.upright.corrections[5]);
}

/// Without the other modes' analysis there is nowhere to put the Guided correction yet.
#[test]
fn storing_waits_for_the_analysis() {
    let m = metadata(0);
    let mut r = Recipe::default();
    r.upright.mode = UprightMode::Guided;
    r.upright.guides = vec![
        UprightGuide {
            a: [0.3, 0.1],
            b: [0.32, 0.9],
        };
        2
    ];
    r.upright.corrections = vec![IDENTITY_9; 2];
    assert_eq!(store(&mut r, &m), None);
    assert_eq!(r.upright.corrections.len(), 2);
}
