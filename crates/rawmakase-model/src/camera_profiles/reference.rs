//! Camera reference metadata, not fitted image corrections.
//! X100F values were read from Lightroom-generated DNGs of DSCF7845 (ISO 400)
//! and DSCF7853 (ISO 200), both DR100. See docs/macos-lightroom-validation.md.
//! Per-camera baseline exposures live in data/cameras.toml (crate::cameras).
use crate::camera_data::{HighlightTonePriority, Metadata};
/// Camera Raw's default exposure for this photo: a DNG's own BaselineExposure,
/// otherwise the camera table's value (data/cameras.toml).
pub fn baseline_exposure(m: &Metadata) -> f32 {
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
    crate::cameras::neutral_calibration(&m.make, &m.model, m.sony_daylight_wb)
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
    fn baseline_covers_measured_cameras() {
        assert_eq!(baseline_exposure(&camera("SONY", "ILCE-7M2")), 0.3);
        assert_eq!(baseline_exposure(&camera("Sony", "ILCE-7M4")), 0.35);
        assert_eq!(baseline_exposure(&camera("Canon", "EOS R7")), 0.4);
        assert_eq!(baseline_exposure(&camera("Nikon", "Z 30")), 0.35);
        // An unlisted body follows its make.
        assert_eq!(
            baseline_exposure(&camera("Canon", "EOS R1")),
            crate::cameras::baseline_exposure("Canon", "EOS R1").ev
        );
        assert!(baseline_exposure(&camera("Canon", "EOS R1")) > 0.3);
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
