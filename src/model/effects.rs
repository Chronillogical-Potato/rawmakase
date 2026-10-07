//! The Effects, Detail and Calibration settings a recipe keeps beside the Basic
//! panel's: curves, grading, grain, vignettes, Defringe and noise reduction.
use crate::color::curve::ToneCurve;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// Lightroom's post-crop vignette styles. Recipes and XMP store Lightroom's codes:
/// 1 Highlight Priority, 2 Color Priority, 3 Paint Overlay. Camera Raw 18.7 renders 0
/// and an omitted style as Highlight Priority.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum VignetteStyle {
    #[default]
    HighlightPriority,
    ColorPriority,
    PaintOverlay,
}
impl VignetteStyle {
    pub fn code(self) -> u8 {
        match self {
            VignetteStyle::HighlightPriority => 1,
            VignetteStyle::ColorPriority => 2,
            VignetteStyle::PaintOverlay => 3,
        }
    }
}
impl TryFrom<u8> for VignetteStyle {
    type Error = anyhow::Error;
    fn try_from(code: u8) -> Result<Self> {
        Ok(match code {
            0 | 1 => VignetteStyle::HighlightPriority,
            2 => VignetteStyle::ColorPriority,
            3 => VignetteStyle::PaintOverlay,
            _ => anyhow::bail!("Unsupported vignette style {code}"),
        })
    }
}
impl From<VignetteStyle> for u8 {
    fn from(style: VignetteStyle) -> u8 {
        style.code()
    }
}
/// Defringe's default hue ranges: Purple, then Green (Lightroom's 30–70 and 40–60).
pub const DEFRINGE_RANGES: [[f32; 2]; 2] = [[0.3, 0.7], [0.4, 0.6]];
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Effects {
    pub channels: [ToneCurve; 3],
    pub parametric: [f32; 4],
    pub splits: [f32; 3],
    pub calibration: [[f32; 2]; 3],
    pub shadow_tint: f32,
    pub monochrome: bool,
    pub gray_mix: [f32; 8],
    pub balance: f32,
    pub blending: f32,
    pub global_grade: [f32; 3],
    pub clarity: f32,
    pub texture: f32,
    pub dehaze: f32,
    pub grain: f32,
    pub grain_size: f32,
    pub grain_roughness: f32,
    pub grain_seed: u32,
    pub vignette: f32,
    pub vignette_midpoint: f32,
    pub vignette_roundness: f32,
    pub vignette_feather: f32,
    pub vignette_highlights: f32,
    pub vignette_style: VignetteStyle,
    pub lens_vignette: f32,
    pub lens_vignette_midpoint: f32,
    pub defringe: [f32; 2],
    pub defringe_ranges: [[f32; 2]; 2],
    pub luma_detail: f32,
    pub luma_contrast: f32,
    pub chroma_detail: f32,
    pub chroma_smoothness: f32,
}
impl Default for Effects {
    fn default() -> Self {
        Self {
            channels: std::array::from_fn(|_| ToneCurve::default()),
            parametric: [0.; 4],
            splits: [0.25, 0.5, 0.75],
            calibration: [[0.; 2]; 3],
            shadow_tint: 0.,
            monochrome: false,
            gray_mix: [0.; 8],
            balance: 0.,
            blending: 0.5,
            global_grade: [0.; 3],
            clarity: 0.,
            texture: 0.,
            dehaze: 0.,
            grain: 0.,
            grain_size: 0.25,
            grain_roughness: 0.5,
            grain_seed: 42,
            vignette: 0.,
            vignette_midpoint: 0.5,
            vignette_roundness: 0.,
            vignette_feather: 0.5,
            vignette_highlights: 0.,
            vignette_style: VignetteStyle::HighlightPriority,
            lens_vignette: 0.,
            lens_vignette_midpoint: 0.5,
            defringe: [0.; 2],
            defringe_ranges: DEFRINGE_RANGES,
            luma_detail: 0.5,
            luma_contrast: 0.,
            chroma_detail: 0.5,
            chroma_smoothness: 0.5,
        }
    }
}
impl Effects {
    /// The Effects panel's reset: post-crop vignette and grain at their defaults. Other
    /// panels' settings and the grain seed are kept.
    pub fn reset_post_crop(&mut self) {
        let d = Effects::default();
        self.vignette = d.vignette;
        self.vignette_midpoint = d.vignette_midpoint;
        self.vignette_roundness = d.vignette_roundness;
        self.vignette_feather = d.vignette_feather;
        self.vignette_highlights = d.vignette_highlights;
        self.vignette_style = d.vignette_style;
        self.grain = d.grain;
        self.grain_size = d.grain_size;
        self.grain_roughness = d.grain_roughness;
    }
    pub fn validate(&self) -> Result<()> {
        for c in &self.channels {
            c.validate()?;
        }
        ensure!(
            self.parametric
                .iter()
                .chain(self.calibration.iter().flatten())
                .chain(self.gray_mix.iter())
                .chain([
                    &self.shadow_tint,
                    &self.balance,
                    &self.clarity,
                    &self.texture,
                    &self.dehaze,
                    &self.vignette,
                    &self.vignette_roundness,
                    &self.lens_vignette
                ])
                .all(|v| v.is_finite() && v.abs() <= 1.),
            "Invalid preset effect"
        );
        ensure!(
            self.splits[0] > 0.
                && self.splits[2] < 1.
                && self.splits.windows(2).all(|p| p[0] < p[1]),
            "Invalid parametric curve splits"
        );
        ensure!(
            [
                self.blending,
                self.grain,
                self.grain_size,
                self.grain_roughness,
                self.vignette_midpoint,
                self.vignette_feather,
                self.vignette_highlights,
                self.lens_vignette_midpoint,
                self.luma_detail,
                self.luma_contrast,
                self.chroma_detail,
                self.chroma_smoothness
            ]
            .iter()
            .chain(self.defringe.iter())
            .chain(self.defringe_ranges.iter().flatten())
            .all(|v| v.is_finite() && (0. ..=1.).contains(v)),
            "Invalid effect range"
        );
        ensure!(
            self.global_grade
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 1.),
            "Invalid global grade"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_effects_fill_omitted_settings_and_keep_lightroom_style_codes() {
        let e: Effects =
            serde_json::from_str(r#"{"vignette": -0.4, "vignette_style": 2}"#).unwrap();
        assert_eq!(e.vignette, -0.4);
        assert_eq!(e.vignette_style, VignetteStyle::ColorPriority);
        assert_eq!(e.defringe_ranges, DEFRINGE_RANGES);
        assert_eq!(e.grain_seed, 42);
        // Camera Raw renders style 0 as Highlight Priority; it is stored as 1.
        let e: Effects = serde_json::from_str(r#"{"vignette_style": 0}"#).unwrap();
        assert_eq!(e.vignette_style, VignetteStyle::HighlightPriority);
        assert!(
            serde_json::to_string(&e)
                .unwrap()
                .contains(r#""vignette_style":1"#)
        );
        assert!(serde_json::from_str::<Effects>(r#"{"vignette_style": 4}"#).is_err());
        assert!(serde_json::from_str::<Effects>(r#"{"unknown": 1}"#).is_err());
        assert!(Effects::default().validate().is_ok());
    }

    /// Every setting's stored name and default, as saved edits and presets hold them.
    #[test]
    fn stored_defaults_keep_every_name() {
        let stored = serde_json::to_value(Effects::default()).unwrap();
        let curve = serde_json::to_value(ToneCurve::default()).unwrap();
        let expected = serde_json::json!({
            "channels": [curve.clone(), curve.clone(), curve],
            "parametric": [0.0, 0.0, 0.0, 0.0],
            "splits": [0.25, 0.5, 0.75],
            "calibration": [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
            "shadow_tint": 0.0,
            "monochrome": false,
            "gray_mix": [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
            "balance": 0.0,
            "blending": 0.5,
            "global_grade": [0.0, 0.0, 0.0],
            "clarity": 0.0,
            "texture": 0.0,
            "dehaze": 0.0,
            "grain": 0.0,
            "grain_size": 0.25,
            "grain_roughness": 0.5,
            "grain_seed": 42,
            "vignette": 0.0,
            "vignette_midpoint": 0.5,
            "vignette_roundness": 0.0,
            "vignette_feather": 0.5,
            "vignette_highlights": 0.0,
            "vignette_style": 1,
            "lens_vignette": 0.0,
            "lens_vignette_midpoint": 0.5,
            "defringe": [0.0, 0.0],
            "defringe_ranges": [[0.3_f32, 0.7_f32], [0.4_f32, 0.6_f32]],
            "luma_detail": 0.5,
            "luma_contrast": 0.0,
            "chroma_detail": 0.5,
            "chroma_smoothness": 0.5,
        });
        assert_eq!(stored, expected);
    }
}
