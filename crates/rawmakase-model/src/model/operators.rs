//! Which operator renders each setting: the versions a recipe records so it
//! renders as it did when saved. A newer operator never replaces an older one
//! in a saved edit (docs/architecture.md); new recipes take the newest.
use serde::{Deserialize, Serialize};

/// Which operator renders Texture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextureModel {
    /// RAWmakase's first Texture, a 3-pixel box detail: what recipes saved before the
    /// measured one keep, so they render as they did.
    #[default]
    Original,
    /// Fitted to Camera Raw 18.7.
    Measured,
}
impl TextureModel {
    pub fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// Which operator renders a recipe's sharpening.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SharpeningModel {
    /// RAWmakase's first unsharp mask: what recipes saved before the measured model
    /// keep, so they render as they did.
    #[default]
    Original,
    /// The sharpening measured in Camera Raw.
    Measured,
}
impl SharpeningModel {
    pub fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// Which soft edge a recipe's Heal and Clone operations render with.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetouchModel {
    /// RAWmakase's first feather, a smoothstep over the feathered width: what recipes
    /// saved before the measured one keep, so they render as they did.
    #[default]
    Original,
    /// The feather measured in Camera Raw.
    Measured,
}
impl RetouchModel {
    pub(crate) fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// Which operator renders a recipe's parametric curve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParametricModel {
    /// RAWmakase's first approximation, applied to each channel: what recipes saved
    /// before the measured curve keep, so they render as they did.
    #[default]
    Original,
    /// The curve measured in Camera Raw, applied as DNG RGBTone; a look's own
    /// parametric curve is added to the user's regions.
    Measured,
    /// As `Measured`, with a look's own parametric curve applied as a second curve
    /// after the user's, as Camera Raw 18.7 renders it.
    Layered,
}
impl ParametricModel {
    pub(crate) fn is_original(&self) -> bool {
        *self == Self::Original
    }
    /// Whether the measured curve renders the regions.
    pub fn is_measured(&self) -> bool {
        *self != Self::Original
    }
}

