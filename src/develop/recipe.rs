use super::panels::PanelSwitches;
use super::white_balance::{estimate_temperature, illuminant_camera};
use crate::{develop::curve::ToneCurve, raw::Metadata};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
/// Lightroom's white balance slider ranges for RAW files.
pub const TEMPERATURE_MIN: f32 = 2000.;
pub const TEMPERATURE_MAX: f32 = 50000.;
pub const TINT_LIMIT: f32 = 150.;
/// A photo's develop settings. Fields this build does not know (from a newer release)
/// are kept in `unknown` and saved again, so an older build never drops them.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
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
    /// Lightroom's manual Distortion (Lens Corrections > Manual,
    /// `crs:LensManualDistortionAmount` / 100, −1 to 1): positive corrects barrel
    /// distortion, negative pincushion. Omitted at 0, so releases that predate it read
    /// the recipe (and keep the field when it is set).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub lens_manual_distortion: f32,
    /// Lightroom's Remove Chromatic Aberration: red and blue fringing measured from the
    /// photo itself, in place of any lens data's lateral CA (`crs:AutoLateralCA`).
    #[serde(default)]
    pub lens_ca: bool,
    /// Use the DCP tone curve without a second generic scene shoulder.
    #[serde(default)]
    pub profile_tone: bool,
    pub effects: crate::develop::effects::Effects,
    pub preset_name: String,
    pub preset_settings: std::collections::BTreeMap<String, String>,
    pub profile: Option<std::sync::Arc<crate::camera_profiles::CameraProfile>>,
    /// Lightroom's Profile Amount (`crs:Look` `Amount`, 0–2, 1 = 100%): the strength of
    /// a look that supports it. Other profiles render at 100% whatever it is. Omitted
    /// at 1, so releases that predate it read the recipe.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub profile_amount: f32,
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
    /// Temperature and Tint that Auto white balance chose, so the WB menu shows Auto
    /// while the photo still has them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_white_balance: Option<[f32; 2]>,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub black_point: f32,
    pub white_point: f32,
    pub midtone: f32,
    pub curve: ToneCurve,
    /// Lightroom's Refine Saturation (`crs:CurveRefineSaturation` / 100): how much of the
    /// saturation the master point curve adds or removes is kept. 1 is Lightroom's
    /// default; Camera Raw renders values above 1 as 1. Omitted at 1, so releases that
    /// predate it read the recipe (and keep the field when it is set).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub curve_saturation: f32,
    pub saturation: f32,
    pub vibrance: f32,
    pub hsl: [[f32; 3]; 8],
    pub grading: [[f32; 3]; 3],
    pub noise_luma: f32,
    pub noise_chroma: f32,
    pub sharpening: f32,
    pub crop: [f32; 4],
    pub straighten: f32,
    /// Lightroom's Constrain Crop (`crs:CropConstrainToWarp`): the crop as rendered
    /// shrinks, keeping its aspect, to leave out the white areas Upright, the Transform
    /// sliders and manual Distortion uncover. `crop` stays as the user set it. Omitted
    /// when off, so releases that predate it read the recipe.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub constrain_crop: bool,
    /// Transform panel sliders (engine 4).
    #[serde(default)]
    pub transform: crate::develop::Transform,
    /// Lightroom's Upright (engine 4), applied before the Transform sliders.
    #[serde(default, skip_serializing_if = "crate::develop::Upright::is_default")]
    pub upright: crate::develop::Upright,
    pub rotation: u8,
    pub flip_x: bool,
    pub flip_y: bool,
    /// Heal and Clone operations, in order. Saved apart from the recipe (see
    /// [`LocalEdits`]); omitted from recipe JSON when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retouch: Vec<crate::develop::retouch::RetouchOp>,
    /// Red eye corrections, in order; saved apart, as `retouch`.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::red_eye::RedEyeList::is_blank"
    )]
    pub red_eye: crate::develop::red_eye::RedEyeList,
    /// Masks with local adjustments; saved apart, as `retouch`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<crate::develop::masks::MaskGroup>,
    /// Lightroom's panel switches; omitted while every panel is on.
    #[serde(default, skip_serializing_if = "PanelSwitches::all_on")]
    pub panels: PanelSwitches,
    /// Settings from a newer release, preserved as they were.
    #[serde(flatten)]
    pub unknown: std::collections::BTreeMap<String, serde_json::Value>,
}
/// A recipe's spot removal, red eye corrections and masks (experimental). They are saved beside the recipe,
/// not in it, so releases that predate them still read every other setting.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LocalEdits {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub retouch: Vec<crate::develop::retouch::RetouchOp>,
    #[serde(skip_serializing_if = "crate::develop::red_eye::RedEyeList::is_blank")]
    pub red_eye: crate::develop::red_eye::RedEyeList,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<crate::develop::masks::MaskGroup>,
}
impl LocalEdits {
    pub fn is_empty(&self) -> bool {
        self.retouch.is_empty() && self.red_eye.is_blank() && self.masks.is_empty()
    }
    pub fn validate(&self) -> Result<()> {
        crate::develop::retouch::validate(&self.retouch)?;
        crate::develop::red_eye::validate(&self.red_eye)?;
        crate::develop::masks::validate(&self.masks)
    }
}
impl Default for Recipe {
    fn default() -> Self {
        Self {
            engine: 4,
            lens_builtin: true,
            lens_profile: false,
            lens_distortion: 1.,
            lens_vignetting: 1.,
            lens_manual_distortion: 0.,
            lens_ca: false,
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
            auto_white_balance: None,
            contrast: 0.,
            highlights: 0.,
            shadows: 0.,
            whites: 0.,
            blacks: 0.,
            black_point: 0.,
            white_point: 1.,
            midtone: 1.,
            curve: ToneCurve::default(),
            curve_saturation: 1.,
            profile_amount: 1.,
            saturation: 0.,
            vibrance: 0.,
            hsl: [[0.; 3]; 8],
            grading: [[0.; 3]; 3],
            noise_luma: 0.,
            noise_chroma: 0.,
            sharpening: 0.35,
            crop: [0., 0., 1., 1.],
            straighten: 0.,
            constrain_crop: false,
            transform: Default::default(),
            upright: Default::default(),
            rotation: 0,
            flip_x: false,
            flip_y: false,
            retouch: Vec::new(),
            red_eye: Default::default(),
            masks: Vec::new(),
            panels: PanelSwitches::default(),
            unknown: Default::default(),
        }
    }
}
/// Which profile a photo's starting settings use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfilePreference {
    /// Adobe Color, else Adobe Standard, else the DNG's own, else RAWmakase Color.
    Adobe,
    /// RAWmakase Color wherever it fits the camera, else as `Adobe`.
    Rawmakase,
}
impl Recipe {
    /// The look at its Profile Amount, and its internal controls added to the user's
    /// sliders without changing them.
    pub(crate) fn with_profile_adjustments(&self) -> std::borrow::Cow<'_, Self> {
        let Some((profile, look)) = self
            .profile
            .as_ref()
            .and_then(|p| Some((p, p.enhanced.as_ref()?)))
            .filter(|_| self.engine >= 3)
        else {
            return std::borrow::Cow::Borrowed(self);
        };
        let amount = if look.amount.is_some() {
            self.profile_amount
        } else {
            1.
        };
        let adjustments = [
            look.highlights,
            look.shadows,
            look.clarity,
            look.contrast,
            look.blacks,
        ];
        if amount == 1.
            && adjustments.iter().all(|v| *v == 0.)
            && !look.monochrome
            && look.settings.is_default()
        {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut r = self.clone();
        let mut look = look.clone();
        if amount != 1. {
            let profile = profile.at_amount(amount);
            look = profile.enhanced.clone().unwrap_or(look);
            r.profile = Some(std::sync::Arc::new(profile));
            r.profile_amount = 1.;
        }
        r.highlights = (r.highlights + look.highlights).clamp(-1., 1.);
        r.shadows = (r.shadows + look.shadows).clamp(-1., 1.);
        r.effects.clarity = (r.effects.clarity + look.clarity).clamp(-1., 1.);
        r.contrast = (r.contrast + look.contrast).clamp(-1., 1.);
        r.blacks = (r.blacks + look.blacks).clamp(-1., 1.);
        r.effects.monochrome |= look.monochrome;
        r.add_look_settings(&look.settings);
        std::borrow::Cow::Owned(r)
    }
    /// A look's Exposure, Saturation, colour mixer and parametric curve added to the
    /// user's; its split toning for the shadows or highlights the user doesn't tone,
    /// and its vignette in place of the user's, as Camera Raw 18.7 renders them.
    fn add_look_settings(&mut self, s: &crate::camera_profiles::LookSettings) {
        let add = |a: f32, b: f32| (a + b).clamp(-1., 1.);
        self.exposure = (self.exposure + s.exposure).clamp(-8., 8.);
        self.saturation = add(self.saturation, s.saturation);
        for (band, look) in self.hsl.iter_mut().zip(s.hsl) {
            for (v, l) in band.iter_mut().zip(look) {
                *v = add(*v, l);
            }
        }
        let e = &mut self.effects;
        if s.parametric != [0.; 4] {
            for (v, l) in e.parametric.iter_mut().zip(s.parametric) {
                *v = add(*v, l);
            }
            if e.splits == crate::develop::effects::Effects::default().splits {
                e.splits = s.splits;
            }
        }
        if let Some(t) = s.toning {
            // Split toning, as Lightroom's looks store it, overlaps all tones; the
            // user's own toning of shadows or highlights wins over the look's.
            if self.grading.iter().all(|g| g[1] == 0. && g[2] == 0.) && e.global_grade == [0.; 3] {
                e.blending = 1.;
            }
            for (slot, [hue, saturation]) in [(0, t.shadows), (2, t.highlights)] {
                if self.grading[slot][1] == 0. {
                    self.grading[slot] = [hue, saturation, self.grading[slot][2]];
                }
            }
            if e.balance == 0. {
                e.balance = t.balance;
            }
        }
        if let Some(v) = s.vignette {
            e.vignette = v.amount.clamp(-1., 1.);
            e.vignette_midpoint = v.midpoint;
            e.vignette_feather = v.feather;
            e.vignette_roundness = v.roundness;
            e.vignette_highlights = v.highlights;
            e.vignette_style = v.style.try_into().unwrap_or_default();
        }
    }

