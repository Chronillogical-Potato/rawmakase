use super::white_balance::{estimate_temperature, illuminant_camera};
use crate::{develop::curve::ToneCurve, raw::Metadata};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
/// Lightroom's white balance slider ranges for RAW files.
pub const TEMPERATURE_MIN: f32 = 2000.;
pub const TEMPERATURE_MAX: f32 = 50000.;
pub const TINT_LIMIT: f32 = 150.;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Recipe {
    pub engine: u32,
    /// Apply the lens correction the camera stored in the RAW (engine 4). Missing in older
    /// recipes, which therefore keep rendering without it.
    #[serde(default)]
    pub lens_builtin: bool,
    /// Lightroom's Enable Profile Corrections: use an imported Adobe lens profile that
    /// matches the lens, in place of the built-in correction.
    #[serde(default)]
    pub lens_profile: bool,
    /// Profile correction amounts, Lightroom's Distortion and Vignetting sliders
    /// (0–2, 1 = 100).
    #[serde(default = "one")]
    pub lens_distortion: f32,
    #[serde(default = "one")]
    pub lens_vignetting: f32,
    /// Use the DCP tone curve without a second generic scene shoulder.
    #[serde(default)]
    pub profile_tone: bool,
    pub effects: crate::develop::effects::Effects,
    pub preset_name: String,
    pub preset_settings: std::collections::BTreeMap<String, String>,
    pub profile: Option<std::sync::Arc<crate::camera_profiles::CameraProfile>>,
    pub sharpening_radius: f32,
    pub sharpening_detail: f32,
    pub sharpening_masking: f32,
    pub exposure: f32,
    #[serde(default)]
    pub camera_exposure: f32,
    #[serde(default)]
    pub wide_gamut_curves: bool,
    #[serde(default)]
    pub reference_curves: bool,
    #[serde(default)]
    pub reference_calibration: bool,
    /// RGB-hue grading and reference-calibrated color response. Missing means legacy.
    #[serde(default)]
    pub reference_color: bool,
    pub temperature: f32,
    pub tint: f32,
    pub wb: [f32; 3],
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub black_point: f32,
    pub white_point: f32,
    pub midtone: f32,
    pub curve: ToneCurve,
    pub saturation: f32,
    pub vibrance: f32,
    pub hsl: [[f32; 3]; 8],
    pub grading: [[f32; 3]; 3],
    pub noise_luma: f32,
    pub noise_chroma: f32,
    pub sharpening: f32,
    pub crop: [f32; 4],
    pub straighten: f32,
    /// Transform panel sliders (engine 4).
    #[serde(default)]
    pub transform: crate::develop::Transform,
    pub rotation: u8,
    pub flip_x: bool,
    pub flip_y: bool,
}
impl Default for Recipe {
    fn default() -> Self {
        Self {
            engine: 4,
            lens_builtin: true,
            lens_profile: false,
            lens_distortion: 1.,
            lens_vignetting: 1.,
            profile_tone: true,
            effects: Default::default(),
            preset_name: String::new(),
            preset_settings: Default::default(),
            profile: None,
            sharpening_radius: 0.8,
            sharpening_detail: 0.25,
            sharpening_masking: 0.35,
            exposure: 0.,
            camera_exposure: 0.,
            wide_gamut_curves: false,
            reference_curves: false,
            reference_calibration: false,
            reference_color: false,
            temperature: 6500.,
            tint: 0.,
            wb: [1.; 3],
            contrast: 0.,
            highlights: 0.,
            shadows: 0.,
            whites: 0.,
            blacks: 0.,
            black_point: 0.,
            white_point: 1.,
            midtone: 1.,
            curve: ToneCurve::default(),
            saturation: 0.,
            vibrance: 0.,
            hsl: [[0.; 3]; 8],
            grading: [[0.; 3]; 3],
            noise_luma: 0.,
            noise_chroma: 0.,
            sharpening: 0.35,
            crop: [0., 0., 1., 1.],
            straighten: 0.,
            transform: Default::default(),
            rotation: 0,
            flip_x: false,
            flip_y: false,
        }
    }
}
impl Recipe {
    /// Profile-internal controls are evaluated without changing the user's sliders.
    pub(crate) fn with_profile_adjustments(&self) -> std::borrow::Cow<'_, Self> {
        let Some(look) = self
            .profile
            .as_ref()
            .and_then(|p| p.enhanced.as_ref())
            .filter(|_| self.engine >= 3)
        else {
            return std::borrow::Cow::Borrowed(self);
        };
        if look.highlights == 0. && look.shadows == 0. && look.clarity == 0. && !look.monochrome {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut r = self.clone();
        r.highlights = (r.highlights + look.highlights).clamp(-1., 1.);
        r.shadows = (r.shadows + look.shadows).clamp(-1., 1.);
        r.effects.clarity = (r.effects.clarity + look.clarity).clamp(-1., 1.);
        r.effects.monochrome |= look.monochrome;
        std::borrow::Cow::Owned(r)
    }

