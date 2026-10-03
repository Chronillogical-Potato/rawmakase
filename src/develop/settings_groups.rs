//! The settings Copy, Paste and Sync transfer between photos, grouped as Lightroom's
//! Copy Settings dialog lists them.
//!
//! Every recipe setting is one of three kinds. Most belong to a group and transfer as
//! the user set them. Some are worked out for the photo they apply to (white balance
//! gains, the camera's exposure baseline, Upright's analysis, which camera profile
//! file a profile name means). The rest are the photo's own and never transfer: its
//! orientation, the preset it came from, and settings from a newer release.
use super::{Recipe, panels::Panel};
use crate::{camera_profiles::CameraProfile, raw::Metadata};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};

/// A setting group, the smallest unit Copy Settings can choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingGroup {
    WhiteBalance,
    Exposure,
    Contrast,
    Highlights,
    Shadows,
    Whites,
    Blacks,
    Texture,
    Clarity,
    Dehaze,
    Vibrance,
    Saturation,
    TreatmentAndProfile,
    ToneCurve,
    ColorAdjustments,
    BlackWhiteMix,
    ColorGrading,
    Sharpening,
    LuminanceNoiseReduction,
    ColorNoiseReduction,
    LensProfileCorrections,
    ChromaticAberration,
    LensVignetting,
    UprightMode,
    TransformAdjustments,
    PostCropVignetting,
    Grain,
    ProcessVersion,
    Calibration,
    SpotRemoval,
    Crop,
    Masking,
}

/// A heading in Copy Settings, with the groups under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: &'static str,
    pub groups: &'static [SettingGroup],
}