    pub fn for_metadata(m: &Metadata) -> Self {
        Self {
            profile: crate::camera_profiles::builtin(m),
            temperature: estimate_temperature(m),
            lens_builtin: m.lens.as_ref().is_none_or(|l| l.default_on),
            ..Default::default()
        }
    }
    /// Lightroom's Adobe Default: the settings a photo starts from.
    pub fn with_profiles(
        m: &Metadata,
        profiles: &[std::sync::Arc<crate::camera_profiles::CameraProfile>],
    ) -> Self {
        Self::with_profile_preference(m, profiles, ProfilePreference::Adobe)
    }
    /// The starting settings, with the profile `preference` picks.
    pub fn with_profile_preference(
        m: &Metadata,
        profiles: &[std::sync::Arc<crate::camera_profiles::CameraProfile>],
        preference: ProfilePreference,
    ) -> Self {
        let mut recipe = Self::for_metadata(m);
        let find = |name: &str| {
            profiles
                .iter()
                .find(|p| p.name == name && p.ensure_camera(m).is_ok())
        };
        let own = match preference {
            ProfilePreference::Adobe => None,
            ProfilePreference::Rawmakase => find(crate::camera_profiles::open::COLOR),
        };
        // As in Lightroom: Adobe Color, else Adobe Standard. Without those, a DNG
        // keeps the profile it embeds, and any other file gets RAWmakase Color.
        if let Some(profile) = own
            .or_else(|| find("Adobe Color"))
            .or_else(|| find("Adobe Standard"))
            .or_else(|| {
                recipe
                    .profile
                    .is_none()
                    .then(|| find(crate::camera_profiles::open::COLOR))
                    .flatten()
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
        ensure!(self.upright.validate(), "Invalid Upright");
        ensure!(
            (0. ..=2.).contains(&self.lens_distortion)
                && (0. ..=2.).contains(&self.lens_vignetting)
                && (-1. ..=1.).contains(&self.lens_manual_distortion),
            "Invalid lens correction amount"
        );
        ensure!(
            (0. ..=2.).contains(&self.curve_saturation),
            "Invalid Refine Saturation"
        );
        ensure!(
            (0. ..=2.).contains(&self.profile_amount),
            "Invalid Profile Amount"
        );
        // Every other number is range-checked above, which also rejects NaN.
        crate::develop::retouch::validate(&self.retouch)?;
        crate::develop::red_eye::validate(&self.red_eye)?;
        crate::develop::masks::validate(&self.masks)?;
        Ok(())
    }
    /// The recipe as saved, without spots and masks, and those apart.
    pub fn split_local(&self) -> (Recipe, LocalEdits) {
        let mut saved = self.clone();
        let local = LocalEdits {
            retouch: std::mem::take(&mut saved.retouch),
            red_eye: std::mem::take(&mut saved.red_eye),
            masks: std::mem::take(&mut saved.masks),
        };
        (saved, local)
    }
    /// Adds spots and masks saved apart. Ones already in the recipe (written by
    /// development builds into the recipe itself) stay when `local` has none.
    pub fn with_local(mut self, local: LocalEdits) -> Recipe {
        if !local.retouch.is_empty() {
            self.retouch = local.retouch;
        }
        if !local.red_eye.is_blank() {
            self.red_eye = local.red_eye;
        }
        if !local.masks.is_empty() {
            self.masks = local.masks;
        }
        self
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
    /// Lightroom's Enable Profile Corrections: an imported Adobe lens profile, else the
    /// correction the camera stored in the RAW. Built-in data that is off by default
    /// (Sony's) follows the switch; Fuji's and a DNG's stay on, as in Lightroom.
    pub fn set_profile_corrections(&mut self, m: &Metadata, state: ProfileCorrections) {
        self.lens_profile = state == ProfileCorrections::On;
        // Lens corrections render from process version 4 only.
        if self.engine >= 4 && m.lens.as_ref().is_some_and(|l| !l.default_on) {
            self.lens_builtin = self.lens_profile;
        }
    }
    /// Why Enable Profile Corrections cannot render with an Adobe profile here, and
    /// what renders instead, when it is on.
    pub fn missing_lens_profile(&self, m: &Metadata) -> Option<String> {
        // A Lens Corrections panel switched off renders no lens correction at all.
        let panel = self
            .panels
            .state(crate::develop::panels::Panel::LensCorrections);
        if !self.lens_profile
            || m.profile_lens.is_some()
            || panel == crate::develop::panels::PanelState::Off
            || self.engine < 4
        {
            return None;
        }
        let lens = if m.lens_model.is_empty() {
            "this lens".to_string()
        } else {
            m.lens_model.clone()
        };
        Some(match m.lens.as_ref().filter(|_| self.lens_builtin) {
            Some(builtin) => format!(
                "Adobe lens profile for {lens} isn't imported; using {}",
                builtin.source
            ),
            None => format!("Adobe lens profile for {lens} isn't imported; no lens correction"),
        })
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
    /// Recipe as rendered: switched-off panels bypassed, profile-internal adjustments
    /// and the engine-4 default profile.
    pub(crate) fn resolved(&self, m: &Metadata) -> std::borrow::Cow<'_, Self> {
        let mut r = match self.as_rendered() {
            std::borrow::Cow::Borrowed(r) => r.with_profile_adjustments(),
            std::borrow::Cow::Owned(r) => {
                std::borrow::Cow::Owned(r.with_profile_adjustments().into_owned())
            }
        };
        // A Lens Corrections panel switched off also turns off built-in data that is
        // off by default (Sony's), which Enable Profile Corrections turned on; what a
        // camera always applies (Fuji, DNG) stays, as the panel bypass leaves it.
        if self
            .panels
            .state(crate::develop::panels::Panel::LensCorrections)
            == crate::develop::panels::PanelState::Off
            && r.lens_builtin
            && m.lens.as_ref().is_some_and(|l| !l.default_on)
        {
            r.to_mut().lens_builtin = false;
        }
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
    /// Settings that follow a newly chosen profile: an XMP look renders through its
    /// own tone and curves, and the camera baseline and white balance controls are
    /// those of the new profile.
    pub fn profile_changed(&mut self, m: &Metadata) {
        if self.profile.as_ref().is_some_and(|p| p.enhanced.is_some()) {
            self.profile_tone = true;
            self.reference_curves = true;
            self.wide_gamut_curves = true;
        }
        self.use_camera_baseline(m);
        self.sync_white_balance_controls(m);
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
        self.auto_white_balance = None;
    }
    pub fn sync_white_balance_controls(&mut self, m: &Metadata) {
        let mut adjusted = m.clone();
        adjusted.wb = std::array::from_fn(|c| m.wb[c] * self.wb[c]);
        // A profile that cannot map gains back (no ColorMatrix1) is also left to the
        // fallback model, which is what update_wb then uses.
        let Some([temperature, tint]) = self
            .color_profile(m)
            .and_then(|p| p.as_shot_white_balance(&adjusted))
        else {
            self.sync_fallback_white_balance_controls(m);
            return;
        };
        self.temperature = temperature.clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
        self.tint = tint.clamp(-TINT_LIMIT, TINT_LIMIT);
        // Gains the controls cannot show become the nearest they can, so the next
        // Temperature or Tint edit does not jump.
        if self.temperature != temperature || self.tint != tint {
            self.update_wb(m);
        }
    }
    /// Temperature and Tint that [`Self::update_wb`] maps to the current gains, solved
    /// under its fallback model (recipes without a camera profile). Temperature sets the red/blue
    /// ratio and Tint scales both against green, so each is solved in turn.
    pub(super) fn sync_fallback_white_balance_controls(&mut self, m: &Metadata) {
        let model = |temperature: f32| {
            let mut r = self.clone();
            r.temperature = temperature;
            r.tint = 0.;
            r.update_wb(m);
            r.wb
        };
        let red_blue = |wb: [f32; 3]| (wb[0].max(1e-6) / wb[2].max(1e-6)).ln();
        let target = red_blue(self.wb);
        // The model's locus covers 2000–15000 K; the ratio is monotonic along it.
        let (mut lo, mut hi) = (2000f32, 15000f32);
        let rising = red_blue(model(hi)) > red_blue(model(lo));
        for _ in 0..40 {
            let mid = (lo * hi).sqrt();
            if (red_blue(model(mid)) < target) == rising {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let temperature = (lo * hi).sqrt();
        let neutral = model(temperature);
        let tint = 50.
            * ((self.wb[0] * self.wb[2]).max(1e-12) / (neutral[0] * neutral[2]).max(1e-12)).log2();
        self.temperature = temperature.clamp(TEMPERATURE_MIN, TEMPERATURE_MAX);
        self.tint = tint.clamp(-TINT_LIMIT, TINT_LIMIT);
        // Gains the controls cannot reproduce become the ones they show, so the next
        // Temperature or Tint edit does not jump. That covers gains past either end of
        // the locus or the Tint limit, and a profile whose white balance Tint does not
        // scale the way the fallback model's does.
        let mut shown = self.clone();
        shown.update_wb(m);
        if (0..3).any(|c| (shown.wb[c] / self.wb[c].max(1e-6) - 1.).abs() > 1e-3) {
            self.wb = shown.wb;
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
fn is_zero(v: &f32) -> bool {
    *v == 0.
}
fn is_one(v: &f32) -> bool {
    *v == 1.
}
/// Whether Enable Profile Corrections is ticked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileCorrections {
    On,
    Off,
}
fn one() -> f32 {
    1.
}
