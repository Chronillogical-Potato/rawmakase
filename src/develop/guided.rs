//! Lightroom's Guided Upright: two to four guides drawn along edges that should be
//! vertical or horizontal solve to a turn of the camera, framed as Upright frames its
//! other modes (docs/transform.md#guided-upright).
use super::upright::{self, Displayed, IDENTITY, Mat, Segment, apply, cross, dot, unit};
use super::{ImageFrame, Recipe, UprightGuide, UprightMode};
use crate::camera_data::Metadata;

/// Lightroom's limit on guides.
pub const MAX_GUIDES: usize = 4;
/// Shortest guide that counts, in units of the photo's long edge.
pub const MIN_LENGTH: f32 = 0.02;
/// Largest turn of the camera a correction may make, in degrees.
const MAX_TURN: f32 = 60.;
/// Guides of one kind whose planes through the camera meet at less than this (radians)
/// lie along one line and fix no direction between them.
const SAME_LINE: f32 = 0.005;
/// A guide left further than this (degrees) from vertical or horizontal means the guides
/// ask for more than one turn can give.
const DISAGREE: f32 = 0.5;

/// Why guides correct less than they might, for the status line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Issue {
    /// Fewer than two guides to go by: nothing is corrected.
    TooFew,
    /// A guide too short to measure was left out.
    TooShort,
    /// Two guides of one kind lie along one line, so they count as one.
    SameLine,
    /// The guides call for turning the camera too far: nothing is corrected.
    TooSteep,
    /// The guides ask for more than one turn can give: corrected as nearly as they allow.
    Disagree,
}
impl Issue {
    pub fn message(self) -> &'static str {
        match self {
            Issue::TooFew => {
                "Guided Upright: draw two or more guides along verticals or horizontals"
            }
            Issue::TooShort => "Guided Upright: a guide is too short to use; drag it longer",
            Issue::SameLine => {
                "Guided Upright: two guides lie along one line; draw them along different edges"
            }
            Issue::TooSteep => {
                "Guided Upright: the guides need too strong a correction; nothing applied"
            }
            Issue::Disagree => {
                "Guided Upright: the guides disagree; corrected as nearly as they allow"
            }
        }
    }
}

/// What a set of guides solves to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Solution {
    /// The correction as Lightroom stores `UprightTransform_5`: a forward homography in
    /// 0–1 coordinates of the photo as recorded; the identity when the guides fix nothing.
    pub correction: [f32; 9],
    pub issue: Option<Issue>,
}

/// The photo guides are solved on: its frame as displayed and its focal length.
pub struct GuideFrame {
    shown: Displayed,
    focal: f32,
}
impl GuideFrame {
    pub fn new(m: &Metadata, r: &Recipe) -> Self {
        let frame = ImageFrame::for_metadata(m);
        let [w, h] = frame.size();
        let (w, h) = if r.rotation % 2 == 1 { (h, w) } else { (w, h) };
        let turns = (frame.turns + r.rotation) % 4;
        Self {
            shown: Displayed::new(w, h, turns, r.flip_x, r.flip_y),
            focal: upright::focal(m),
        }
    }
}

/// What the guides of one kind fix of the direction their edges run along.
enum Fixed {
    Nothing,
    /// One edge: the direction lies in this plane through the camera (its normal).
    Plane([f32; 3]),
    /// Two or more edges meeting at a vanishing point: the direction itself.
    Direction([f32; 3]),
}