impl SettingGroup {
    pub const ALL: [SettingGroup; 32] = [
        SettingGroup::WhiteBalance,
        SettingGroup::Exposure,
        SettingGroup::Contrast,
        SettingGroup::Highlights,
        SettingGroup::Shadows,
        SettingGroup::Whites,
        SettingGroup::Blacks,
        SettingGroup::Texture,
        SettingGroup::Clarity,
        SettingGroup::Dehaze,
        SettingGroup::Vibrance,
        SettingGroup::Saturation,
        SettingGroup::TreatmentAndProfile,
        SettingGroup::ToneCurve,
        SettingGroup::ColorAdjustments,
        SettingGroup::BlackWhiteMix,
        SettingGroup::ColorGrading,
        SettingGroup::Sharpening,
        SettingGroup::LuminanceNoiseReduction,
        SettingGroup::ColorNoiseReduction,
        SettingGroup::LensProfileCorrections,
        SettingGroup::ChromaticAberration,
        SettingGroup::LensVignetting,
        SettingGroup::UprightMode,
        SettingGroup::TransformAdjustments,
        SettingGroup::PostCropVignetting,
        SettingGroup::Grain,
        SettingGroup::ProcessVersion,
        SettingGroup::Calibration,
        SettingGroup::SpotRemoval,
        SettingGroup::Crop,
        SettingGroup::Masking,
    ];
    /// Copy Settings' sections, in Lightroom's order.
    pub const SECTIONS: [Section; 13] = {
        use SettingGroup::*;
        [
            Section {
                title: "White Balance",
                groups: &[WhiteBalance],
            },
            Section {
                title: "Basic Tone",
                groups: &[Exposure, Contrast, Highlights, Shadows, Whites, Blacks],
            },
            Section {
                title: "Presence",
                groups: &[Texture, Clarity, Dehaze, Vibrance, Saturation],
            },
            Section {
                title: "Treatment & Profile",
                groups: &[TreatmentAndProfile],
            },
            Section {
                title: "Tone Curve",
                groups: &[ToneCurve],
            },
            Section {
                title: "Color",
                groups: &[ColorAdjustments, BlackWhiteMix, ColorGrading],
            },
            Section {
                title: "Detail",
                groups: &[Sharpening, LuminanceNoiseReduction, ColorNoiseReduction],
            },
            Section {
                title: "Lens Corrections",
                groups: &[LensProfileCorrections, ChromaticAberration, LensVignetting],
            },
            Section {
                title: "Transform",
                groups: &[UprightMode, TransformAdjustments],
            },
            Section {
                title: "Effects",
                groups: &[PostCropVignetting, Grain],
            },
            Section {
                title: "Process & Calibration",
                groups: &[ProcessVersion, Calibration],
            },
            Section {
                title: "Crop",
                groups: &[Crop],
            },
            Section {
                title: "Spot Removal & Masking",
                groups: &[SpotRemoval, Masking],
            },
        ]
    };
    /// The checkbox label, as in Lightroom.
    pub fn label(self) -> &'static str {
        use SettingGroup::*;
        match self {
            WhiteBalance => "White Balance",
            Exposure => "Exposure",
            Contrast => "Contrast",
            Highlights => "Highlights",
            Shadows => "Shadows",
            Whites => "White Clipping",
            Blacks => "Black Clipping",
            Texture => "Texture",
            Clarity => "Clarity",
            Dehaze => "Dehaze",
            Vibrance => "Vibrance",
            Saturation => "Saturation",
            TreatmentAndProfile => "Treatment & Profile",
            ToneCurve => "Tone Curve",
            ColorAdjustments => "Color Adjustments",
            BlackWhiteMix => "Black & White Mix",
            ColorGrading => "Color Grading",
            Sharpening => "Sharpening",
            LuminanceNoiseReduction => "Luminance Noise Reduction",
            ColorNoiseReduction => "Color Noise Reduction",
            LensProfileCorrections => "Lens Profile Corrections",
            ChromaticAberration => "Chromatic Aberration",
            LensVignetting => "Lens Vignetting",
            UprightMode => "Upright Mode",
            TransformAdjustments => "Transform Adjustments",
            PostCropVignetting => "Post-Crop Vignetting",
            Grain => "Grain",
            ProcessVersion => "Process Version",
            Calibration => "Calibration",
            SpotRemoval => "Spot Removal",
            Crop => "Crop",
            Masking => "Masking",
        }
    }
    /// The panel whose switch travels with this group: every group of a panel with a
    /// switch carries it.
    fn panel(self) -> Option<Panel> {
        use SettingGroup::*;
        Some(match self {
            ToneCurve => Panel::ToneCurve,
            ColorAdjustments => Panel::ColorMixer,
            BlackWhiteMix => Panel::BlackWhiteMix,
            ColorGrading => Panel::ColorGrading,
            Sharpening | LuminanceNoiseReduction | ColorNoiseReduction => Panel::Detail,
            LensProfileCorrections | ChromaticAberration | LensVignetting => Panel::LensCorrections,
            UprightMode | TransformAdjustments => Panel::Transform,
            PostCropVignetting | Grain => Panel::Effects,
            Calibration => Panel::Calibration,
            SpotRemoval => Panel::SpotRemoval,
            Masking => Panel::Masks,
            _ => return None,
        })
    }
    /// Copies this group's settings from `from` into `to`, as the user set them.
    fn copy(self, from: &Recipe, to: &mut Recipe) {
        use SettingGroup::*;
        let (f, e) = (&from.effects, &mut to.effects);
        match self {
            WhiteBalance => {
                to.temperature = from.temperature;
                to.tint = from.tint;
            }
            Exposure => to.exposure = from.exposure,
            Contrast => to.contrast = from.contrast,
            Highlights => to.highlights = from.highlights,
            Shadows => to.shadows = from.shadows,
            Whites => to.whites = from.whites,
            Blacks => to.blacks = from.blacks,
            Texture => e.texture = f.texture,
            Clarity => e.clarity = f.clarity,
            Dehaze => e.dehaze = f.dehaze,
            Vibrance => to.vibrance = from.vibrance,
            Saturation => to.saturation = from.saturation,
            TreatmentAndProfile => {
                to.profile = from.profile.clone();
                e.monochrome = f.monochrome;
            }
            ToneCurve => {
                to.curve = from.curve.clone();
                to.black_point = from.black_point;
                to.white_point = from.white_point;
                to.midtone = from.midtone;
                e.channels = f.channels.clone();
                e.parametric = f.parametric;
                e.splits = f.splits;
            }
            ColorAdjustments => to.hsl = from.hsl,
            BlackWhiteMix => e.gray_mix = f.gray_mix,
            ColorGrading => {
                to.grading = from.grading;
                e.balance = f.balance;
                e.blending = f.blending;
                e.global_grade = f.global_grade;
            }
            Sharpening => {
                to.sharpening = from.sharpening;
                to.sharpening_radius = from.sharpening_radius;
                to.sharpening_detail = from.sharpening_detail;
                to.sharpening_masking = from.sharpening_masking;
            }
            LuminanceNoiseReduction => {
                to.noise_luma = from.noise_luma;
                e.luma_detail = f.luma_detail;
                e.luma_contrast = f.luma_contrast;
            }
            ColorNoiseReduction => {
                to.noise_chroma = from.noise_chroma;
                e.chroma_detail = f.chroma_detail;
                e.chroma_smoothness = f.chroma_smoothness;
            }
            LensProfileCorrections => {
                to.lens_builtin = from.lens_builtin;
                to.lens_profile = from.lens_profile;
                to.lens_distortion = from.lens_distortion;
                to.lens_vignetting = from.lens_vignetting;
            }
            ChromaticAberration => {
                to.lens_ca = from.lens_ca;
                e.defringe = f.defringe;
                e.defringe_ranges = f.defringe_ranges;
            }
            LensVignetting => {
                e.lens_vignette = f.lens_vignette;
                e.lens_vignette_midpoint = f.lens_vignette_midpoint;
            }
            UprightMode => to.upright.mode = from.upright.mode,
            TransformAdjustments => to.transform = from.transform,
            PostCropVignetting => {
                e.vignette = f.vignette;
                e.vignette_midpoint = f.vignette_midpoint;
                e.vignette_roundness = f.vignette_roundness;
                e.vignette_feather = f.vignette_feather;
                e.vignette_highlights = f.vignette_highlights;
                e.vignette_style = f.vignette_style;
            }
            Grain => {
                e.grain = f.grain;
                e.grain_size = f.grain_size;
                e.grain_roughness = f.grain_roughness;
                e.grain_seed = f.grain_seed;
            }
            ProcessVersion => {
                to.engine = from.engine;
                to.profile_tone = from.profile_tone;
                to.wide_gamut_curves = from.wide_gamut_curves;
                to.reference_curves = from.reference_curves;
                to.reference_calibration = from.reference_calibration;
                to.reference_color = from.reference_color;
            }
            Calibration => {
                e.calibration = f.calibration;
                e.shadow_tint = f.shadow_tint;
            }
            SpotRemoval => to.retouch = from.retouch.clone(),
            Crop => {
                to.crop = from.crop;
                to.straighten = from.straighten;
            }
            Masking => to.masks = from.masks.clone(),
        }
        if let Some(panel) = self.panel() {
            to.panels.set(panel, from.panels.state(panel));
        }
    }
}