/// Which operator renders a recipe's manual lens Vignetting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LensVignetteModel {
    /// RAWmakase's first version, applied to the finished pixels after the crop: what
    /// recipes saved before the measured model keep, so they render as they did.
    #[default]
    Original,
    /// The gain measured in Camera Raw, applied to the camera image.
    Measured,
}
impl LensVignetteModel {
    pub fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// Which operator renders a recipe's grain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrainModel {
    /// RAWmakase's first grain: what recipes saved before the measured model keep, so
    /// they render as they did.
    #[default]
    Original,
    /// The grain measured in Camera Raw.
    Measured,
}
impl GrainModel {
    pub fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// Which operator renders a recipe's colour noise reduction.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoiseModel {
    /// RAWmakase's first colour noise reduction, a 3×3 filter at sampling: what
    /// recipes saved before the measured one keep, so they render as they did.
    #[default]
    Original,
    /// The colour noise reduction measured in Camera Raw.
    Measured,
}
impl NoiseModel {
    pub fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// Which tables render the Vibrance slider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VibranceModel {
    /// Tables measured on photos at ±50 and extrapolated: what recipes saved before
    /// the chart tables keep, so they render as they did.
    #[default]
    Original,
    /// The photo tables up to ±50, then Camera Raw 18.7's change at ±75 and ±100
    /// measured on the dense synthetic chart, interpolated between
    /// (docs/color-mixer.md#vibrance).
    Chart,
}
impl VibranceModel {
    pub fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// Which operator renders the global Saturation slider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SaturationModel {
    /// Tables measured on photos at ±50 and extrapolated: what recipes saved
    /// before the gray fade keep, so they render as they did.
    #[default]
    Original,
    /// The same tables down to −50, then a fade to the color's luminance, which
    /// Camera Raw 18.7 reaches exactly at −100 (docs/color-mixer.md#saturation).
    Gray,
}
impl SaturationModel {
    pub fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// Which measured tables render a recipe's color mixer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MixerModel {
    /// Tables measured on nine photos: what recipes saved before the chart tables
    /// keep, so they render as they did.
    #[default]
    Original,
    /// The eight bands' tables refitted to Camera Raw 18.7 on a dense synthetic
    /// chart (docs/color-mixer.md#chart-tables); Saturation and Vibrance as before.
    Chart,
}
impl MixerModel {
    pub fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// Which operator renders a recipe's color grading.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GradingModel {
    /// Luminance-driven tables at the default Blending and Balance, and RAWmakase's
    /// first approximation elsewhere: what recipes saved before the measured curves
    /// keep, so they render as they did.
    #[default]
    Original,
    /// Camera Raw 18.7's per-channel curves, for every setting.
    Measured,
}
impl GradingModel {
    pub(crate) fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// Which operator renders positive Clarity.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClarityModel {
    /// The detail gain of `quality::local`: what recipes saved before the measured one
    /// keep, so they render as they did.
    #[default]
    Original,
    /// Fitted to Camera Raw 18.7 (docs/tone-controls.md#clarity). Negative Clarity
    /// keeps the original operator.
    Measured,
}
impl ClarityModel {
    pub fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// Which fit renders Camera Calibration's primary Hue and Saturation sliders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CalibrationModel {
    /// Coefficients from isolated Adobe Standard/X100F controls, scaled linearly:
    /// what recipes saved before the measured fit keep, so they render as they did.
    #[default]
    Original,
    /// Camera Raw 18.7's change at four slider positions, interpolated.
    Measured,
}
impl CalibrationModel {
    pub fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// Which operator renders the black & white mix.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlackWhiteModel {
    /// A change to Oklab lightness by the hue-weighted mix times chroma: what
    /// recipes saved before the chart tables keep, so they render as they did.
    #[default]
    Original,
    /// Camera Raw 18.7's gray, measured on a dense synthetic chart: a table of the
    /// gray's luminance against the color's, and one per band at −100, −50, +50 and
    /// +100 (docs/color-mixer.md#chart-gray).
    Chart,
}
impl BlackWhiteModel {
    pub fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// How a recipe's positive Whites renders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhitesModel {
    /// The median of five photos' curves: what recipes saved before the adaptive one
    /// keep, so they render as they did.
    #[default]
    Original,
    /// Camera Raw's curve for a photo whose highlights are as bright as this one's
    /// (docs/tone-controls.md#whites).
    Adaptive,
}
impl WhitesModel {
    pub(crate) fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// How a recipe's Contrast renders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContrastModel {
    /// The curve averaged over five photos, before Whites and Blacks: what recipes
    /// saved before the adaptive one keep, so they render as they did.
    #[default]
    Original,
    /// Camera Raw's curve, measured on the synthetic chart and moved to the photo's own
    /// pivot, after Whites and Blacks (docs/tone-controls.md#contrast).
    Adaptive,
}
impl ContrastModel {
    pub(crate) fn is_original(&self) -> bool {
        *self == Self::Original
    }
}

/// How colors outside sRGB are brought into it at the end of the color stage.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GamutModel {
    /// Chroma compressed toward the neutral of the same lightness: what recipes saved
    /// before the clipped model keep, so they render as they did.
    #[default]
    Compress,
    /// Each channel clipped on its own, as Camera Raw's conversion to sRGB does
    /// (docs/color-pipeline.md#out-of-gamut-colors).
    Clip,
}
impl GamutModel {
    pub(crate) fn is_compress(&self) -> bool {
        *self == Self::Compress
    }
}

