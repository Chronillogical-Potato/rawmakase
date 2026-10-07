use super::panels::PanelSwitches;
use super::white_balance::{estimate_temperature, illuminant_camera};
use crate::{camera_data::Metadata, develop::curve::ToneCurve};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
/// Lightroom's white balance slider ranges for RAW files.
pub const TEMPERATURE_MIN: f32 = 2000.;
pub const TEMPERATURE_MAX: f32 = 50000.;
pub const TINT_LIMIT: f32 = 150.;
/// The Exposure a recipe may hold, in EV. The slider spans ±5; imported edits and
/// typed values may go further.
pub const EXPOSURE_LIMIT: f32 = 8.;
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
    /// Lightroom's profile Setup and the profile the edit names. Omitted at Default
    /// with no profile named, so releases that predate it read the recipe.
    #[serde(
        default,
        skip_serializing_if = "crate::lens::choice::LensProfileChoice::is_default"
    )]
    pub lens_profile_choice: crate::lens::choice::LensProfileChoice,
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
    /// Which operator renders Grain. Missing means the original grain, so recipes
    /// saved before the measured one look as they did; omitted at that default, and
    /// kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::effects::GrainModel::is_original"
    )]
    pub grain_model: crate::develop::effects::GrainModel,
    /// Which operator renders positive Clarity, as `grain_model`.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::clarity::ClarityModel::is_original"
    )]
    pub clarity_model: crate::develop::clarity::ClarityModel,
    /// Which operator renders Texture, as `grain_model`.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::texture::TextureModel::is_original"
    )]
    pub texture_model: crate::develop::texture::TextureModel,
    pub preset_name: String,
    pub preset_settings: std::collections::BTreeMap<String, String>,
    pub profile: Option<std::sync::Arc<crate::camera_profiles::CameraProfile>>,
    /// Lightroom's Profile Amount (`crs:Look` `Amount`, 0–2, 1 = 100%): the strength of
    /// a look that supports it. Other profiles render at 100% whatever it is. Omitted
    /// at 1, so releases that predate it read the recipe.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub profile_amount: f32,
    /// Which operator renders Sharpening. Missing means the original unsharp mask, so
    /// recipes saved before the measured one look as they did; omitted at that
    /// default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::sharpening::SharpeningModel::is_original"
    )]
    pub sharpening_model: crate::develop::sharpening::SharpeningModel,
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
    /// How the Tone Curve's parametric regions render. Missing means the original
    /// approximation, so recipes saved before the measured curve look as they did;
    /// omitted at that default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::parametric::ParametricModel::is_original"
    )]
    pub parametric_model: crate::develop::parametric::ParametricModel,
    /// Whether Contrast pivots where the photo puts it, after Whites and Blacks.
    /// Missing means the original averaged curve before them, so older recipes look as
    /// they did; omitted at that default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::basic_tone::ContrastModel::is_original"
    )]
    pub contrast_model: crate::develop::basic_tone::ContrastModel,
    /// How manual lens Vignetting renders. Missing means the original operator, so
    /// recipes saved before the measured one look as they did; omitted at that
    /// default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::effects::LensVignetteModel::is_original"
    )]
    pub lens_vignette_model: crate::develop::effects::LensVignetteModel,
    /// The soft edge Heal and Clone render with. Missing means the original one, so
    /// recipes saved before the measured feather look as they did; omitted at that
    /// default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::retouch::RetouchModel::is_original"
    )]
    pub retouch_model: crate::develop::retouch::RetouchModel,
    /// How color grading renders. Missing means the original operator, so recipes
    /// saved before the measured curves look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::color_grade::GradingModel::is_original"
    )]
    pub grading_model: crate::develop::color_grade::GradingModel,
    /// Which measured tables render the color mixer. Missing means the tables
    /// measured on photos, so older recipes look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::color_mixer::MixerModel::is_original"
    )]
    pub mixer_model: crate::develop::color_mixer::MixerModel,
    /// Which operator renders the global Saturation slider. Missing means the
    /// photo-measured tables, so older recipes look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::color_mixer::SaturationModel::is_original"
    )]
    pub saturation_model: crate::develop::color_mixer::SaturationModel,
    /// Which tables render Vibrance. Missing means the photo-measured ones, so older
    /// recipes look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::color_mixer::VibranceModel::is_original"
    )]
    pub vibrance_model: crate::develop::color_mixer::VibranceModel,
    /// Which operator renders the black & white mix. Missing means the Oklab
    /// lightness shift, so older recipes look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::black_white::BlackWhiteModel::is_original"
    )]
    pub black_white_model: crate::develop::black_white::BlackWhiteModel,
    /// Which fit renders Camera Calibration's primary sliders. Missing means the
    /// original coefficients, so older recipes look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::calibration::CalibrationModel::is_original"
    )]
    pub calibration_model: crate::develop::calibration::CalibrationModel,
    /// Whether positive Whites follows the photo's highlights. Missing means the
    /// original median curve, so older recipes look as they did; omitted at that
    /// default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::basic_tone::WhitesModel::is_original"
    )]
    pub whites_model: crate::develop::basic_tone::WhitesModel,
    /// How out-of-gamut colors reach sRGB. Missing means compressed, so recipes saved
    /// before the clipped model look as they did; omitted at that default.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::GamutModel::is_compress"
    )]
    pub gamut_model: crate::develop::GamutModel,
    /// How Color noise reduction renders. Missing means the original operator, so
    /// recipes saved before the measured one look as they did; omitted at that
    /// default, and kept by releases that predate it.
    #[serde(
        default,
        skip_serializing_if = "crate::develop::color_noise::NoiseModel::is_original"
    )]
    pub noise_model: crate::develop::color_noise::NoiseModel,
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
    /// Lightroom's Point Color swatches, at most eight (`crs:PointColors` and
    /// `crs:ColorVariance`). Omitted when empty, so releases that predate it read the
    /// recipe (and keep the swatches when they are set).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub point_colors: Vec<crate::develop::point_color::PointColor>,
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
            lens_profile_choice: Default::default(),
            lens_distortion: 1.,
            lens_vignetting: 1.,
            lens_manual_distortion: 0.,
            lens_ca: false,
            profile_tone: true,
            effects: Default::default(),
            grain_model: Default::default(),
            clarity_model: Default::default(),
            texture_model: Default::default(),
            preset_name: String::new(),
            preset_settings: Default::default(),
            profile: None,
            sharpening_model: Default::default(),
            sharpening_radius: 0.8,
            sharpening_detail: 0.25,
            sharpening_masking: 0.35,
            exposure: 0.,
            camera_exposure: 0.,
            wide_gamut_curves: false,
            reference_curves: false,
            reference_calibration: false,
            reference_color: false,
            parametric_model: Default::default(),
            contrast_model: Default::default(),
            lens_vignette_model: Default::default(),
            retouch_model: Default::default(),
            grading_model: Default::default(),
            mixer_model: Default::default(),
            saturation_model: Default::default(),
            vibrance_model: Default::default(),
            black_white_model: Default::default(),
            calibration_model: Default::default(),
            whites_model: Default::default(),
            gamut_model: Default::default(),
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
            point_colors: Vec::new(),
            grading: [[0.; 3]; 3],
            noise_luma: 0.,
            noise_chroma: 0.,
            noise_model: Default::default(),
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
    /// Lightroom's Camera Settings: the imported profile matching the camera's
    /// standard look (see [`camera_matching_profile`]), else as `Adobe`.
    Camera,
}
/// Names of the camera-matching profile Adobe ships for a camera's standard
/// look, by maker. RAWmakase doesn't read the picture style set in the camera,
/// so Camera Settings always starts from this one.
pub fn camera_matching_names(m: &Metadata) -> &'static [&'static str] {
    let make = m.make.trim().to_ascii_lowercase();
    if make.starts_with("fujifilm") {
        &["Camera PROVIA/Standard"]
    } else {
        &["Camera Standard"]
    }
}
/// The imported camera-matching profile for this camera's standard look.
pub fn camera_matching_profile<'a>(
    m: &Metadata,
    profiles: &'a [std::sync::Arc<crate::camera_profiles::CameraProfile>],
) -> Option<&'a std::sync::Arc<crate::camera_profiles::CameraProfile>> {
    camera_matching_names(m).iter().find_map(|name| {
        profiles
            .iter()
            .find(|p| p.name == *name && p.ensure_camera(m).is_ok())
    })
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
        // The layered curve renders a look's parametric curve as a curve of its own,
        // on the measured path only; elsewhere the look's regions join the user's.
        let merge_parametric = !(self.parametric_model
            == crate::develop::parametric::ParametricModel::Layered
            && self.engine >= 4
            && self.reference_curves);
        if s.parametric != [0.; 4] && merge_parametric {
            for (v, l) in e.parametric.iter_mut().zip(s.parametric) {
                *v = add(*v, l);
            }
            if e.splits == crate::develop::effects::Effects::default().splits {
                e.splits = s.splits;
            }
        }
        // The measured grading renders a look's split toning as a pass of its own.
        let merge_toning =
            self.grading_model == crate::develop::color_grade::GradingModel::Original;
        if let Some(t) = s.toning.filter(|_| merge_toning) {
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
            ProfilePreference::Camera => camera_matching_profile(m, profiles),
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
        recipe.parametric_model = crate::develop::parametric::ParametricModel::Layered;
        recipe.set_sharpening_defaults(crate::develop::sharpening::SharpeningModel::Measured);
        recipe.grain_model = crate::develop::effects::GrainModel::Measured;
        recipe.clarity_model = crate::develop::clarity::ClarityModel::Measured;
        recipe.texture_model = crate::develop::texture::TextureModel::Measured;
        recipe.contrast_model = crate::develop::basic_tone::ContrastModel::Adaptive;
        recipe.lens_vignette_model = crate::develop::effects::LensVignetteModel::Measured;
        recipe.retouch_model = crate::develop::retouch::RetouchModel::Measured;
        recipe.grading_model = crate::develop::color_grade::GradingModel::Measured;
        recipe.mixer_model = crate::develop::color_mixer::MixerModel::Chart;
        recipe.saturation_model = crate::develop::color_mixer::SaturationModel::Gray;
        recipe.vibrance_model = crate::develop::color_mixer::VibranceModel::Chart;
        recipe.black_white_model = crate::develop::black_white::BlackWhiteModel::Chart;
        recipe.calibration_model = crate::develop::calibration::CalibrationModel::Measured;
        recipe.whites_model = crate::develop::basic_tone::WhitesModel::Adaptive;
        recipe.gamut_model = crate::develop::GamutModel::Clip;
        recipe.set_color_noise_defaults(crate::develop::color_noise::NoiseModel::Measured);
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
            (-EXPOSURE_LIMIT..=EXPOSURE_LIMIT).contains(&self.exposure)
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
    /// The imported Adobe profile Enable Profile Corrections uses here, and the one
    /// the edit names when it isn't imported.
    pub fn lens_profile_in_use<'c, 'p>(
        &'c self,
        m: &'p Metadata,
    ) -> crate::lens::choice::Resolved<'c, 'p> {
        if !self.lens_profile || self.engine < 4 {
            return Default::default();
        }
        self.lens_profile_choice.resolve(&m.lens_profiles, m)
    }
    /// Why Enable Profile Corrections cannot render with the Adobe profile it should
    /// here, and what renders instead, when it is on.
    pub fn missing_lens_profile(&self, m: &Metadata) -> Option<String> {
        // A Lens Corrections panel switched off renders no lens correction at all.
        let panel = self
            .panels
            .state(crate::develop::panels::Panel::LensCorrections);
        if panel == crate::develop::panels::PanelState::Off || !self.lens_profile || self.engine < 4
        {
            return None;
        }
        let resolved = self.lens_profile_in_use(m);
        // The profile the RAW carries renders as its built-in correction.
        if let Some(id) = self
            .lens_profile_choice
            .id
            .as_ref()
            .filter(|id| id.embedded)
        {
            return m.lens.as_ref().filter(|_| self.lens_builtin).is_none().then(|| {
                format!(
                    "Lens profile \"{}\" comes with the camera, but this file has none; no lens correction",
                    id.label()
                )
            });
        }
        let profile = match (resolved.missing, m.lens_model.as_str()) {
            (Some(id), _) => format!("Lens profile \"{}\"", id.label()),
            (None, _) if resolved.used.is_some() => return None,
            (None, "") => "Adobe lens profile for this lens".to_string(),
            (None, lens) => format!("Adobe lens profile for {lens}"),
        };
        let instead = match (resolved.used, m.lens.as_ref().filter(|_| self.lens_builtin)) {
            (Some(used), _) => format!("using {}", used.profile.name),
            (None, Some(builtin)) => format!("using {}", builtin.source),
            (None, None) => "no lens correction".to_string(),
        };
        Some(format!("{profile} isn't imported; {instead}"))
    }
    /// The lens correction to apply: the Adobe profile in use, else the built-in
    /// correction if enabled and present in the file.
    /// Manual lens Vignetting as measured in Camera Raw, applied with the lens
    /// profile's to the camera image; `None` at Amount 0 and for recipes that keep the
    /// original operator, which [`crate::develop::effects::spatial_finish`] applies.
    pub(crate) fn manual_vignette(&self) -> Option<crate::develop::effects::ManualVignette> {
        if self.lens_vignette_model.is_original() {
            return None;
        }
        crate::develop::effects::ManualVignette::new(
            self.effects.lens_vignette,
            self.effects.lens_vignette_midpoint,
        )
    }
    /// Adds a Heal or Clone operation. The first on a recipe has no spots of the
    /// original feather to keep, so it takes the measured one.
    pub fn add_retouch(&mut self, op: crate::develop::retouch::RetouchOp) {
        if self.retouch.is_empty() {
            self.retouch_model = crate::develop::retouch::RetouchModel::Measured;
        }
        self.retouch.push(op);
    }
    /// After an edit of manual Vignetting from Amount `previous`: an Amount moved from 0
    /// has nothing of the original operator's to keep, so it takes the measured one.
    pub fn adopt_measured_vignette(&mut self, previous: f32) {
        if previous == 0. && self.effects.lens_vignette != 0. {
            self.lens_vignette_model = crate::develop::effects::LensVignetteModel::Measured;
        }
    }
    /// The manual lens Vignetting Amount the finishing stage applies: only the
    /// original operator's; the measured one is applied with the lens profile.
    pub(crate) fn finished_lens_vignette(&self) -> f32 {
        if self.lens_vignette_model.is_original() {
            self.effects.lens_vignette
        } else {
            0.
        }
    }
    /// Calibration's Update: the current process, with the camera's built-in profile
    /// and the sharpening that version 3 introduced for older recipes.
    pub fn update_process(&mut self, m: Option<&Metadata>) {
        if self.engine < 3 {
            self.profile = m.and_then(crate::camera_profiles::builtin);
            if self.sharpening == 0. {
                self.sharpening =
                    crate::develop::sharpening::SharpeningSliders::defaults(self.sharpening_model)
                        .amount;
            }
        }
        self.engine = 4;
    }
    /// Color noise reduction at its defaults for `model`, which the recipe then uses:
    /// Lightroom's Amount 25 for raw files with the measured operator (Detail and
    /// Smoothness 50), off with the original.
    pub fn set_color_noise_defaults(&mut self, model: crate::develop::color_noise::NoiseModel) {
        let d = crate::develop::effects::Effects::default();
        self.noise_model = model;
        self.noise_chroma = match model {
            crate::develop::color_noise::NoiseModel::Original => 0.,
            crate::develop::color_noise::NoiseModel::Measured => 0.25,
        };
        self.effects.chroma_detail = d.chroma_detail;
        self.effects.chroma_smoothness = d.chroma_smoothness;
    }
    /// The measured Color noise reduction to run on the camera image; `None` for the
    /// original operator, which [`Recipe::sampled_noise_chroma`] applies at sampling.
    pub(crate) fn chroma_denoise(&self) -> Option<crate::develop::color_noise::ChromaDenoise> {
        if !self.measures_color_noise() {
            return None;
        }
        crate::develop::color_noise::ChromaDenoise::new(
            self.noise_chroma,
            self.effects.chroma_detail,
            self.effects.chroma_smoothness,
        )
    }
    /// The Color amount the original operator applies when sampling the camera image:
    /// none for the measured one, which runs on the camera image first.
    pub(crate) fn sampled_noise_chroma(&self) -> f32 {
        if self.measures_color_noise() {
            0.
        } else {
            self.noise_chroma
        }
    }
    /// Whether the measured Color noise reduction renders: engines before 3 develop
    /// without it and keep the original filter.
    fn measures_color_noise(&self) -> bool {
        !self.noise_model.is_original() && self.engine >= 3
    }
    /// The Sharpening sliders at their defaults for `model`, which the recipe then
    /// uses: Lightroom's for raw files (Amount 40, Radius 1.0, Detail 25, Masking 0)
    /// with the measured operator, RAWmakase's earlier ones with the original.
    pub fn set_sharpening_defaults(&mut self, model: crate::develop::sharpening::SharpeningModel) {
        let d = crate::develop::sharpening::SharpeningSliders::defaults(model);
        self.sharpening_model = model;
        self.sharpening = if self.engine >= 3 { d.amount } else { 0. };
        self.sharpening_radius = d.radius;
        self.sharpening_detail = d.detail;
        self.sharpening_masking = d.masking;
    }
    /// After an edit of Grain from Amount `previous`: grain added from none has nothing
    /// of the original operator's to keep, so it takes the measured one.
    pub fn adopt_measured_grain(&mut self, previous: f32) {
        if previous == 0. && self.effects.grain != 0. {
            self.grain_model = crate::develop::effects::GrainModel::Measured;
        }
    }
    /// After an edit of Clarity from `previous`: Clarity added from none has nothing
    /// of the original operator's to keep, so it takes the measured one.
    pub fn adopt_measured_clarity(&mut self, previous: f32) {
        if previous == 0. && self.effects.clarity != 0. {
            self.clarity_model = crate::develop::clarity::ClarityModel::Measured;
        }
    }
    /// After an edit of Texture from `previous`: Texture added from none has nothing
    /// of the original operator's to keep, so it takes the measured one.
    pub fn adopt_measured_texture(&mut self, previous: f32) {
        if previous == 0. && self.effects.texture != 0. {
            self.texture_model = crate::develop::texture::TextureModel::Measured;
        }
    }
    pub(crate) fn lens_correction<'a>(
        &self,
        m: &'a Metadata,
    ) -> Option<&'a crate::optics::LensCorrection> {
        if self.engine < 4 {
            return None;
        }
        self.lens_profile_in_use(m)
            .used
            .and_then(|c| c.correction(m))
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
