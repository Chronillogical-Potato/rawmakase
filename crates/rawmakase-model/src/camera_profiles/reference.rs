//! Camera reference metadata, not fitted image corrections.
//! X100F values were read from Lightroom-generated DNGs of DSCF7845 (ISO 400)
//! and DSCF7853 (ISO 200), both DR100. See docs/macos-lightroom-validation.md.
//! Per-camera baseline exposures live in data/cameras.toml (crate::cameras).
use crate::camera_data::{HighlightTonePriority, Metadata};
/// Camera Raw's default exposure for this photo: a DNG's own BaselineExposure,
/// otherwise the camera table's value (data/cameras.toml). A rendered file has none.
pub fn baseline_exposure(m: &Metadata) -> f32 {
    if m.is_rendered() {
        return 0.;
    }
    if let Some(dng) = m.baseline_exposure {
        return dng;
    }
    let row = crate::cameras::baseline_exposure(&m.make, &m.model);
    let table = row.ev + fujifilm_shift(m, &row);
    // Table rows hold for photos without Highlight Tone Priority, which Camera Raw
    // brightens by the stop the camera held back.
    match m.highlight_tone_priority {
        HighlightTonePriority::Off => table,
        HighlightTonePriority::On | HighlightTonePriority::Enhanced => table + 1.,
    }
}
/// Camera Raw's baseline for a Fujifilm raw is a per-body constant minus the raw's
/// exposure midpoint shift, which moves a stop for each DR step (DR200 is often not
/// recorded otherwise) and for extended ISO. Table rows hold at the body's DR100 shift:
/// the row's own, else the sensor's usual one.
fn fujifilm_shift(m: &Metadata, row: &crate::cameras::Baseline) -> f32 {
    let Some(shift) = m.fuji_exposure_shift else {
        return 0.;
    };
    if !m.make.eq_ignore_ascii_case("Fujifilm")
        || row.exposure_shift == crate::cameras::ExposureShift::Ignored
    {
        return 0.;
    }
    let usual = if m.xtrans { -0.72 } else { 0. };
    row.dr100_shift.unwrap_or(usual) - shift
}
pub fn neutral_calibration(m: &Metadata) -> [f32; 3] {
    crate::cameras::neutral_calibration(&m.make, &m.model, &m.wb_presets)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn camera(make: &str, model: &str) -> Metadata {
        Metadata {
            make: make.into(),
            model: model.into(),
            ..Default::default()
        }
    }
    fn fujifilm(model: &str, xtrans: bool, shift: Option<f32>) -> Metadata {
        Metadata {
            xtrans,
            fuji_exposure_shift: shift,
            ..camera("Fujifilm", model)
        }
    }
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }
    #[test]
    fn fujifilm_baselines_follow_the_exposure_shift() {
        // Rows hold at the usual DR100 shift: -0.72 for X-Trans, -0.49 for GFX.
        let row = |model| crate::cameras::baseline_exposure("Fujifilm", model).ev;
        let x_e5 = fujifilm("X-E5", true, Some(-0.72));
        assert_eq!(baseline_exposure(&x_e5), row("X-E5"));
        // DR200 (the raw often does not say so) shifts a stop: Camera Raw adds it back.
        let x_e5 = fujifilm("X-E5", true, Some(-1.72));
        assert!(close(baseline_exposure(&x_e5), row("X-E5") + 1.));
        // Extended ISO 100 on an ISO 160 body shifts the other way.
        let x_t30 = fujifilm("X-T30", true, Some(0.28));
        assert!(close(baseline_exposure(&x_t30), row("X-T30") - 1.));
        // GFX rows give their own DR100 shift, -0.49.
        let gfx = fujifilm("GFX100 II", false, Some(-1.72));
        assert!(close(baseline_exposure(&gfx), row("GFX100 II") + 1.23));
        // A Bayer body without one assumes 0, whatever its name.
        let unlisted = fujifilm("GFX200", false, Some(-1.));
        assert!(close(baseline_exposure(&unlisted), row("GFX200") + 1.));
        // Small Bayer bodies record no shift at DR100.
        let xf10 = Metadata {
            fuji_dynamic_range: 65535,
            ..fujifilm("XF10", false, Some(0.))
        };
        assert_eq!(baseline_exposure(&xf10), row("XF10"));
        // Without a shift in the raw, the row as it is.
        assert_eq!(baseline_exposure(&fujifilm("X-T5", true, None)), -0.1);
        // Camera Raw keeps one value for the X-Trans III bodies whatever the shift.
        let x_t2 = fujifilm("X-T2", true, Some(-1.72));
        assert_eq!(baseline_exposure(&x_t2), 0.15);
        assert_eq!(
            baseline_exposure(&fujifilm("X100F", true, Some(-1.72))),
            0.15
        );
        assert_eq!(
            neutral_calibration(&fujifilm("Unlisted body", true, None)),
            [1.; 3]
        );
    }
    #[test]
    fn olympus_units_use_their_5300k_preset() {
        use crate::camera_data::WhiteBalancePresets;
        // Two E-M5 Mark III units: their calibrations differ exactly as their
        // stored 5300 K presets do (data/camera-calibration-facts.json).
        let unit = |v| Metadata {
            wb_presets: WhiteBalancePresets {
                fine_weather: Some(v),
                ..Default::default()
            },
            ..camera("Olympus", "E-M5 Mark III")
        };
        assert_eq!(
            neutral_calibration(&unit([460., 256., 540.])),
            [1.0478, 1., 0.8741]
        );
        assert_eq!(
            neutral_calibration(&unit([512., 256., 462.])),
            [0.9414, 1., 1.0216]
        );
    }
    #[test]
    fn canon_and_fujifilm_units_use_their_own_presets() {
        use crate::camera_data::WhiteBalancePresets;
        // Anonymous native presets of separate units, with the CameraCalibration
        // Adobe DNG Converter 18.7 recorded for each; see data/camera-calibration-facts.json.
        let unit = |make: &str, model: &str, wb_presets| Metadata {
            wb_presets,
            ..camera(make, model)
        };
        let daylight = |v| WhiteBalancePresets {
            daylight: Some(v),
            ..Default::default()
        };
        let d65 = |v| WhiteBalancePresets {
            d65: Some(v),
            ..Default::default()
        };
        for (m, expected) in [
            (
                unit("Canon", "EOS 800D", daylight([1872., 1024., 1649.])),
                [1.0112, 1., 1.0291],
            ),
            (
                unit("Canon", "EOS 800D", daylight([1876., 1024., 1623.])),
                [1.0091, 1., 1.0456],
            ),
            (
                unit("Fujifilm", "X-T5", d65([658., 302., 473.])),
                [0.9347, 1., 1.055],
            ),
            (
                unit("Fujifilm", "X-T5", d65([646., 302., 494.])),
                [0.952, 1., 1.0101],
            ),
            // Lightroom's DNGs of the owner's X100F, and another X100F.
            (
                unit("Fujifilm", "X100F", d65([681., 302., 484.])),
                [0.9883, 1., 1.031],
            ),
            (
                unit("Fujifilm", "X100F", d65([679., 302., 500.])),
                [0.9912, 1., 0.998],
            ),
            // Bayer bodies' doubled multipliers give the same ratios.
            (
                unit("Fujifilm", "X-T200", d65([1118., 512., 852.])),
                [0.9338, 1., 0.9695],
            ),
        ] {
            assert_eq!(neutral_calibration(&m), expected, "{} {}", m.make, m.model);
        }
        // Some Canon bodies' reference is their Shade preset, not Daylight.
        let presets = WhiteBalancePresets {
            daylight: Some([1827., 1024., 1686.]),
            shade: Some([2106., 1024., 1464.]),
            ..Default::default()
        };
        assert_eq!(
            neutral_calibration(&unit("Canon", "EOS 200D", presets)),
            [1.0247, 1., 1.026]
        );
        // Without the row's preset, identity rather than another unit's correction.
        let other = daylight([658., 302., 473.]);
        assert_eq!(
            neutral_calibration(&unit("Fujifilm", "X-T5", other)),
            [1.; 3]
        );
        // Signs that would cancel in the ratios are still invalid.
        let negative = d65([-681., -302., -484.]);
        assert_eq!(
            neutral_calibration(&unit("Fujifilm", "X100F", negative)),
            [1.; 3]
        );
        let none = WhiteBalancePresets::default();
        assert_eq!(
            neutral_calibration(&unit("Fujifilm", "X100F", none)),
            [1.; 3]
        );
        // Adobe calibrates neither Nikon bodies nor these Fujifilm ones.
        let nikon = daylight([2000., 1024., 1500.]);
        assert_eq!(neutral_calibration(&unit("Nikon", "Z 6_2", nikon)), [1.; 3]);
        assert_eq!(
            neutral_calibration(&unit("Fujifilm", "X-T50", d65([644., 302., 498.]))),
            [1.; 3]
        );
    }
    #[test]
    fn baseline_covers_measured_cameras() {
        assert_eq!(baseline_exposure(&camera("SONY", "ILCE-7M2")), 0.35);
        assert_eq!(baseline_exposure(&camera("Sony", "ILCE-7M4")), 0.35);
        assert_eq!(baseline_exposure(&camera("Canon", "EOS R7")), 0.4);
        assert_eq!(baseline_exposure(&camera("Nikon", "Z 30")), 0.35);
        // An unlisted body follows its make.
        assert_eq!(
            baseline_exposure(&camera("Canon", "EOS R1")),
            crate::cameras::baseline_exposure("Canon", "EOS R1").ev
        );
        // The Canon median, 0.3 EV with the older bodies' rows.
        assert!(baseline_exposure(&camera("Canon", "EOS R1")) > 0.2);
    }
    #[test]
    fn canon_highlight_tone_priority_adds_a_stop() {
        let mut m = camera("Canon", "EOS R8");
        let normal = baseline_exposure(&m);
        m.highlight_tone_priority = HighlightTonePriority::On;
        assert_eq!(baseline_exposure(&m), normal + 1.);
        m.highlight_tone_priority = HighlightTonePriority::Enhanced;
        assert_eq!(baseline_exposure(&m), normal + 1.);
        // A DNG's own value already includes it.
        m.baseline_exposure = Some(1.27);
        assert_eq!(baseline_exposure(&m), 1.27);
    }
    #[test]
    fn dngs_use_their_own_baseline() {
        let mut dng = camera("Sony", "ILCE-7M4");
        dng.baseline_exposure = Some(-0.2);
        assert_eq!(baseline_exposure(&dng), -0.2);
        dng.baseline_exposure = Some(0.);
        assert_eq!(baseline_exposure(&dng), 0.);
    }
}