/// The Sharpening sliders, in recipe units (Amount 1 is Lightroom's 150).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SharpeningSliders {
    pub amount: f32,
    pub radius: f32,
    pub detail: f32,
    pub masking: f32,
}
impl SharpeningSliders {
    /// A new edit's sliders for `model`: Lightroom Classic's raw defaults (in every
    /// raw and DNG import of the catalogue sampled, process versions 2012 to 6) with
    /// the measured operator, RAWmakase's own with the original.
    pub fn defaults(model: SharpeningModel) -> Self {
        match model {
            SharpeningModel::Original => Self {
                amount: 0.35,
                radius: 0.8,
                detail: 0.25,
                masking: 0.35,
            },
            SharpeningModel::Measured => Self {
                amount: 40. / 150.,
                radius: 1.,
                detail: 0.25,
                masking: 0.,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::DeserializeOwned;

    /// Saved edits name operators by these strings and default to the oldest.
    fn stored_as<T>(oldest: T, names: &[(T, &str)])
    where
        T: Default + PartialEq + std::fmt::Debug + Serialize + DeserializeOwned,
    {
        assert_eq!(T::default(), oldest);
        for (value, name) in names {
            let json = format!("\"{name}\"");
            assert_eq!(serde_json::to_string(value).unwrap(), json);
            assert_eq!(&serde_json::from_str::<T>(&json).unwrap(), value);
        }
    }

    #[test]
    fn operators_keep_their_stored_names_and_oldest_defaults() {
        use TextureModel as T;
        stored_as(
            T::Original,
            &[(T::Original, "Original"), (T::Measured, "Measured")],
        );
        use SharpeningModel as S;
        stored_as(
            S::Original,
            &[(S::Original, "Original"), (S::Measured, "Measured")],
        );
        use RetouchModel as R;
        stored_as(
            R::Original,
            &[(R::Original, "Original"), (R::Measured, "Measured")],
        );
        use ParametricModel as P;
        stored_as(
            P::Original,
            &[
                (P::Original, "Original"),
                (P::Measured, "Measured"),
                (P::Layered, "Layered"),
            ],
        );
        use LensVignetteModel as L;
        stored_as(
            L::Original,
            &[(L::Original, "Original"), (L::Measured, "Measured")],
        );
        use GrainModel as G;
        stored_as(
            G::Original,
            &[(G::Original, "Original"), (G::Measured, "Measured")],
        );
        use NoiseModel as N;
        stored_as(
            N::Original,
            &[(N::Original, "Original"), (N::Measured, "Measured")],
        );
        use VibranceModel as V;
        stored_as(
            V::Original,
            &[(V::Original, "Original"), (V::Chart, "Chart")],
        );
        use SaturationModel as Sa;
        stored_as(
            Sa::Original,
            &[(Sa::Original, "Original"), (Sa::Gray, "Gray")],
        );
        use MixerModel as M;
        stored_as(
            M::Original,
            &[(M::Original, "Original"), (M::Chart, "Chart")],
        );
        use GradingModel as Gr;
        stored_as(
            Gr::Original,
            &[(Gr::Original, "Original"), (Gr::Measured, "Measured")],
        );
        use ClarityModel as C;
        stored_as(
            C::Original,
            &[(C::Original, "Original"), (C::Measured, "Measured")],
        );
        use CalibrationModel as Ca;
        stored_as(
            Ca::Original,
            &[(Ca::Original, "Original"), (Ca::Measured, "Measured")],
        );
        use BlackWhiteModel as B;
        stored_as(
            B::Original,
            &[(B::Original, "Original"), (B::Chart, "Chart")],
        );
        use WhitesModel as W;
        stored_as(
            W::Original,
            &[(W::Original, "Original"), (W::Adaptive, "Adaptive")],
        );
        use WhiteBalanceModel as Wb;
        stored_as(
            Wb::Original,
            &[(Wb::Original, "Original"), (Wb::Calibrated, "Calibrated")],
        );
        use ContrastModel as Co;
        stored_as(
            Co::Original,
            &[(Co::Original, "Original"), (Co::Adaptive, "Adaptive")],
        );
        use GamutModel as Ga;
        stored_as(
            Ga::Compress,
            &[(Ga::Compress, "Compress"), (Ga::Clip, "Clip")],
        );
    }

    #[test]
    fn sharpening_defaults_are_lightroom_s_and_rawmakase_s_earlier_ones() {
        let measured = SharpeningSliders::defaults(SharpeningModel::Measured);
        // Lightroom's Amount 40 (of 150), Radius 1.0, Detail 25, Masking 0.
        assert_eq!(
            [
                measured.amount,
                measured.radius,
                measured.detail,
                measured.masking
            ],
            [40. / 150., 1., 0.25, 0.]
        );
        let original = SharpeningSliders::defaults(SharpeningModel::Original);
        assert_eq!(
            [
                original.amount,
                original.radius,
                original.detail,
                original.masking
            ],
            [0.35, 0.8, 0.25, 0.35]
        );
    }
}

/// Calibration used when translating Temperature/Tint to camera-neutral gains.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WhiteBalanceModel {
    /// The pre-calibration conversion, including the original X100F correction.
    #[default]
    Original,
    /// Exact camera or DNG calibration with profile signature matching.
    Calibrated,
}
impl WhiteBalanceModel {
    pub(crate) fn is_original(&self) -> bool {
        *self == Self::Original
    }
}
