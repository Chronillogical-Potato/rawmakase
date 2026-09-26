//! Camera reference metadata, not fitted image corrections.
//! X100F values were read from Lightroom-generated DNGs of DSCF7845 (ISO 400)
//! and DSCF7853 (ISO 200), both DR100. See docs/macos-lightroom-validation.md.
//! Sony values were measured against Camera Raw renders.
use crate::raw::Metadata;
fn x100f(m: &Metadata) -> bool {
    m.make.eq_ignore_ascii_case("Fujifilm") && m.model.eq_ignore_ascii_case("X100F")
}
pub fn baseline_exposure(m: &Metadata) -> f32 {
    if let Some(dng) = m.baseline_exposure {
        return dng;
    }
    // Other DR modes have not been calibrated; never extrapolate an ISO rule.
    if x100f(m) && m.fuji_dynamic_range == 100 {
        return 0.15;
    }
    // Measured against Camera Raw 18.6 renders with Adobe Standard: 0.25 leaves these
    // bodies 0.03–0.07 EV dark and 0.35 leaves them 0.04–0.08 EV bright (8 A7 II and
    // 4 A7CR photos, 2026-09-26).
    let sony =
        |model: &str| m.make.eq_ignore_ascii_case("Sony") && m.model.eq_ignore_ascii_case(model);
    if sony("ILCE-7M2") || sony("ILCE-7CR") {
        return 0.3;
    }
    0.
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
    #[test]
    fn baseline_is_limited_to_validated_camera_and_dr_mode() {
        let mut m = Metadata {
            make: "Fujifilm".into(),
            model: "X100F".into(),
            fuji_dynamic_range: 100,
            ..Default::default()
        };
        assert_eq!(baseline_exposure(&m), 0.15);
        m.fuji_dynamic_range = 200;
        assert_eq!(baseline_exposure(&m), 0.);
        m.model = "X100V".into();
        m.fuji_dynamic_range = 100;
        assert_eq!(baseline_exposure(&m), 0.);
        assert_eq!(neutral_calibration(&m), [1.; 3]);
        let sony = Metadata {
            make: "SONY".into(),
            model: "ILCE-7M2".into(),
            ..Default::default()
        };
        assert_eq!(baseline_exposure(&sony), 0.3);
    }
}
