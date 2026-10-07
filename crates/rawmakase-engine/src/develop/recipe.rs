//! What rendering makes of a recipe's settings: the measured manual Vignetting and
//! Color noise reduction it runs on the camera image. The recipe itself is
//! [`crate::model::recipe`]'s.
use crate::model::recipe::Recipe;

/// What rendering makes of a recipe's settings, as it runs them on the camera image.
pub(crate) trait RenderedRecipe {
    /// Manual lens Vignetting as measured in Camera Raw, applied with the lens
    /// profile's to the camera image; `None` at Amount 0 and for recipes that keep the
    /// original operator, which [`crate::develop::effects::spatial_finish`] applies.
    fn manual_vignette(&self) -> Option<crate::develop::effects::ManualVignette>;
    /// The measured Color noise reduction to run on the camera image; `None` for the
    /// original operator, which [`Recipe::sampled_noise_chroma`] applies at sampling.
    fn chroma_denoise(&self) -> Option<crate::develop::color_noise::ChromaDenoise>;
}
impl RenderedRecipe for Recipe {
    fn manual_vignette(&self) -> Option<crate::develop::effects::ManualVignette> {
        if self.lens_vignette_model.is_original() {
            return None;
        }
        crate::develop::effects::ManualVignette::new(
            self.effects.lens_vignette,
            self.effects.lens_vignette_midpoint,
        )
    }
    fn chroma_denoise(&self) -> Option<crate::develop::color_noise::ChromaDenoise> {
        if !self.measures_color_noise() {
            return None;
        }
        crate::develop::color_noise::ChromaDenoise::new(
            self.noise_chroma,
            self.effects.chroma_detail,
            self.effects.chroma_smoothness,
        )
    }
}
