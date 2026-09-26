//! Reference-calibrated color controls. This is an independent approximation,
//! not Adobe's proprietary color-grading implementation. Coefficients were
//! calibrated on isolated Lightroom color exports, then checked on a second RAW.
//! See docs/macos-lightroom-validation.md for the tested range and limitations.
use crate::{
    camera_profiles::{PRO_TO_RGB, RGB_TO_PRO},
    develop::Recipe,
};

pub(crate) fn vibrance_gain(hue: f32, chroma: f32, amount: f32) -> f32 {
    // A broad warm-hue guard protects skin without classifying image content.
    let distance = (hue - 0.16 + 0.5).rem_euclid(1.) - 0.5;
    let warm = (-(distance / 0.09).powi(2)).exp();
    1. + amount * 0.8 * (1. - (chroma / 0.3).clamp(0., 1.)) * (1. - 0.5 * warm)
}

pub(crate) fn hue_rgb(hue: f32) -> [f32; 3] {
    let h = hue.rem_euclid(1.) * 6.;
    let x = 1. - (h.rem_euclid(2.) - 1.).abs();
    match h as u32 {
        0 => [1., x, 0.],
        1 => [x, 1., 0.],
        2 => [0., 1., x],
        3 => [0., x, 1.],
        4 => [x, 0., 1.],
        _ => [1., 0., x],
    }
}
fn mul(m: [[f32; 3]; 3], p: [f32; 3]) -> [f32; 3] {
    m.map(|row| row.iter().zip(p).map(|(a, b)| a * b).sum())
}
fn tint_channel(value: f32, tint: f32) -> f32 {
    // Symmetric, endpoint-preserving response in the ProPhoto display domain.
    value + (2. * tint - 1.) * value * (1. - value)
}
fn lift(value: f32, amount: f32) -> f32 {
    // Most change belongs to the interior of the range, with a small endpoint
    // lift so shadow luminance can expose black to the tint operator.
    (value + amount * (0.01 + 0.4 * value * (1. - value))).clamp(0., 1.)
}
fn weights(light: f32, balance: f32, blending: f32) -> [f32; 3] {
    let l = (light + balance * 0.3).clamp(0., 1.);
    let power = 2f32.powf(1.5 - blending);
    [
        (1. - l).powf(power),
        0.5 * (4. * l * (1. - l)).powf(power),
        l.powf(power),
    ]
}
/// Work in the wide-gamut display-referred domain. The tint operator leaves
/// black and white endpoints intact; luminance is applied first so they can be toned.
pub(crate) fn grade(rgb: [f32; 3], r: &Recipe) -> [f32; 3] {
    if r.grading.iter().all(|g| g[1] == 0. && g[2] == 0.)
        && r.effects.global_grade[1] == 0.
        && r.effects.global_grade[2] == 0.
    {
        return rgb;
    }
    let mut p = mul(RGB_TO_PRO, rgb).map(|v| v.clamp(0., 1.).powf(1. / 2.2));
    let w = weights(
        p.iter().sum::<f32>() / 3.,
        r.effects.balance,
        r.effects.blending,
    );
    let mut tint = [0.; 3];
    let mut luminance = 0.;
    for (g, weight) in r.grading.iter().zip(w) {
        let color = hue_rgb(g[0]);
        for c in 0..3 {
            tint[c] += (color[c] - 0.5) * g[1] * weight * 0.7;
        }
        luminance += g[2] * weight;
    }
    for c in 0..3 {
        p[c] = tint_channel(lift(p[c], luminance), (0.5 + tint[c]).clamp(0., 1.));
    }
    // Global is a separate finishing pass, unaffected by balance or blending.
    let g = r.effects.global_grade;
    let color = hue_rgb(g[0]);
    for c in 0..3 {
        p[c] = tint_channel(lift(p[c], g[2]), 0.5 + (color[c] - 0.5) * g[1] * 0.4);
    }
    mul(PRO_TO_RGB, p.map(|v| v.powf(2.2)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wheel_matches_rgb_hues_and_wraps() {
        assert_eq!(hue_rgb(0.), [1., 0., 0.]);
        assert_eq!(hue_rgb(1. / 3.), [0., 1., 0.]);
        assert_eq!(hue_rgb(2. / 3.), [0., 0., 1.]);
        assert_eq!(hue_rgb(1.), hue_rgb(0.));
    }
    #[test]
    fn grade_is_identity_when_inactive_and_preserves_endpoints() {
        let mut r = Recipe::default();
        let pixel = [-0.01, 0.3, 1.1];
        assert_eq!(grade(pixel, &r), pixel);
        r.grading = [[0.61, 0.8, 0.], [0.3, 0.5, 0.], [0.1, 0.9, 0.]];
        for v in [0., 1.] {
            for c in grade([v; 3], &r) {
                assert!((c - v).abs() < 1e-5);
            }
        }
        r.grading[0][2] = 0.5;
        let lifted = grade([0.; 3], &r);
        assert!(lifted[2] > 0. && lifted[2] > lifted[0]);
    }
    #[test]
    fn balance_blending_and_global_are_independent() {
        let a = weights(0.5, -0.7, 0.5);
        let b = weights(0.5, 0.7, 0.5);
        assert!(a[0] > b[0] && a[2] < b[2]);
        assert!(weights(0.5, 0., 0.)[0] < weights(0.5, 0., 1.)[0]);
        let mut r = Recipe::default();
        r.effects.global_grade = [1. / 3., 0.5, 0.];
        let a = grade([0.2; 3], &r);
        r.effects.balance = 1.;
        r.effects.blending = 0.;
        assert_eq!(a, grade([0.2; 3], &r));
        assert!(a[1] > a[0] && a[1] > a[2]);
    }
    #[test]
    fn vibrance_protects_warm_and_saturated_colors() {
        assert!(vibrance_gain(0.16, 0.1, 0.6) < vibrance_gain(0.7, 0.1, 0.6));
        assert_eq!(vibrance_gain(0.7, 0.3, 0.6), 1.);
        assert_eq!(vibrance_gain(0.7, 0.1, 0.), 1.);
    }
}
