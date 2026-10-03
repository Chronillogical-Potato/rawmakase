//! Writes a Lightroom Develop preset: the chosen setting groups of a recipe as an XMP
//! file Lightroom and Camera Raw read, laid out as the presets Lightroom writes.
use super::{
    ns::CRS,
    write::{curve, settings},
    xml::{escape_text, xmpmeta},
};
use crate::develop::{
    Recipe,
    settings_groups::{GroupSelection, SettingGroup},
};
use std::fmt::Write;

/// A preset's name, group and identity.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetInfo {
    pub name: String,
    pub group: String,
    /// 32 hexadecimal digits, as Lightroom writes; kept when a preset is updated.
    pub uuid: String,
}

impl PresetInfo {
    /// A new preset with a fresh identity.
    pub fn new(name: &str, group: &str) -> Self {
        Self {
            name: name.trim().to_string(),
            group: group.trim().to_string(),
            uuid: new_uuid(),
        }
    }
}

/// 128 random bits as Lightroom's UUID text.
fn new_uuid() -> String {
    use std::hash::{BuildHasher, Hasher};
    let half = || {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.finish()
    };
    format!("{:016X}{:016X}", half(), half())
}

/// The preset for `r`'s settings in `groups`.
pub fn preset(r: &Recipe, info: &PresetInfo, groups: &GroupSelection) -> String {
    let mut attributes: Vec<(String, String)> = [
        ("PresetType", "Normal"),
        ("Cluster", ""),
        ("UUID", info.uuid.as_str()),
        ("SupportsAmount", "False"),
        ("SupportsColor", "True"),
        ("SupportsMonochrome", "True"),
        ("SupportsHighDynamicRange", "True"),
        ("SupportsNormalDynamicRange", "True"),
        ("SupportsSceneReferred", "True"),
        ("SupportsOutputReferred", "True"),
        ("CameraModelRestriction", ""),
        ("Copyright", ""),
        ("ContactInfo", ""),
        ("Version", "15.4"),
        ("RAWmakasePreset", "1"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    attributes.extend(
        settings(r, None)
            .0
            .into_iter()
            .filter(|(key, _)| group_of_key(key).is_some_and(|g| groups.contains(g))),
    );
    // Each chosen panel's switch, on or off, so applying the preset also turns its
    // panel back on where a photo had it off.
    for panel in crate::develop::panels::Panel::ALL {
        let key = panel.lightroom_keys()[0];
        if groups.groups().any(|g| g.panel() == Some(panel))
            && !attributes.iter().any(|(k, _)| k == key)
        {
            attributes.push((key.to_string(), "True".into()));
        }
    }
    // Written once, after the settings, as Lightroom does.
    attributes.push(("HasSettings".into(), "True".into()));
    let mut out = format!("  <rdf:Description rdf:about=\"\"\n    xmlns:crs=\"{CRS}\"");
    for (key, value) in &attributes {
        let _ = write!(out, "\n   crs:{key}=\"{}\"", escape_text(value));
    }
    out.push_str(">\n");
    for (element, text) in [
        ("Name", info.name.as_str()),
        ("ShortName", ""),
        ("SortName", ""),
        ("Group", info.group.as_str()),
        ("Description", ""),
    ] {
        let _ = write!(
            out,
            "   <crs:{element}>\n    <rdf:Alt>\n     <rdf:li xml:lang=\"x-default\">{}</rdf:li>\n    </rdf:Alt>\n   </crs:{element}>\n",
            escape_text(text)
        );
    }
    if groups.contains(SettingGroup::ToneCurve) {
        curve(&mut out, "ToneCurvePV2012", &r.curve);
        for (i, name) in ["Red", "Green", "Blue"].iter().enumerate() {
            curve(
                &mut out,
                &format!("ToneCurvePV2012{name}"),
                &r.effects.channels[i],
            );
        }
    }
    out.push_str("  </rdf:Description>\n");
    xmpmeta(&out)
}

/// The setting group a Camera Raw key belongs to, as Lightroom's New Develop Preset
/// dialog groups them.
pub(crate) fn group_of_key(key: &str) -> Option<SettingGroup> {
    use SettingGroup::*;
    let starts = |prefix: &str| key.starts_with(prefix);
    Some(match key {
        "WhiteBalance" | "Temperature" | "Tint" => WhiteBalance,
        "Exposure2012" => Exposure,
        "Contrast2012" => Contrast,
        "Highlights2012" => Highlights,
        "Shadows2012" => Shadows,
        "Whites2012" => Whites,
        "Blacks2012" => Blacks,
        "Texture" => Texture,
        "Clarity2012" => Clarity,
        "Dehaze" => Dehaze,
        "Vibrance" => Vibrance,
        "Saturation" => Saturation,
        "CameraProfile" | "ConvertToGrayscale" => TreatmentAndProfile,
        "ProcessVersion" => ProcessVersion,
        "Sharpness" | "EnableDetail" => Sharpening,
        "LuminanceSmoothing" => LuminanceNoiseReduction,
        "ColorNoiseReduction" => ColorNoiseReduction,
        "AutoLateralCA" => ChromaticAberration,
        "VignetteAmount" | "VignetteMidpoint" => LensVignetting,
        "PerspectiveUpright" => UprightMode,
        "EnableToneCurve" | "CurveRefineSaturation" => ToneCurve,
        "LensManualDistortionAmount" => LensProfileCorrections,
        "EnableColorAdjustments" => ColorAdjustments,
        "EnableGrayscaleMix" => BlackWhiteMix,
        "EnableSplitToning" => ColorGrading,
        "EnableLensCorrections" => LensProfileCorrections,
        "EnableTransform" => TransformAdjustments,
        "EnableEffects" => PostCropVignetting,
        "EnableCalibration" | "ShadowTint" => Calibration,
        "EnableRetouch" => SpotRemoval,
        "CropLeft"
        | "CropTop"
        | "CropRight"
        | "CropBottom"
        | "CropAngle"
        | "HasCrop"
        | "CropConstrainToWarp" => Crop,
        _ if starts("Parametric") || starts("ToneCurve") => ToneCurve,
        _ if starts("HueAdjustment")
            || starts("SaturationAdjustment")
            || starts("LuminanceAdjustment") =>
        {
            ColorAdjustments
        }
        _ if starts("GrayMixer") => BlackWhiteMix,
        _ if starts("SplitToning") || starts("ColorGrade") => ColorGrading,
        _ if starts("Sharpen") => Sharpening,
        _ if starts("LuminanceNoiseReduction") => LuminanceNoiseReduction,
        _ if starts("ColorNoiseReduction") => ColorNoiseReduction,
        _ if starts("LensProfile") => LensProfileCorrections,
        _ if starts("Defringe") => ChromaticAberration,
        _ if starts("Upright") => UprightMode,
        _ if starts("Perspective") => TransformAdjustments,
        _ if starts("PostCropVignette") => PostCropVignetting,
        _ if starts("Grain") => Grain,
        _ if matches!(
            key,
            "RedHue"
                | "RedSaturation"
                | "GreenHue"
                | "GreenSaturation"
                | "BlueHue"
                | "BlueSaturation"
        ) =>
        {
            Calibration
        }
        _ if key.starts_with("Enable") && key.ends_with("Corrections") => Masking,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::settings_groups::GroupInclusion;
    use std::path::Path;

    /// A recipe whose every written setting differs from the default.
    fn edited() -> Recipe {
        let mut r = Recipe {
            exposure: 0.5,
            contrast: 0.2,
            temperature: 4800.,
            sharpening: 0.5,
            ..Default::default()
        };
        r.effects.monochrome = true;
        r.effects.vignette = -0.3;
        r.effects.grain = 0.2;
        r.curve.points = vec![[0., 0.1], [1., 1.]];
        r.panels.set(
            crate::develop::panels::Panel::Effects,
            crate::develop::panels::PanelState::Off,
        );
        r
    }

    #[test]
    fn every_key_written_belongs_to_a_group() {
        let unplaced: Vec<_> = settings(&edited(), None)
            .0
            .into_iter()
            .map(|(k, _)| k)
            .filter(|k| k != "HasSettings" && group_of_key(k).is_none())
            .collect();
        assert!(unplaced.is_empty(), "{unplaced:?}");
    }

    #[test]
    fn a_preset_holds_only_its_groups_and_reads_back_as_written() -> anyhow::Result<()> {
        let mut groups = GroupSelection::none();
        groups.set(SettingGroup::Exposure, GroupInclusion::Included);
        groups.set(SettingGroup::ToneCurve, GroupInclusion::Included);
        let info = PresetInfo::new("Bright & airy", "User Presets");
        assert_eq!(info.uuid.len(), 32);
        let text = preset(&edited(), &info, &groups);
        let parsed = crate::xmp::parse(Path::new("Bright.xmp"), &text)?;
        assert_eq!(parsed.name, "Bright & airy");
        assert_eq!(parsed.group, "User Presets");
        assert_eq!(parsed.settings["UUID"], info.uuid);
        assert!(parsed.settings.contains_key("Exposure2012"));
        for key in [
            "Contrast2012",
            "Temperature",
            "ConvertToGrayscale",
            "Sharpness",
        ] {
            assert!(!parsed.settings.contains_key(key), "{key}");
        }
        let m = crate::raw::Metadata {
            wb: [2., 1., 1.8],
            daylight_wb: [2., 1., 1.8],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        };
        let applied = parsed.apply(&Recipe::default(), &m, &[], None)?;
        assert_eq!(applied.exposure, 0.5);
        assert_eq!(applied.contrast, 0.);
        assert_eq!(applied.curve.points.len(), 2);
        // Every group: the whole edit comes back.
        let all = crate::xmp::parse(
            Path::new("All.xmp"),
            &preset(&edited(), &info, &GroupSelection::all()),
        )?
        .apply(&Recipe::default(), &m, &[], None)?;
        assert!(all.effects.monochrome);
        // A chosen panel's switch is written either way, so applying turns it on.
        let mut grain = GroupSelection::none();
        grain.set(SettingGroup::Grain, GroupInclusion::Included);
        let text = preset(&Recipe::default(), &info, &grain);
        assert!(text.contains(r#"crs:EnableEffects="True""#), "{text}");
        assert!(!text.contains("crs:EnableDetail"));
        assert_eq!(
            all.panels.state(crate::develop::panels::Panel::Effects),
            crate::develop::panels::PanelState::Off
        );
        Ok(())
    }
}
