//! Lightroom's per-panel switches: a panel switched off keeps its settings but renders
//! as if they were at their defaults.
//!
//! Lightroom stores one `Enable*` flag per panel in each photo's develop settings. The
//! settings each switch covers follow the panel layout Lightroom Classic 15 names in
//! its own history (the Basic panel and crop have no switch).
use super::Recipe;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// A Develop panel with a switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Panel {
    ToneCurve,
    ColorMixer,
    BlackWhiteMix,
    ColorGrading,
    Detail,
    LensCorrections,
    Transform,
    Effects,
    Calibration,
    SpotRemoval,
    RedEye,
    Masks,
}

/// Whether a panel's settings are applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelState {
    On,
    Off,
}

impl Panel {
    pub const ALL: [Panel; 12] = [
        Panel::ToneCurve,
        Panel::ColorMixer,
        Panel::BlackWhiteMix,
        Panel::ColorGrading,
        Panel::Detail,
        Panel::LensCorrections,
        Panel::Transform,
        Panel::Effects,
        Panel::Calibration,
        Panel::SpotRemoval,
        Panel::RedEye,
        Panel::Masks,
    ];
    /// Lightroom's develop-settings keys for this switch; the first is the one written.
    /// Masks have one switch since Lightroom 11 and three for the older local tools,
    /// which RAWmakase converts into the same masks.
    pub fn lightroom_keys(self) -> &'static [&'static str] {
        match self {
            Panel::ToneCurve => &["EnableToneCurve"],
            Panel::ColorMixer => &["EnableColorAdjustments"],
            Panel::BlackWhiteMix => &["EnableGrayscaleMix"],
            Panel::ColorGrading => &["EnableSplitToning"],
            Panel::Detail => &["EnableDetail"],
            Panel::LensCorrections => &["EnableLensCorrections"],
            Panel::Transform => &["EnableTransform"],
            Panel::Effects => &["EnableEffects"],
            Panel::Calibration => &["EnableCalibration"],
            Panel::SpotRemoval => &["EnableRetouch"],
            Panel::RedEye => &["EnableRedEye"],
            Panel::Masks => &[
                "EnableMaskGroupBasedCorrections",
                "EnablePaintBasedCorrections",
                "EnableGradientBasedCorrections",
                "EnableCircularGradientBasedCorrections",
            ],
        }
    }
    /// Sets this panel's settings in `r` to the values of `defaults`.
    fn bypass(self, r: &mut Recipe, defaults: &Recipe) {
        let (e, d) = (&mut r.effects, &defaults.effects);
        match self {
            Panel::ToneCurve => {
                r.curve = defaults.curve.clone();
                r.curve_saturation = defaults.curve_saturation;
                e.channels = d.channels.clone();
                e.parametric = d.parametric;
                e.splits = d.splits;
            }
            Panel::ColorMixer => r.hsl = defaults.hsl,
            Panel::BlackWhiteMix => e.gray_mix = d.gray_mix,
            Panel::ColorGrading => {
                r.grading = defaults.grading;
                e.balance = d.balance;
                e.blending = d.blending;
                e.global_grade = d.global_grade;
            }
            Panel::Detail => {
                r.sharpening = 0.;
                r.sharpening_radius = defaults.sharpening_radius;
                r.sharpening_detail = defaults.sharpening_detail;
                r.sharpening_masking = defaults.sharpening_masking;
                r.noise_luma = 0.;
                r.noise_chroma = 0.;
                e.luma_detail = d.luma_detail;
                e.luma_contrast = d.luma_contrast;
                e.chroma_detail = d.chroma_detail;
                e.chroma_smoothness = d.chroma_smoothness;
            }
            // The correction a camera stores in its RAW stays: Lightroom applies it
            // outside the panel's controls (inferred; not yet measured).
            Panel::LensCorrections => {
                r.lens_profile = false;
                r.lens_ca = false;
                r.lens_distortion = defaults.lens_distortion;
                r.lens_vignetting = defaults.lens_vignetting;
                r.lens_manual_distortion = defaults.lens_manual_distortion;
                e.defringe = [0.; 2];
                e.defringe_ranges = d.defringe_ranges;
                e.lens_vignette = 0.;
                e.lens_vignette_midpoint = d.lens_vignette_midpoint;
            }
            Panel::Transform => {
                r.transform = defaults.transform;
                r.upright = defaults.upright.clone();
                r.constrain_crop = defaults.constrain_crop;
            }
            Panel::Effects => e.reset_post_crop(),
            Panel::Calibration => {
                e.calibration = d.calibration;
                e.shadow_tint = d.shadow_tint;
            }
            Panel::SpotRemoval => r.retouch.clear(),
            Panel::RedEye => r.red_eye.clear(),
            Panel::Masks => r.masks.clear(),
        }
    }
}

/// The panels switched off in a recipe. Every panel is on by default.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelSwitches {
    off: BTreeSet<Panel>,
}

impl PanelSwitches {
    pub fn state(&self, panel: Panel) -> PanelState {
        if self.off.contains(&panel) {
            PanelState::Off
        } else {
            PanelState::On
        }
    }
    pub fn set(&mut self, panel: Panel, state: PanelState) {
        match state {
            PanelState::On => self.off.remove(&panel),
            PanelState::Off => self.off.insert(panel),
        };
    }
    pub fn all_on(&self) -> bool {
        self.off.is_empty()
    }
    pub fn switched_off(&self) -> impl Iterator<Item = Panel> + '_ {
        self.off.iter().copied()
    }
}

impl Recipe {
    /// The recipe with every switched-off panel at its defaults, as preview, export and
    /// thumbnails render it. The stored settings are unchanged.
    pub fn as_rendered(&self) -> std::borrow::Cow<'_, Recipe> {
        if self.panels.all_on() {
            return std::borrow::Cow::Borrowed(self);
        }
        let defaults = Recipe::default();
        let mut r = self.clone();
        for panel in self.panels.switched_off() {
            panel.bypass(&mut r, &defaults);
        }
        // The switches stay, so `Recipe::resolved`, which knows the camera, can also
        // turn off lens data that is only on because of the panel.
        std::borrow::Cow::Owned(r)
    }
}