/// Which groups a Copy, Paste or Sync transfers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GroupSelection(BTreeSet<SettingGroup>);

impl Default for GroupSelection {
    /// As Lightroom's Paste Settings: everything but spot removal and masks, which
    /// belong to the photo they were made on.
    fn default() -> Self {
        Self(
            SettingGroup::ALL
                .into_iter()
                .filter(|g| !matches!(g, SettingGroup::SpotRemoval | SettingGroup::Masking))
                .collect(),
        )
    }
}

impl GroupSelection {
    pub fn all() -> Self {
        Self(SettingGroup::ALL.into_iter().collect())
    }
    pub fn none() -> Self {
        Self(BTreeSet::new())
    }
    pub fn contains(&self, group: SettingGroup) -> bool {
        self.0.contains(&group)
    }
    pub fn set(&mut self, group: SettingGroup, included: GroupInclusion) {
        match included {
            GroupInclusion::Included => self.0.insert(group),
            GroupInclusion::Excluded => self.0.remove(&group),
        };
    }
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn groups(&self) -> impl Iterator<Item = SettingGroup> + '_ {
        self.0.iter().copied()
    }
}

/// Whether a group is part of a selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupInclusion {
    Included,
    Excluded,
}

/// The photo settings are transferred to: its camera, and the camera profiles
/// imported for it.
#[derive(Clone, Copy)]
pub struct Target<'a> {
    pub metadata: &'a Metadata,
    pub profiles: &'a [Arc<CameraProfile>],
}

