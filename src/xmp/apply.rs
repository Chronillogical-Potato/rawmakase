use super::Preset;
use crate::{
    camera_profiles::CameraProfile,
    develop::{Recipe, mul},
    raw::{CameraImage, Metadata},
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
fn number(values: &BTreeMap<String, String>, key: &str) -> Result<Option<f32>> {
    values
        .get(key)
        .map(|s| {
            let v = s
                .parse::<f32>()
                .with_context(|| format!("Invalid {key}: {s}"))?;
            ensure!(v.is_finite(), "Non-finite {key}");
            Ok(v)
        })
        .transpose()
}
fn boolean(values: &BTreeMap<String, String>, key: &str) -> Result<Option<bool>> {
    values
        .get(key)
        // Lightroom writes some flags empty (e.g. ConvertToGrayscale=""),
        // meaning not set.
        .filter(|s| !s.trim().is_empty())
        .map(|s| match s.to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            _ => anyhow::bail!("Invalid boolean {key}"),
        })
        .transpose()
}
struct Settings<'a> {
    values: &'a BTreeMap<String, String>,
    seen: BTreeSet<String>,
}
impl Settings<'_> {
    fn assign(&mut self, key: &str, out: &mut f32, scale: f32, lo: f32, hi: f32) -> Result<()> {
        self.seen.insert(key.to_string());
        if let Some(value) = number(self.values, key)? {
            let value = value * scale;
            ensure!(
                (lo..=hi).contains(&value),
                "{key} is outside supported range"
            );
            *out = value;
        }
        Ok(())
    }
}
const METADATA: &[&str] = &[
    "Version",
    "ProcessVersion",
    "HasSettings",
    "HasCrop",
    "PresetType",
    "Cluster",
    "UUID",
    "SupportsAmount",
    "SupportsAmount2",
    "SupportsColor",
    "SupportsMonochrome",
    "SupportsHighDynamicRange",
    "SupportsNormalDynamicRange",
    "SupportsSceneReferred",
    "SupportsOutputReferred",
    "CameraModelRestriction",
    "Copyright",
    "ContactInfo",
    "Name",
    "ShortName",
    "SortName",
    "Description",
    "Group",
    "ToneCurveName",
    "ToneCurveName2012",
    "CameraProfileDigest",
    "ShowInPresets",
    "ShowInQuickActions",
    "OverrideLookVignette",
    "CompatibleVersion",
    "AlreadyApplied",
    "RawFileName",
];
impl Preset {
    /// Apply to a private recipe, publishing only after every stage validates.
    pub fn apply(
        &self,
        base: &Recipe,
        m: &Metadata,
        profiles: &[Arc<CameraProfile>],
        image: Option<&CameraImage>,
    ) -> Result<Recipe> {
        ensure!(self.blockers.is_empty(), "{}", self.blockers.join("; "));
        let mut settings = Settings {
            values: &self.settings,
            seen: METADATA.iter().map(|key| key.to_string()).collect(),
        };
        let mut recipe = base.clone();
        recipe.engine = recipe.engine.max(3);
        recipe.profile_tone = true;
        recipe.reference_color = true;
        self.apply_profile(&mut settings, &mut recipe, m, profiles)?;
        self.apply_basic(&mut settings, &mut recipe)?;
        self.apply_white_balance(&mut settings, &mut recipe, m, image)?;
        self.apply_color_mixer(&mut settings, &mut recipe)?;
        self.apply_curves(&mut settings, &mut recipe)?;
        self.apply_grading(&mut settings, &mut recipe)?;
        self.apply_effects(&mut settings, &mut recipe)?;
        self.apply_auto_tone_and_crop(&mut settings, &mut recipe, m, image)?;
        self.validate_remaining(&mut settings)?;
        recipe.preset_name = self.name.clone();
        recipe.preset_settings = self.settings.clone();
        recipe.validate()?;
        Ok(recipe)
    }
    /// Lightroom-style best effort: when `apply` rejects the preset (missing
    /// profile, camera restriction, unsupported settings), apply every stage
    /// that validates, keep the base values for stages that don't, and return
    /// what was skipped. Fails only if the combined recipe is invalid.
    pub fn apply_lenient(
        &self,
        base: &Recipe,
        m: &Metadata,
        profiles: &[Arc<CameraProfile>],
        image: Option<&CameraImage>,
    ) -> Result<(Recipe, Vec<String>)> {
        if let Ok(recipe) = self.apply(base, m, profiles, image) {
            return Ok((recipe, Vec::new()));
        }
        let mut warnings = self.blockers.clone();
        let mut settings = Settings {
            values: &self.settings,
            seen: METADATA.iter().map(|key| key.to_string()).collect(),
        };
        let mut recipe = base.clone();
        recipe.engine = recipe.engine.max(3);
        recipe.profile_tone = true;
        recipe.reference_color = true;
        let mut stage = |recipe: &mut Recipe, result: &dyn Fn(&mut Recipe) -> Result<()>| {
            let backup = recipe.clone();
            if let Err(e) = result(recipe) {
                *recipe = backup;
                warnings.push(format!("{e:#}"));
            }
        };
        let settings = std::cell::RefCell::new(&mut settings);
        stage(&mut recipe, &|r| {
            self.apply_profile(&mut settings.borrow_mut(), r, m, profiles)
        });
        stage(&mut recipe, &|r| {
            self.apply_basic(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|r| {
            self.apply_white_balance(&mut settings.borrow_mut(), r, m, image)
        });
        stage(&mut recipe, &|r| {
            self.apply_color_mixer(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|r| {
            self.apply_curves(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|r| {
            self.apply_grading(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|r| {
            self.apply_effects(&mut settings.borrow_mut(), r)
        });
        stage(&mut recipe, &|r| {
            self.apply_auto_tone_and_crop(&mut settings.borrow_mut(), r, m, image)
        });
        stage(&mut recipe, &|_| {
            self.validate_remaining(&mut settings.borrow_mut())
        });
        recipe.preset_name = self.name.clone();
        recipe.preset_settings = self.settings.clone();
        recipe.validate()?;
        Ok((recipe, warnings))
    }
    fn apply_profile(
        &self,
        settings: &mut Settings<'_>,
        r: &mut Recipe,
        m: &Metadata,
        profiles: &[Arc<CameraProfile>],
    ) -> Result<()> {
        let v = settings.values;
        if let Some(model) = v.get("CameraModelRestriction").filter(|s| !s.is_empty()) {
            ensure!(
                model.eq_ignore_ascii_case(&m.model)
                    || model.eq_ignore_ascii_case(&format!("{} {}", m.make, m.model)),
                "Preset is restricted to {model}"
            );
        }
        settings.seen.insert("RequiresRGBTables".into());
        ensure!(
            boolean(v, "RequiresRGBTables")? != Some(true),
            "Preset requires external RGB tables"
        );
        settings.seen.insert("CameraProfile".into());
        if let Some(name) = v.get("CameraProfile") {
            let name = if name == "Default Profile" {
                "Adobe Standard"
            } else {
                name.as_str()
            };
            let profile = profiles
                .iter()
                .find(|p| p.name == name && p.ensure_camera(m).is_ok());
            r.profile = Some(
                profile
                    .with_context(|| format!("Missing camera profile ‘{name}’ for {}", m.model))?
                    .clone(),
            );
        }
        settings.seen.insert("RAWmakaseLookUUID".into());
        if !self.look.is_empty() {
            let profile = profiles.iter().find(|p| {
                p.name == self.look
                    && p.ensure_camera(m).is_ok()
                    && p.enhanced.as_ref().is_some_and(|look| {
                        v.get("RAWmakaseLookUUID")
                            .is_none_or(|uuid| look.uuid.eq_ignore_ascii_case(uuid))
                    })
            });
            r.profile = Some(
                profile
                    .with_context(|| {
                        format!(
                            "Missing or unsupported enhanced profile ‘{}’ for {}",
                            self.look, m.model
                        )
                    })?
                    .clone(),
            );
            r.reference_curves = true;
            r.wide_gamut_curves = true;
        }
        if r.profile.as_ref().is_some_and(|p| p.enhanced.is_some()) {
            r.reference_curves = true;
            r.wide_gamut_curves = true;
        }
        if v.contains_key("CameraProfile") || !self.look.is_empty() {
            r.use_camera_baseline(m);
        }
        if !self.curves.is_empty() {
            r.wide_gamut_curves = true;
            r.reference_curves = true;
        }
        Ok(())
    }

    fn apply_basic(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        settings.assign("Exposure2012", &mut r.exposure, 1., -8., 8.)?;
        settings.assign("Contrast2012", &mut r.contrast, 0.01, -1., 1.)?;
        settings.assign("Highlights2012", &mut r.highlights, 0.01, -1., 1.)?;
        settings.assign("Shadows2012", &mut r.shadows, 0.01, -1., 1.)?;
        settings.assign("Whites2012", &mut r.whites, 0.01, -1., 1.)?;
        settings.assign("Blacks2012", &mut r.blacks, 0.01, -1., 1.)?;
        settings.assign("Saturation", &mut r.saturation, 0.01, -1., 1.)?;
        settings.assign("Vibrance", &mut r.vibrance, 0.01, -1., 1.)?;
        settings.assign("Sharpness", &mut r.sharpening, 1. / 150., 0., 1.)?;
        settings.assign("SharpenRadius", &mut r.sharpening_radius, 1., 0.5, 3.)?;
        settings.assign("SharpenDetail", &mut r.sharpening_detail, 0.01, 0., 1.)?;
        settings.assign(
            "SharpenEdgeMasking",
            &mut r.sharpening_masking,
            0.01,
            0.,
            1.,
        )?;
        settings.assign("LuminanceSmoothing", &mut r.noise_luma, 0.01, 0., 1.)?;
        settings.assign("ColorNoiseReduction", &mut r.noise_chroma, 0.01, 0., 1.)?;
        Ok(())
    }

    fn apply_white_balance(
        &self,
        settings: &mut Settings<'_>,
        r: &mut Recipe,
        m: &Metadata,
        image: Option<&CameraImage>,
    ) -> Result<()> {
        let v = settings.values;
        settings.seen.insert("WhiteBalance".into());
        settings.seen.insert("Temperature".into());
        settings.seen.insert("Tint".into());
        match v.get("WhiteBalance").map(String::as_str) {
            Some("As Shot") => {
                r.reset_white_balance(m);
                // Photo sidecars/catalogs contain resolved WB numbers, even when
                // the provenance label remains As Shot. Presets retain camera-relative semantics.
                if self.photo_settings
                    && let (Some(temperature), Some(tint)) =
                        (number(v, "Temperature")?, number(v, "Tint")?)
                {
                    r.temperature = temperature;
                    r.tint = tint;
                    r.update_wb(m);
                }
            }
            Some("Auto") => {
                if self.photo_settings
                    && let (Some(temperature), Some(tint)) =
                        (number(v, "Temperature")?, number(v, "Tint")?)
                {
                    r.temperature = temperature;
                    r.tint = tint;
                    r.update_wb(m);
                } else if let Some(im) = image {
                    let mut avg = [0.; 3];
                    let mut count = 0.;
                    for p in im.pixels.iter().step_by(16) {
                        if p.iter().all(|v| *v > 0.005 && *v < 0.9) {
                            for c in 0..3 {
                                avg[c] += p[c];
                            }
                            count += 1.;
                        }
                    }
                    ensure!(count > 0., "No usable pixels for automatic white balance");
                    r.wb = std::array::from_fn(|c| (avg[1] / avg[c].max(1e-8)).clamp(0.01, 100.));
                    r.tint = 0.;
                    r.sync_white_balance_controls(m);
                }
            }
            Some("Custom") | None => {
                let mut changed = false;
                if let Some(t) = number(v, "Temperature")? {
                    r.temperature = t;
                    changed = true;
                }
                if let Some(t) = number(v, "Tint")? {
                    r.tint = t;
                    changed = true;
                }
                if changed {
                    r.update_wb(m);
                }
            }
            Some(other) => anyhow::bail!("Unsupported white balance mode: {other}"),
        }
        Ok(())
    }

    fn apply_color_mixer(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        let v = settings.values;
        let bands = [
            "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
        ];
        for (i, band) in bands.iter().enumerate() {
            for (j, control) in ["Hue", "Saturation", "Luminance"].iter().enumerate() {
                settings.assign(
                    &format!("{control}Adjustment{band}"),
                    &mut r.hsl[i][j],
                    0.01,
                    -1.,
                    1.,
                )?;
            }
            settings.assign(
                &format!("GrayMixer{band}"),
                &mut r.effects.gray_mix[i],
                0.01,
                -1.,
                1.,
            )?;
        }
        for (i, band) in ["Red", "Green", "Blue"].iter().enumerate() {
            settings.assign(
                &format!("{band}Hue"),
                &mut r.effects.calibration[i][0],
                0.01,
                -1.,
                1.,
            )?;
            settings.assign(
                &format!("{band}Saturation"),
                &mut r.effects.calibration[i][1],
                0.01,
                -1.,
                1.,
            )?;
        }
        settings.assign("ShadowTint", &mut r.effects.shadow_tint, 0.01, -1., 1.)?;
        if [
            "RedHue",
            "RedSaturation",
            "GreenHue",
            "GreenSaturation",
            "BlueHue",
            "BlueSaturation",
            "ShadowTint",
        ]
        .iter()
        .any(|key| v.contains_key(*key))
        {
            r.reference_calibration = true;
        }
        Ok(())
    }

    fn apply_curves(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        let v = settings.values;
        for (i, name) in ["Shadows", "Darks", "Lights", "Highlights"]
            .iter()
            .enumerate()
        {
            settings.assign(
                &format!("Parametric{name}"),
                &mut r.effects.parametric[i],
                0.01,
                -1.,
                1.,
            )?;
        }
        for (i, name) in ["Shadow", "Midtone", "Highlight"].iter().enumerate() {
            settings.assign(
                &format!("Parametric{name}Split"),
                &mut r.effects.splits[i],
                0.01,
                0.01,
                0.99,
            )?;
        }
        if let Some(c) = self
            .curves
            .get("ToneCurvePV2012")
            .or_else(|| self.curves.get("ToneCurve"))
        {
            r.curve = c.clone();
        }
        for (i, name) in ["Red", "Green", "Blue"].iter().enumerate() {
            if let Some(c) = self
                .curves
                .get(&format!("ToneCurvePV2012{name}"))
                .or_else(|| self.curves.get(&format!("ToneCurve{name}")))
            {
                r.effects.channels[i] = c.clone();
            }
        }
        settings.seen.insert("ConvertToGrayscale".into());
        if let Some(b) = boolean(v, "ConvertToGrayscale")? {
            r.effects.monochrome = b;
        }
        Ok(())
    }

    fn apply_grading(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        let v = settings.values;
        for (i, name) in [(0, "Shadow"), (2, "Highlight")] {
            settings.assign(
                &format!("SplitToning{name}Hue"),
                &mut r.grading[i][0],
                1. / 360.,
                0.,
                1.,
            )?;
            settings.assign(
                &format!("SplitToning{name}Saturation"),
                &mut r.grading[i][1],
                0.01,
                0.,
                1.,
            )?;
            settings.assign(
                &format!("ColorGrade{name}Lum"),
                &mut r.grading[i][2],
                0.01,
                -1.,
                1.,
            )?;
        }
        settings.assign(
            "ColorGradeMidtoneHue",
            &mut r.grading[1][0],
            1. / 360.,
            0.,
            1.,
        )?;
        settings.assign("ColorGradeMidtoneSat", &mut r.grading[1][1], 0.01, 0., 1.)?;
        settings.assign("ColorGradeMidtoneLum", &mut r.grading[1][2], 0.01, -1., 1.)?;
        settings.assign("SplitToningBalance", &mut r.effects.balance, 0.01, -1., 1.)?;
        // Adobe's pre-Color-Grading split toning used full tonal overlap.
        // Only split-tone records without any modern grading keys imply that mode.
        if v.keys().any(|key| key.starts_with("SplitToning"))
            && !v.keys().any(|key| key.starts_with("ColorGrade"))
        {
            r.effects.blending = 1.;
            r.grading[1] = [0.; 3];
            r.grading[0][2] = 0.;
            r.grading[2][2] = 0.;
            r.effects.global_grade = [0.; 3];
        }
        settings.assign("ColorGradeBlending", &mut r.effects.blending, 0.01, 0., 1.)?;
        for (i, (name, scale)) in [("Hue", 1. / 360.), ("Sat", 0.01), ("Lum", 0.01)]
            .iter()
            .enumerate()
        {
            settings.assign(
                &format!("ColorGradeGlobal{name}"),
                &mut r.effects.global_grade[i],
                *scale,
                if i == 2 { -1. } else { 0. },
                1.,
            )?;
        }
        Ok(())
    }

    fn apply_effects(&self, settings: &mut Settings<'_>, r: &mut Recipe) -> Result<()> {
        let v = settings.values;
        settings.assign("Clarity2012", &mut r.effects.clarity, 0.01, -1., 1.)?;
        settings.assign("Texture", &mut r.effects.texture, 0.01, -1., 1.)?;
        settings.assign("Dehaze", &mut r.effects.dehaze, 0.01, -1., 1.)?;
        settings.assign("GrainAmount", &mut r.effects.grain, 0.01, 0., 1.)?;
        settings.assign("GrainSize", &mut r.effects.grain_size, 0.01, 0., 1.)?;
        settings.assign(
            "GrainFrequency",
            &mut r.effects.grain_roughness,
            0.01,
            0.,
            1.,
        )?;
        settings.seen.insert("GrainSeed".into());
        if let Some(seed) = v.get("GrainSeed") {
            r.effects.grain_seed = seed.parse().context("Invalid grain seed")?;
        }
        settings.assign(
            "PostCropVignetteAmount",
            &mut r.effects.vignette,
            0.01,
            -1.,
            1.,
        )?;
        settings.assign(
            "PostCropVignetteMidpoint",
            &mut r.effects.vignette_midpoint,
            0.01,
            0.,
            1.,
        )?;
        settings.assign(
            "PostCropVignetteRoundness",
            &mut r.effects.vignette_roundness,
            0.01,
            -1.,
            1.,
        )?;
        settings.assign(
            "PostCropVignetteFeather",
            &mut r.effects.vignette_feather,
            0.01,
            0.,
            1.,
        )?;
        settings.assign(
            "PostCropVignetteHighlightContrast",
            &mut r.effects.vignette_highlights,
            0.01,
            0.,
            1.,
        )?;
        settings.seen.insert("PostCropVignetteStyle".into());
        if let Some(style) = v.get("PostCropVignetteStyle") {
            r.effects.vignette_style = style.parse()?;
        }
        settings.assign(
            "VignetteAmount",
            &mut r.effects.lens_vignette,
            0.01,
            -1.,
            1.,
        )?;
        settings.assign(
            "VignetteMidpoint",
            &mut r.effects.lens_vignette_midpoint,
            0.01,
            0.,
            1.,
        )?;
        for (i, name) in ["Purple", "Green"].iter().enumerate() {
            settings.assign(
                &format!("Defringe{name}Amount"),
                &mut r.effects.defringe[i],
                0.05,
                0.,
                1.,
            )?;
            settings.assign(
                &format!("Defringe{name}HueLo"),
                &mut r.effects.defringe_ranges[i][0],
                0.01,
                0.,
                1.,
            )?;
            settings.assign(
                &format!("Defringe{name}HueHi"),
                &mut r.effects.defringe_ranges[i][1],
                0.01,
                0.,
                1.,
            )?;
        }
        settings.assign(
            "LuminanceNoiseReductionDetail",
            &mut r.effects.luma_detail,
            0.01,
            0.,
            1.,
        )?;
        settings.assign(
            "LuminanceNoiseReductionContrast",
            &mut r.effects.luma_contrast,
            0.01,
            0.,
            1.,
        )?;
        settings.assign(
            "ColorNoiseReductionDetail",
            &mut r.effects.chroma_detail,
            0.01,
            0.,
            1.,
        )?;
        settings.assign(
            "ColorNoiseReductionSmoothness",
            &mut r.effects.chroma_smoothness,
            0.01,
            0.,
            1.,
        )?;
        Ok(())
    }

    fn apply_auto_tone_and_crop(
        &self,
        settings: &mut Settings<'_>,
        r: &mut Recipe,
        m: &Metadata,
        image: Option<&CameraImage>,
    ) -> Result<()> {
        let v = settings.values;
        settings.seen.insert("AutoTone".into());
        // Lightroom stores the exposure Auto Tone chose; recompute only when it is absent,
        // so every route (open, reset, history, hover) renders the same edit.
        if boolean(v, "AutoTone")? == Some(true)
            && !v.contains_key("Exposure2012")
            && let Some(im) = image
        {
            let mut l: Vec<f32> = im
                .pixels
                .iter()
                .step_by(64)
                .map(|p| {
                    let q = mul(m.matrix, std::array::from_fn(|c| p[c] * r.wb[c]));
                    (q[0] * 0.2126 + q[1] * 0.7152 + q[2] * 0.0722).max(1e-6)
                })
                .collect();
            l.sort_by(f32::total_cmp);
            if !l.is_empty() {
                r.exposure = (0.18 / l[l.len() / 2]).log2().clamp(-8., 8.);
            }
        }
        settings.assign("CropAngle", &mut r.straighten, 1., -45., 45.)?;
        settings.seen.insert("LensProfileEnable".into());
        // Which Adobe profile Lightroom chose; RAWmakase matches imported profiles itself.
        for key in [
            "LensProfileName",
            "LensProfileFilename",
            "LensProfileDigest",
            "LensProfileIsEmbedded",
        ] {
            settings.seen.insert(key.into());
        }
        settings.assign(
            "LensProfileDistortionScale",
            &mut r.lens_distortion,
            0.01,
            0.,
            2.,
        )?;
        settings.assign(
            "LensProfileVignettingScale",
            &mut r.lens_vignetting,
            0.01,
            0.,
            2.,
        )?;
        if let Some(enable) = number(v, "LensProfileEnable")? {
            r.lens_profile = enable != 0.;
        }
        let t = &mut r.transform;
        settings.assign("PerspectiveVertical", &mut t.vertical, 0.01, -1., 1.)?;
        settings.assign("PerspectiveHorizontal", &mut t.horizontal, 0.01, -1., 1.)?;
        settings.assign("PerspectiveRotate", &mut t.rotate, 1., -10., 10.)?;
        settings.assign("PerspectiveAspect", &mut t.aspect, 0.01, -1., 1.)?;
        settings.assign("PerspectiveScale", &mut t.scale, 0.01, 0.5, 1.5)?;
        settings.assign("PerspectiveX", &mut t.offset_x, 0.01, -1., 1.)?;
        settings.assign("PerspectiveY", &mut t.offset_y, 0.01, -1., 1.)?;
        for (i, name) in ["Left", "Top", "Right", "Bottom"].iter().enumerate() {
            settings.assign(&format!("Crop{name}"), &mut r.crop[i], 1., 0., 1.)?;
        }
        Ok(())
    }

    fn validate_remaining(&self, settings: &mut Settings<'_>) -> Result<()> {
        let v = settings.values;
        // No-op geometry/default flags are safe; active unsupported operations are explicit blockers.
        for (key, default) in [
            ("HDREditMode", "0"),
            ("CurveRefineSaturation", "100"),
            ("AutoLateralCA", "0"),
            ("LensManualDistortionAmount", "0"),
            ("PerspectiveUpright", "0"),
            ("CropConstrainToWarp", "0"),
            ("IncrementalTemperature", "0"),
            ("IncrementalTint", "0"),
            // Camera Raw 18.6 effects without rendering support yet.
            ("Glow", "0"),
            ("ReshapeAmount", "0"),
        ] {
            settings.seen.insert(key.into());
            if let Some(value) = v.get(key) {
                ensure!(
                    value.parse::<f32>().ok() == default.parse::<f32>().ok(),
                    "Active {key} requires rendering support not yet available"
                );
            }
        }
        settings.seen.insert("LensProfileSetup".into());
        // Lightroom 15 records whether the crop is kept inside the image; it only
        // constrains the crop tool and does not change rendering.
        settings.seen.insert("CropConstrainToUnitSquare".into());
        let unknown: Vec<_> = v
            .keys()
            .filter(|k| !settings.seen.contains(*k))
            .cloned()
            .collect();
        ensure!(
            unknown.is_empty(),
            "Unsupported preset settings: {}",
            unknown.join(", ")
        );
        Ok(())
    }
}