/// Solves `guides` on `frame`: guides nearer upright than level are verticals, the rest
/// horizontals. Two verticals (or horizontals) fix that direction and the camera turns
/// the least that makes it vertical (or horizontal); with guides of both kinds both
/// directions are fixed, the verticals exactly and the horizontals as nearly as they
/// allow. One of each turns the least that makes both right.
pub fn solve(guides: &[UprightGuide], frame: &GuideFrame) -> Solution {
    let f = frame.focal;
    let mut issue = None;
    let mut verticals = Vec::new();
    let mut horizontals = Vec::new();
    for g in guides.iter().take(MAX_GUIDES) {
        let s = Segment {
            a: frame.shown.centred(g.a),
            b: frame.shown.centred(g.b),
        };
        if s.length().is_nan() || s.length() < MIN_LENGTH {
            issue = Some(Issue::TooShort);
            continue;
        }
        let (dx, dy) = (s.b[0] - s.a[0], s.b[1] - s.a[1]);
        if dy.abs() > dx.abs() {
            verticals.push(s);
        } else {
            horizontals.push(s);
        }
    }
    let unsolved = |issue| Solution {
        correction: [1., 0., 0., 0., 1., 0., 0., 0., 1.],
        issue: Some(issue),
    };
    let (v, v_issue) = fixed(&verticals, f);
    let (h, h_issue) = fixed(&horizontals, f);
    issue = issue.or(v_issue).or(h_issue);
    let down = [0., 1., 0.];
    let right = [1., 0., 0.];
    let rotation = match (v, h) {
        (Fixed::Direction(v), _) if !horizontals.is_empty() => {
            let v = towards(v, down);
            let h = towards(nearest_across(v, &horizontals, f), right);
            rows(h, v)
        }
        (Fixed::Direction(v), _) => upright::align(towards(v, down), down),
        (_, Fixed::Direction(h)) if !verticals.is_empty() => {
            let h = towards(h, right);
            let v = towards(nearest_across(h, &verticals, f), down);
            rows(h, v)
        }
        (_, Fixed::Direction(h)) => upright::align(towards(h, right), right),
        (Fixed::Plane(nv), Fixed::Plane(nh)) => least_turn(nv, nh),
        _ => return unsolved(issue.unwrap_or(Issue::TooFew)),
    };
    let turned = ((rotation[0][0] + rotation[1][1] + rotation[2][2] - 1.) / 2.)
        .clamp(-1., 1.)
        .acos()
        .to_degrees();
    let g = upright::camera_turn(rotation, f);
    // Every corner of the photo must stay in front of the turned camera.
    let (hw, hh) = (0.5 * frame.shown.width, 0.5 * frame.shown.height);
    let in_front = [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]]
        .iter()
        .all(|&[x, y]| apply(g, [x, y, 1.])[2] > 0.1);
    if turned.is_nan() || turned > MAX_TURN || !in_front {
        return unsolved(Issue::TooSteep);
    }
    // How far the guides end up from vertical and horizontal.
    let off = |s: &Segment, upright: bool| {
        let p = |q: [f32; 2]| {
            let r = apply(g, [q[0], q[1], 1.]);
            [r[0] / r[2], r[1] / r[2]]
        };
        let (a, b) = (p(s.a), p(s.b));
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let (along, across) = if upright { (dy, dx) } else { (dx, dy) };
        across.abs().atan2(along.abs()).to_degrees()
    };
    let worst = verticals
        .iter()
        .map(|s| off(s, true))
        .chain(horizontals.iter().map(|s| off(s, false)))
        .fold(0f32, f32::max);
    if worst > DISAGREE {
        issue = issue.or(Some(Issue::Disagree));
    }
    Solution {
        correction: frame.shown.correction(g, UprightMode::Guided),
        issue,
    }
}

/// What edges of one kind fix of their direction, and whether some of them lie along
/// one line.
fn fixed(edges: &[Segment], f: f32) -> (Fixed, Option<Issue>) {
    let normals: Vec<[f32; 3]> = edges.iter().map(|s| upright::normal(s, f)).collect();
    let Some(&first) = normals.first() else {
        return (Fixed::Nothing, None);
    };
    let apart = |a: [f32; 3], b: [f32; 3]| {
        let c = cross(a, b);
        dot(c, c).sqrt() > SAME_LINE
    };
    let distinct = normals.iter().any(|&n| apart(first, n));
    let same_line = normals
        .iter()
        .enumerate()
        .any(|(i, &a)| normals[i + 1..].iter().any(|&b| !apart(a, b)));
    let issue = same_line.then_some(Issue::SameLine);
    if !distinct {
        return (Fixed::Plane(first), issue);
    }
    // The direction closest to every edge's plane.
    let mut m = [[0f32; 3]; 3];
    for n in &normals {
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += n[i] * n[j];
            }
        }
    }
    (Fixed::Direction(least_eigenvector(m)), issue)
}

/// The unit eigenvector of a symmetric 3 × 3 matrix's smallest eigenvalue, by Jacobi
/// rotations: exact however close the two smallest eigenvalues are, which power
/// iteration is not for a few guides.
fn least_eigenvector(m: Mat) -> [f32; 3] {
    let mut a: [[f64; 3]; 3] = m.map(|row| row.map(f64::from));
    let mut v = [[1f64, 0., 0.], [0., 1., 0.], [0., 0., 1.]];
    for _ in 0..50 {
        let (p, q) = [(0, 1), (0, 2), (1, 2)]
            .into_iter()
            .max_by(|&(i, j), &(k, l)| a[i][j].abs().total_cmp(&a[k][l].abs()))
            .unwrap_or((0, 1));
        if a[p][q].abs() < 1e-15 {
            break;
        }
        let theta = 0.5 * (2. * a[p][q]).atan2(a[q][q] - a[p][p]);
        let (s, c) = theta.sin_cos();
        // a ← Jᵀ a J and v ← v J, with J the rotation in the (p, q) plane.
        for row in &mut a {
            let (akp, akq) = (row[p], row[q]);
            row[p] = c * akp - s * akq;
            row[q] = s * akp + c * akq;
        }
        let (row_p, row_q) = (a[p], a[q]);
        for k in 0..3 {
            a[p][k] = c * row_p[k] - s * row_q[k];
            a[q][k] = s * row_p[k] + c * row_q[k];
        }
        for row in &mut v {
            let (vp, vq) = (row[p], row[q]);
            row[p] = c * vp - s * vq;
            row[q] = s * vp + c * vq;
        }
    }
    let i = (0..3)
        .min_by(|&i, &j| a[i][i].total_cmp(&a[j][j]))
        .unwrap_or(0);
    unit([v[0][i] as f32, v[1][i] as f32, v[2][i] as f32])
}