    pub fn for_metadata(m: &Metadata) -> Self {
        Self {
            profile: crate::camera_profiles::builtin(m),
            temperature: estimate_temperature(m),
            lens_builtin: m.lens.as_ref().is_none_or(|l| l.default_on),
            ..Default::default()
        }
    }
    pub fn with_profiles(
        m: &Metadata,
        profiles: &[std::sync::Arc<crate::camera_profiles::CameraProfile>],
    ) -> Self {
        let mut recipe = Self::for_metadata(m);
        if let Some(profile) = ["Adobe Color", "Adobe Standard"]
            .into_iter()
            .find_map(|name| {
                profiles
                    .iter()
                    .find(|p| p.name == name && p.ensure_camera(m).is_ok())
            })
        {
            recipe.profile = Some(profile.clone());
        }
        recipe.wide_gamut_curves = recipe.profile.is_some();
        recipe.reference_color = true;
        recipe.reference_curves = true;
        recipe.reference_calibration = true;
        recipe.use_camera_baseline(m);
        recipe.reset_white_balance(m);
        recipe
    }
    pub fn validate(&self) -> Result<()> {
        self.effects.validate()?;
        ensure!(
            (1..=4).contains(&self.engine),
            "Unsupported rendering engine"
        );
        ensure!(
            (0.5..=3.).contains(&self.sharpening_radius)
                && (0. ..=1.).contains(&self.sharpening_detail)
                && (0. ..=1.).contains(&self.sharpening_masking),
            "Invalid sharpening controls"
        );
        if let Some(p) = &self.profile {
            p.validate()?;
        }
        let mut scalar_recipe = self.clone();
        scalar_recipe.profile = None;
        let mut finite = serde_json::to_value(&scalar_recipe)?;
        finite.as_object_mut().unwrap().remove("profile");
        fn all(v: &serde_json::Value) -> bool {
            match v {
                serde_json::Value::Null => false,
                serde_json::Value::Array(a) => a.iter().all(all),
                serde_json::Value::Object(a) => a.values().all(all),
                _ => true,
            }
        }
        ensure!(all(&finite), "Non-finite edit parameter");
        ensure!(
            (-8. ..=8.).contains(&self.exposure)
                && self.camera_exposure.is_finite()
                && self.camera_exposure.abs() <= 5.,
            "Exposure out of bounds"
        );
        ensure!(
            (TEMPERATURE_MIN..=TEMPERATURE_MAX).contains(&self.temperature)
                && self.tint.abs() <= TINT_LIMIT,
            "White balance out of bounds"
        );
        ensure!(
            self.wb
                .iter()
                .all(|v| v.is_finite() && *v >= 0.01 && *v <= 100.),
            "Invalid white balance"
        );
        for v in [
            self.contrast,
            self.highlights,
            self.shadows,
            self.whites,
            self.blacks,
            self.saturation,
            self.vibrance,
        ] {
            ensure!(v.abs() <= 1., "Adjustment out of bounds");
        }
        ensure!(
            self.black_point >= 0.
                && self.white_point <= 1.
                && self.white_point - self.black_point >= 0.01
                && (0.1..=4.).contains(&self.midtone),
            "Invalid levels"
        );
        self.curve.validate()?;
        ensure!(
            self.hsl
                .iter()
                .flatten()
                .chain(self.grading.iter().flatten())
                .all(|v| v.abs() <= 1.),
            "Invalid color adjustment"
        );
        ensure!(
            [self.noise_luma, self.noise_chroma, self.sharpening]
                .iter()
                .all(|v| (0. ..=1.).contains(v)),
            "Invalid detail adjustment"
        );
        ensure!(
            self.crop.iter().all(|v| (0. ..=1.).contains(v))
                && self.crop[2] - self.crop[0] >= 0.01
                && self.crop[3] - self.crop[1] >= 0.01,
            "Invalid crop"
        );
        ensure!(
            self.rotation < 4 && self.straighten.abs() <= 45.,
            "Invalid rotation"
        );
        ensure!(self.transform.validate(), "Invalid transform");
        ensure!(
            (0. ..=2.).contains(&self.lens_distortion)
                && (0. ..=2.).contains(&self.lens_vignetting),
            "Invalid lens correction amount"
        );
        Ok(())
    }
    /// The camera profile used for rendering. From engine 4, photos without an imported or
    /// bundled profile render through the DNG ColorMatrix default instead of the legacy path.
    pub(crate) fn color_profile(
        &self,
        m: &Metadata,
    ) -> Option<std::borrow::Cow<'_, crate::camera_profiles::CameraProfile>> {
        match &self.profile {
            Some(p) => Some(std::borrow::Cow::Borrowed(p.as_ref())),
            None if self.engine >= 4 => {
                crate::camera_profiles::CameraProfile::camera_matrix_default(m)
                    .map(std::borrow::Cow::Owned)
            }
            None => None,
        }
    }
    /// The built-in lens correction to apply, if enabled and present in the file.
    pub(crate) fn lens_correction<'a>(
        &self,
        m: &'a Metadata,
    ) -> Option<&'a crate::lens::LensCorrection> {
        if self.engine < 4 {
            return None;
        }
        m.profile_lens
            .as_ref()
            .filter(|_| self.lens_profile)
            .or_else(|| m.lens.as_ref().filter(|_| self.lens_builtin))
    }
    /// Recipe as rendered: profile-internal adjustments plus the engine-4 default profile.
    pub(crate) fn resolved(&self, m: &Metadata) -> std::borrow::Cow<'_, Self> {
        let r = self.with_profile_adjustments();
        if r.profile.is_some() || r.engine < 4 {
            return r;
        }
        let Some(profile) = crate::camera_profiles::CameraProfile::camera_matrix_default(m) else {
            return r;
        };
        let mut r = r.into_owned();
        r.profile = Some(std::sync::Arc::new(profile));
        r.profile_tone = true;
        std::borrow::Cow::Owned(r)
    }
    pub fn use_camera_baseline(&mut self, m: &Metadata) {
        self.camera_exposure = if self.profile.is_some() || self.engine >= 4 {
            crate::camera_profiles::reference::baseline_exposure(m)
        } else {
            0.
        };
    }
    pub fn reset_white_balance(&mut self, m: &Metadata) {
        let values = self
            .color_profile(m)
            .and_then(|p| p.as_shot_white_balance(m))
            .unwrap_or([estimate_temperature(m), 0.]);
        self.temperature = values[0].clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
        self.tint = values[1].clamp(-TINT_LIMIT, TINT_LIMIT);
        self.wb = [1.; 3];
    }
    pub fn sync_white_balance_controls(&mut self, m: &Metadata) {
        let mut adjusted = m.clone();
        adjusted.wb = std::array::from_fn(|c| m.wb[c] * self.wb[c]);
        if let Some([temperature, tint]) = self
            .color_profile(m)
            .and_then(|p| p.as_shot_white_balance(&adjusted))
        {
            self.temperature = temperature.clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
            self.tint = tint.clamp(-TINT_LIMIT, TINT_LIMIT);
        }
    }
    pub fn update_wb(&mut self, m: &Metadata) {
        if let Some(wb) = self
            .color_profile(m)
            .and_then(|p| p.white_balance(self.temperature, self.tint, m))
        {
            self.wb = wb;
            return;
        }
        let current = illuminant_camera(self.temperature, m);
        let baseline = illuminant_camera(estimate_temperature(m), m);
        for c in 0..3 {
            self.wb[c] = ((baseline[c] / baseline[1]) / (current[c] / current[1]).max(0.001))
                .clamp(0.01, 100.);
        }
        self.wb[1] *= 2f32.powf(-self.tint / 100.);
        let g = self.wb[1];
        for v in &mut self.wb {
            *v /= g;
        }
    }
}
fn one() -> f32 {
    1.
}
