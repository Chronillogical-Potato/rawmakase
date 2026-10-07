//! HSV of linear RGB, with hue in radians, as Point Color and its sampling read it.
use std::f32::consts::TAU;

/// HSV with hue in radians.
pub fn rgb_to_hsv(p: [f32; 3]) -> [f32; 3] {
    let max = p.into_iter().fold(f32::NEG_INFINITY, f32::max);
    let min = p.into_iter().fold(f32::INFINITY, f32::min);
    let d = max - min;
    let sextant = if d <= 0. {
        0.
    } else if max == p[0] {
        ((p[1] - p[2]) / d).rem_euclid(6.)
    } else if max == p[1] {
        (p[2] - p[0]) / d + 2.
    } else {
        (p[0] - p[1]) / d + 4.
    };
    let s = if max > 0. { d / max } else { 0. };
    [sextant / 6. * TAU, s, max]
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h6 = (h / TAU).rem_euclid(1.) * 6.;
    let c = v * s;
    let x = c * (1. - (h6 % 2. - 1.).abs());
    let m = v - c;
    let [r, g, b] = match h6 as usize {
        0 => [c, x, 0.],
        1 => [x, c, 0.],
        2 => [0., c, x],
        3 => [0., x, c],
        4 => [x, 0., c],
        _ => [c, 0., x],
    };
    [r + m, g + m, b + m]
}
