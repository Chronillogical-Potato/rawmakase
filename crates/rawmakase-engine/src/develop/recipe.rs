//! What rendering makes of a recipe's settings: the measured manual Vignetting and
//! color and luminance noise reduction it runs on the camera image. The recipe
//! itself is [`crate::model::recipe`]'s.
use crate::model::recipe::Recipe;

/// What rendering makes of a recipe's settings, as it runs them on the camera image.
pub(crate) trait RenderedRecipe {
    /// Manual lens Vignetting as measured in Camera Raw, applied with the lens
    /// profile's to the camera image; `None` at Amount 0.
    fn manual_vignette(&self) -> Option<crate::develop::effects::ManualVignette>;
    /// Color and luminance noise reduction on the camera image; `None` when both
    /// amounts are zero.
    fn denoise(&self) -> Option<Denoise>;
}
impl RenderedRecipe for Recipe {
    fn manual_vignette(&self) -> Option<crate::develop::effects::ManualVignette> {
        crate::develop::effects::ManualVignette::new(
            self.effects.lens_vignette,
            self.effects.lens_vignette_midpoint,
        )
    }
    fn denoise(&self) -> Option<Denoise> {
        let chroma = crate::develop::color_noise::ChromaDenoise::new(
            self.noise_chroma,
            self.effects.chroma_detail,
            self.effects.chroma_smoothness,
        );
        let luminance = crate::develop::luminance_noise::LuminanceDenoise::new(
            self.noise_luma,
            self.effects.luma_detail,
            self.effects.luma_contrast,
        );
        (chroma.is_some() || luminance.is_some()).then_some(Denoise { chroma, luminance })
    }
}

/// All noise settings and their execution, shared by cached previews and exports.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Denoise {
    chroma: Option<crate::develop::color_noise::ChromaDenoise>,
    luminance: Option<crate::develop::luminance_noise::LuminanceDenoise>,
}
impl Denoise {
    pub(crate) fn apply(
        &self,
        source: &crate::camera_data::CameraImage,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> anyhow::Result<crate::camera_data::CameraImage> {
        let chroma = self.chroma.map(|d| d.apply(source, cancel)).transpose()?;
        if let Some(luminance) = self.luminance {
            luminance.apply(chroma.as_ref().unwrap_or(source), cancel)
        } else {
            Ok(chroma.unwrap_or_else(|| source.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{camera_data::CameraImage, develop::retouch::RetouchCache};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[test]
    fn denoise_cache_recovers_after_cancellation_and_source_replacement() {
        let source = Arc::new(CameraImage {
            width: 65,
            height: 67,
            pixels: (0..65 * 67)
                .map(|i| [0.1 + (i % 7) as f32 * 0.002; 3])
                .collect(),
            metadata: Default::default(),
            recovered: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        });
        let mut r = Recipe {
            noise_luma: 0.5,
            noise_chroma: 0.,
            ..Default::default()
        };
        let mut cache = RetouchCache::default();
        let cancel = AtomicBool::new(false);
        let first = cache.denoised(&source, r.denoise(), &cancel).unwrap();
        assert!(Arc::ptr_eq(
            &first,
            &cache.denoised(&source, r.denoise(), &cancel).unwrap()
        ));
        r.noise_luma = 1.;
        cancel.store(true, Ordering::Relaxed);
        assert!(cache.denoised(&source, r.denoise(), &cancel).is_err());
        cancel.store(false, Ordering::Relaxed);
        let retried = cache.denoised(&source, r.denoise(), &cancel).unwrap();
        assert_eq!(
            retried.pixels,
            r.denoise().unwrap().apply(&source, &cancel).unwrap().pixels
        );
        let mut replacement = (*source).clone();
        replacement.pixels.fill([0.3; 3]);
        let replacement = Arc::new(replacement);
        let replaced = cache.denoised(&replacement, r.denoise(), &cancel).unwrap();
        assert_eq!(
            replaced.pixels,
            r.denoise()
                .unwrap()
                .apply(&replacement, &cancel)
                .unwrap()
                .pixels
        );
        r.noise_luma = 0.;
        assert!(Arc::ptr_eq(
            &replacement,
            &cache.denoised(&replacement, r.denoise(), &cancel).unwrap()
        ));
    }
}