/// The target's new recipe, and what could not transfer as it was.
#[derive(Clone, Debug, PartialEq)]
pub struct Transferred {
    pub recipe: Recipe,
    pub notes: Vec<String>,
}

/// The photo settings come from: its recipe and its camera.
#[derive(Clone, Copy)]
pub struct Source<'a> {
    pub recipe: &'a Recipe,
    pub metadata: &'a Metadata,
}

/// How a photo is turned and flipped for display (see [`super::display_axes`]).
fn display_axes(r: &Recipe, m: &Metadata) -> [[f32; 2]; 2] {
    let turns = super::ImageFrame::for_metadata(m).turns;
    super::display_axes((turns + r.rotation) % 4, r.flip_x, r.flip_y)
}

/// The source's settings in `selection` applied over `to`, with everything that depends
/// on the photo worked out again for `target`.
pub fn transfer(
    source: Source<'_>,
    to: &Recipe,
    selection: &GroupSelection,
    target: Target<'_>,
) -> Transferred {
    let from = source.recipe;
    let mut recipe = to.clone();
    let mut notes = Vec::new();
    for group in selection.groups() {
        group.copy(from, &mut recipe);
    }
    let m = target.metadata;
    if selection.contains(SettingGroup::TransformAdjustments) {
        // The sliders as shown on the source photo, along the target's displayed axes.
        recipe.transform = from
            .transform
            .displayed(display_axes(from, source.metadata))
            .recorded(display_axes(&recipe, m));
    }
    if selection.contains(SettingGroup::TreatmentAndProfile)
        && let Some(profile) = &from.profile
    {
        // A profile file belongs to one photo's camera (or its DNG): use the target's of
        // that name, else the source's when it fits this camera, else keep the target's.
        let name = profile.name.as_str();
        match target
            .profiles
            .iter()
            .find(|p| p.name == name && p.ensure_camera(m).is_ok())
        {
            Some(p) => recipe.profile = Some(p.clone()),
            None if profile.ensure_camera(m).is_ok() => {}
            None => {
                recipe.profile = to.profile.clone();
                notes.push(format!(
                    "{name} isn't available for this camera; kept its profile"
                ));
            }
        }
    }
    // The baseline depends on both the profile and the process version.
    if selection.contains(SettingGroup::TreatmentAndProfile)
        || selection.contains(SettingGroup::ProcessVersion)
    {
        recipe.use_camera_baseline(m);
    }
    if selection.contains(SettingGroup::WhiteBalance) {
        // The same Temperature and Tint mean different gains on another camera.
        recipe.update_wb(m);
        recipe.auto_white_balance = None;
    } else if selection.contains(SettingGroup::TreatmentAndProfile)
        || recipe.engine.min(4) != to.engine.min(4)
    {
        // A new profile, or a process version that brings the default camera profile,
        // maps the same gains to other Temperature and Tint values.
        recipe.sync_white_balance_controls(m);
    }
    // Upright's corrections are analysed from the photo as its lens corrections render
    // it: new lens settings call for a new analysis, which the editor runs.
    let lens = |r: &Recipe| {
        (
            r.lens_builtin,
            r.lens_profile,
            r.lens_distortion,
            r.panels.state(Panel::LensCorrections),
            // Lens corrections render from process version 4.
            r.engine >= 4,
        )
    };
    if lens(&recipe) != lens(to) {
        recipe.upright.corrections.clear();
        if recipe.upright.mode == super::UprightMode::Guided {
            recipe.upright.mode = super::UprightMode::Off;
            notes.push(
                "New lens corrections need Guided Upright's guides drawn again; left Off".into(),
            );
        }
    }
    // Only the mode transfers: the target keeps the corrections analysed from it, and
    // the editor analyses one it lacks. Guided needs guides drawn on the photo itself.
    let upright = &mut recipe.upright;
    if selection.contains(SettingGroup::UprightMode)
        && upright.mode == super::UprightMode::Guided
        && upright.corrections.len() <= upright.mode.code()
    {
        upright.mode = super::UprightMode::Off;
        notes.push("Guided Upright needs guides drawn on this photo; left Off".into());
    }
    Transferred { recipe, notes }
}

