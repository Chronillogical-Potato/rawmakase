//! Camera reference metadata, not fitted image corrections.
//! X100F values were read from Lightroom-generated DNGs of DSCF7845 (ISO 400)
//! and DSCF7853 (ISO 200), both DR100. See docs/macos-lightroom-validation.md.
//! Per-camera baseline exposures live in data/cameras.toml (crate::cameras).
use crate::raw::Metadata;
fn x100f(m: &Metadata) -> bool {
    m.make.eq_ignore_ascii_case("Fujifilm") && m.model.eq_ignore_ascii_case("X100F")
}
/// Camera Raw's default exposure for this photo: a DNG's own BaselineExposure,
/// otherwise the camera table's value (data/cameras.toml).
pub fn baseline_exposure(m: &Metadata) -> f32 {
    if let Some(dng) = m.baseline_exposure {
        return dng;
    }
    // Fujifilm rows hold for DR100; other DR modes have not been calibrated.
    if m.make.eq_ignore_ascii_case("Fujifilm") && m.fuji_dynamic_range != 100 {
        return 0.;
    }
    crate::cameras::baseline_exposure(&m.make, &m.model).ev
}
pub fn neutral_calibration(m: &Metadata) -> [f32; 3] {
    if x100f(m) {
        [0.9883, 1., 1.031]
    } else {
        [1.; 3]
    }
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
    #[test]
    fn fujifilm_baselines_hold_for_dr100_only() {
        let mut m = camera("Fujifilm", "X100F");
        m.fuji_dynamic_range = 100;
        assert_eq!(baseline_exposure(&m), 0.15);
        m.fuji_dynamic_range = 200;
        assert_eq!(baseline_exposure(&m), 0.);
        m.model = "X-T5".into();
        m.fuji_dynamic_range = 100;
        assert_eq!(baseline_exposure(&m), -0.1);
        m.model = "X100V".into();
        assert_eq!(neutral_calibration(&m), [1.; 3]);
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
    fn dngs_use_their_own_baseline() {
        let mut dng = camera("Sony", "ILCE-7M4");
        dng.baseline_exposure = Some(-0.2);
        assert_eq!(baseline_exposure(&dng), -0.2);
        dng.baseline_exposure = Some(0.);
        assert_eq!(baseline_exposure(&dng), 0.);
    }
}
