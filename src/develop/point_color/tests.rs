use super::*;

fn swatch(values: [f32; 19], variance: f32) -> PointColor {
    PointColor {
        source: [values[0], values[1], values[2]],
        shift: [values[3], values[4], values[5]],
        range: values[6],
        hue_range: values[7..11].try_into().unwrap(),
        saturation_range: values[11..15].try_into().unwrap(),
        luminance_range: values[15..19].try_into().unwrap(),
        variance,
    }
}

fn two_swatches(variances: [f32; 2]) -> Vec<PointColor> {
    vec![
        swatch(
            [
                1.0, 0.6, 0.15, 0.4, -0.3, 0.2, 0.3, 0.0, 0.333333, 0.666667, 1.0, 0.0, 0.42, 0.78,
                1.0, 0.0, 0.243583, 0.603583, 1.0,
            ],
            variances[0],
        ),
        swatch(
            [
                5.9, 0.5, 0.3, -0.5, 0.5, -0.4, 0.8, 0.0, 0.2, 0.6, 1.0, 0.0, 0.32, 0.68, 1.0, 0.0,
                0.403831, 0.763831, 1.0,
            ],
            variances[1],
        ),
    ]
}

const PIXELS: [[f32; 3]; 6] = [
    [0.2, 0.15, 0.05],
    [0.3, 0.1, 0.12],
    [0.18, 0.18, 0.18],
    [0.05, 0.3, 0.04],
    [0.4, 0.08, 0.15],
    [0.25, 0.2, 0.1],
];

/// The fitted model (scratch Python used for the fit) on the same swatches and pixels.
#[test]
fn matches_the_fitted_model() {
    let expected = [
        (
            [0., 0.],
            [
                [0.230742, 0.208478, 0.086651],
                [0.209296, 0.006977, 0.086218],
                [0.18, 0.18, 0.18],
                [0.05, 0.3, 0.04],
                [0.301718, 0.0, 0.134922],
                [0.288756, 0.266704, 0.144711],
            ],
        ),
        (
            [0.4, -0.6],
            [
                [0.233657, 0.194803, 0.082297],
                [0.209296, 0.021147, 0.094839],
                [0.18, 0.18, 0.18],
                [0.05, 0.3, 0.04],
                [0.296327, 0.0056, 0.119383],
                [0.295009, 0.257322, 0.147845],
            ],
        ),
    ];
    for (variances, outputs) in expected {
        let list = two_swatches(variances);
        assert!(list.iter().all(PointColor::is_valid));
        let op = PointColors::new(&list).unwrap();
        for (p, want) in PIXELS.iter().zip(outputs) {
            let got = op.apply_prophoto(*p);
            for c in 0..3 {
                assert!(
                    (got[c] - want[c]).abs() < 2e-4,
                    "{p:?}: {got:?} vs {want:?}"
                );
            }
        }
    }
}

#[test]
fn neutrals_and_unselected_colors_stay() {
    let mut p = PointColor::sampled([1.0, 0.6, 0.15]);
    p.shift = [1., 1., 1.];
    let op = PointColors::new(&[p]).unwrap();
    for gray in [0.01, 0.18, 0.9] {
        assert_eq!(op.apply_prophoto([gray; 3]), [gray; 3]);
    }
    // A green, far from the orange swatch.
    assert_eq!(op.apply_prophoto([0.05, 0.3, 0.04]), [0.05, 0.3, 0.04]);
}

#[test]
fn hues_wrap_around_red() {
    // Swatches just below and just above red select reds on both sides of 0.
    for hue in [5.95, 0.05] {
        let mut p = PointColor::sampled([hue, 0.6, 0.2]);
        p.shift = [0., -1., 0.];
        let op = PointColors::new(&[p]).unwrap();
        for side in [hsv_to_rgb(-0.1, 0.6, 0.3), hsv_to_rgb(0.1, 0.6, 0.3)] {
            let [_, s, _] = rgb_to_hsv(op.apply_prophoto(side));
            assert!(s < 0.45, "{hue}: {s}");
        }
    }
}

#[test]
fn overlapping_swatches_apply_in_turn() {
    let mut a = PointColor::sampled([1.0, 0.6, 0.15]);
    a.shift = [0.3, 0., 0.];
    let mut b = PointColor::sampled([1.2, 0.6, 0.15]);
    b.shift = [0., 0.5, -0.4];
    let pixel = a.source_prophoto();
    let one = |p: &PointColor, x| PointColors::new(&[*p]).unwrap().apply_prophoto(x);
    let both = PointColors::new(&[a, b]).unwrap().apply_prophoto(pixel);
    assert_eq!(both, one(&b, one(&a, pixel)));
    assert_ne!(both, one(&a, one(&b, pixel)));
    // The first swatch turns its own color by Hue × 35°.
    let h = rgb_to_hsv(pixel)[0];
    assert!((wrap(rgb_to_hsv(one(&a, pixel))[0] - h) - 0.3 * fit::HUE_SHIFT).abs() < 1e-3);
}

#[test]
fn camera_raws_invalid_swatches_are_dropped() {
    let mut p = PointColor::sampled([1.0, 0.6, 0.15]);
    p.shift[0] = 0.5;
    assert!(p.is_valid());
    let invalid = [
        PointColor {
            source: [6.1, 0.6, 0.15],
            ..p
        },
        PointColor {
            hue_range: [0., 0.5, 0.5, 1.],
            ..p
        },
        PointColor {
            saturation_range: [0., 0.65, 0.8, 1.],
            ..p
        },
        PointColor {
            luminance_range: [0., 0.32, 0.38, 1.],
            ..p
        },
    ];
    for q in invalid {
        assert!(!q.is_valid(), "{q:?}");
        assert!(PointColors::new(&[q]).is_none());
    }
    // Lightroom's empty selection is 19 values of -1.
    assert!(!swatch([-1.; 19], -0.5).is_valid());
}

#[test]
fn sampled_swatches_hold_their_color() {
    for source in [[0.0, 0.1, 0.01], [3.2, 0.95, 0.9], [5.99, 0.5, 1.0]] {
        let p = PointColor::sampled(source);
        assert!(p.is_valid(), "{p:?}");
        assert!(!p.is_active());
        assert!(PointColors::new(&[p]).is_none());
        let back = rgb_to_hsv(p.source_prophoto());
        assert!((back[0] / TAU * 6. - source[0]).abs() < 1e-4);
    }
}