/// Every recipe setting's kind, for the coverage test. Taken apart without `..`, so
/// adding a field to `Recipe` or `Effects` is a compile error here until it is placed.
#[cfg(test)]
pub(crate) fn every_setting(r: &Recipe) -> Vec<(&'static str, Kind)> {
    use Kind::*;
    use SettingGroup::*;
    let Recipe {
        engine: _,
        lens_builtin: _,
        lens_profile: _,
        lens_distortion: _,
        lens_vignetting: _,
        lens_ca: _,
        profile_tone: _,
        effects,
        preset_name: _,
        preset_settings: _,
        profile: _,
        sharpening_radius: _,
        sharpening_detail: _,
        sharpening_masking: _,
        exposure: _,
        camera_exposure: _,
        wide_gamut_curves: _,
        reference_curves: _,
        reference_calibration: _,
        reference_color: _,
        temperature: _,
        tint: _,
        wb: _,
        auto_white_balance: _,
        contrast: _,
        highlights: _,
        shadows: _,
        whites: _,
        blacks: _,
        black_point: _,
        white_point: _,
        midtone: _,
        curve: _,
        saturation: _,
        vibrance: _,
        hsl: _,
        grading: _,
        noise_luma: _,
        noise_chroma: _,
        sharpening: _,
        crop: _,
        straighten: _,
        transform: _,
        upright: _,
        rotation: _,
        flip_x: _,
        flip_y: _,
        retouch: _,
        masks: _,
        panels: _,
        unknown: _,
    } = r;
    let crate::develop::effects::Effects {
        channels: _,
        parametric: _,
        splits: _,
        calibration: _,
        shadow_tint: _,
        monochrome: _,
        gray_mix: _,
        balance: _,
        blending: _,
        global_grade: _,
        clarity: _,
        texture: _,
        dehaze: _,
        grain: _,
        grain_size: _,
        grain_roughness: _,
        grain_seed: _,
        vignette: _,
        vignette_midpoint: _,
        vignette_roundness: _,
        vignette_feather: _,
        vignette_highlights: _,
        vignette_style: _,
        lens_vignette: _,
        lens_vignette_midpoint: _,
        defringe: _,
        defringe_ranges: _,
        luma_detail: _,
        luma_contrast: _,
        chroma_detail: _,
        chroma_smoothness: _,
    } = effects;
    vec![
        ("engine", Group(ProcessVersion)),
        ("lens_builtin", Group(LensProfileCorrections)),
        ("lens_profile", Group(LensProfileCorrections)),
        ("lens_distortion", Group(LensProfileCorrections)),
        ("lens_vignetting", Group(LensProfileCorrections)),
        ("lens_ca", Group(ChromaticAberration)),
        ("profile_tone", Group(ProcessVersion)),
        ("preset_name", PhotosOwn),
        ("preset_settings", PhotosOwn),
        ("profile", Group(TreatmentAndProfile)),
        ("sharpening_radius", Group(Sharpening)),
        ("sharpening_detail", Group(Sharpening)),
        ("sharpening_masking", Group(Sharpening)),
        ("exposure", Group(Exposure)),
        ("camera_exposure", Derived),
        ("wide_gamut_curves", Group(ProcessVersion)),
        ("reference_curves", Group(ProcessVersion)),
        ("reference_calibration", Group(ProcessVersion)),
        ("reference_color", Group(ProcessVersion)),
        ("temperature", Group(WhiteBalance)),
        ("tint", Group(WhiteBalance)),
        ("wb", Derived),
        ("auto_white_balance", Derived),
        ("contrast", Group(Contrast)),
        ("highlights", Group(Highlights)),
        ("shadows", Group(Shadows)),
        ("whites", Group(Whites)),
        ("blacks", Group(Blacks)),
        ("black_point", Group(ToneCurve)),
        ("white_point", Group(ToneCurve)),
        ("midtone", Group(ToneCurve)),
        ("curve", Group(ToneCurve)),
        ("saturation", Group(Saturation)),
        ("vibrance", Group(Vibrance)),
        ("hsl", Group(ColorAdjustments)),
        ("grading", Group(ColorGrading)),
        ("noise_luma", Group(LuminanceNoiseReduction)),
        ("noise_chroma", Group(ColorNoiseReduction)),
        ("sharpening", Group(Sharpening)),
        ("crop", Group(Crop)),
        ("straighten", Group(Crop)),
        ("transform", Group(TransformAdjustments)),
        // The mode transfers; its corrections are analysed for each photo.
        ("upright", Group(UprightMode)),
        ("rotation", PhotosOwn),
        ("flip_x", PhotosOwn),
        ("flip_y", PhotosOwn),
        ("retouch", Group(SpotRemoval)),
        ("masks", Group(Masking)),
        // Each switch travels with its panel's groups.
        ("panels", Derived),
        ("unknown", PhotosOwn),
        ("effects.channels", Group(ToneCurve)),
        ("effects.parametric", Group(ToneCurve)),
        ("effects.splits", Group(ToneCurve)),
        ("effects.calibration", Group(Calibration)),
        ("effects.shadow_tint", Group(Calibration)),
        ("effects.monochrome", Group(TreatmentAndProfile)),
        ("effects.gray_mix", Group(BlackWhiteMix)),
        ("effects.balance", Group(ColorGrading)),
        ("effects.blending", Group(ColorGrading)),
        ("effects.global_grade", Group(ColorGrading)),
        ("effects.clarity", Group(Clarity)),
        ("effects.texture", Group(Texture)),
        ("effects.dehaze", Group(Dehaze)),
        ("effects.grain", Group(Grain)),
        ("effects.grain_size", Group(Grain)),
        ("effects.grain_roughness", Group(Grain)),
        ("effects.grain_seed", Group(Grain)),
        ("effects.vignette", Group(PostCropVignetting)),
        ("effects.vignette_midpoint", Group(PostCropVignetting)),
        ("effects.vignette_roundness", Group(PostCropVignetting)),
        ("effects.vignette_feather", Group(PostCropVignetting)),
        ("effects.vignette_highlights", Group(PostCropVignetting)),
        ("effects.vignette_style", Group(PostCropVignetting)),
        ("effects.lens_vignette", Group(LensVignetting)),
        ("effects.lens_vignette_midpoint", Group(LensVignetting)),
        ("effects.defringe", Group(ChromaticAberration)),
        ("effects.defringe_ranges", Group(ChromaticAberration)),
        ("effects.luma_detail", Group(LuminanceNoiseReduction)),
        ("effects.luma_contrast", Group(LuminanceNoiseReduction)),
        ("effects.chroma_detail", Group(ColorNoiseReduction)),
        ("effects.chroma_smoothness", Group(ColorNoiseReduction)),
    ]
}

/// How a setting transfers (see the module documentation).
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Group(SettingGroup),
    Derived,
    PhotosOwn,
}

#[cfg(test)]
mod tests;