/// The direction at right angles to `fixed` closest to the planes of `edges`.
fn nearest_across(fixed: [f32; 3], edges: &[Segment], f: f32) -> [f32; 3] {
    // Two directions spanning the plane at right angles to `fixed`.
    let helper = if fixed[0].abs() < 0.9 {
        [1., 0., 0.]
    } else {
        [0., 1., 0.]
    };
    let a = unit(cross(fixed, helper));
    let b = cross(fixed, a);
    // Least squares in that plane: the smaller eigenvector of a 2 × 2 matrix.
    let (mut aa, mut ab, mut bb) = (0f32, 0f32, 0f32);
    for s in edges {
        let n = upright::normal(s, f);
        let (na, nb) = (dot(n, a), dot(n, b));
        aa += na * na;
        ab += na * nb;
        bb += nb * nb;
    }
    let angle = 0.5 * (2. * ab).atan2(aa - bb) + std::f32::consts::FRAC_PI_2;
    let (s, c) = angle.sin_cos();
    unit([
        c * a[0] + s * b[0],
        c * a[1] + s * b[1],
        c * a[2] + s * b[2],
    ])
}

/// `d` or its opposite, whichever points more along `towards`.
fn towards(d: [f32; 3], towards: [f32; 3]) -> [f32; 3] {
    if dot(d, towards) < 0. {
        d.map(|v| -v)
    } else {
        d
    }
}

/// The camera turn taking `h` to the right and `v` down: the rotation whose rows they
/// are (`v` at right angles to `h`).
fn rows(h: [f32; 3], v: [f32; 3]) -> Mat {
    [h, v, cross(h, v)]
}

/// One vertical and one horizontal edge, with planes `nv` and `nh`: of the turns that
/// make both right, the smallest.
fn least_turn(nv: [f32; 3], nh: [f32; 3]) -> Mat {
    let helper = if nh[0].abs() < 0.9 {
        [1., 0., 0.]
    } else {
        [0., 1., 0.]
    };
    let a = unit(cross(nh, helper));
    let b = cross(nh, a);
    // The horizontal direction runs in the plane `nh`; the vertical one is then fixed.
    let at = |t: f32| -> Option<Mat> {
        let (s, c) = t.sin_cos();
        let h = [
            c * a[0] + s * b[0],
            c * a[1] + s * b[1],
            c * a[2] + s * b[2],
        ];
        let v = cross(nv, h);
        (dot(v, v) > 1e-8).then(|| {
            let h = towards(h, [1., 0., 0.]);
            rows(h, towards(unit(v), [0., 1., 0.]))
        })
    };
    let trace = |m: &Mat| m[0][0] + m[1][1] + m[2][2];
    let score = |t: f32| at(t).map_or(f32::MIN, |m| trace(&m));
    let steps = 720;
    let step = std::f32::consts::PI / steps as f32;
    let mut best = (0..steps)
        .map(|i| i as f32 * step)
        .max_by(|x, y| score(*x).total_cmp(&score(*y)))
        .unwrap_or(0.);
    // Refined about the best sample by ternary search.
    let (mut lo, mut hi) = (best - step, best + step);
    for _ in 0..40 {
        let (m1, m2) = (lo + (hi - lo) / 3., hi - (hi - lo) / 3.);
        if score(m1) < score(m2) {
            lo = m1;
        } else {
            hi = m2;
        }
    }
    best = 0.5 * (lo + hi);
    at(best).unwrap_or(IDENTITY)
}

/// A guide as Camera Raw writes `UprightFourSegments_N`: "x1,y1,x2,y2", 0–1
/// coordinates separated by commas; None unless it is four finite numbers.
pub fn parse_guide(value: &str) -> Option<UprightGuide> {
    let v: Vec<f32> = value
        .split(',')
        .map(|x| x.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .ok()?;
    let [x1, y1, x2, y2]: [f32; 4] = v.try_into().ok()?;
    [x1, y1, x2, y2]
        .iter()
        .all(|x| x.is_finite())
        .then_some(UprightGuide {
            a: [x1, y1],
            b: [x2, y2],
        })
}

/// Solves this recipe's guides into its Guided correction (`UprightTransform_5`),
/// once the other modes' corrections are there to sit beside it; returns why the guides
/// correct less than they might. Without that analysis nothing changes: the editor
/// analyses the photo and solves the guides when it lands.
pub fn store(r: &mut Recipe, m: &Metadata) -> Option<Issue> {
    let code = UprightMode::Guided.code();
    if r.upright.corrections.len() < code {
        return None;
    }
    let solution = solve(&r.upright.guides, &GuideFrame::new(m, r));
    r.upright.corrections.truncate(code);
    r.upright.corrections.push(solution.correction);
    // Lightroom's record of the guides its Guided correction was solved from.
    r.upright.lightroom.retain(|key, _| {
        key != "UprightGuidedDependentDigest" && !key.starts_with("UprightFourSegments")
    });
    solution.issue
}

#[cfg(test)]
mod tests;
