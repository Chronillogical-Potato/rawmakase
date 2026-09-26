//! Stateful desktop preview backend. Export remains on the reference CPU path.
use super::{Recipe, Rendered, gpu, quality};
use crate::raw::CameraImage;
use anyhow::Result;
use std::sync::atomic::AtomicBool;

#[derive(Default)]
pub struct PreviewRenderer {
    pub(crate) gpu: Option<gpu::Processor>,
    fallback: Option<String>,
    used_gpu: bool,
    /// Reduced camera image for Fit renders, keyed by source identity and size.
    reduced: Option<(ReducedKey, std::sync::Arc<CameraImage>)>,
}
#[derive(PartialEq)]
struct ReducedKey {
    pixels: usize,
    width: u32,
    height: u32,
    edge: u32,
    /// Sampled pixel bits, so a new photo reusing a freed allocation never matches.
    sample: u64,
}
fn sample_bits(im: &CameraImage) -> u64 {
    let n = im.pixels.len().max(1);
    (0..97).fold(0xcbf2_9ce4_8422_2325u64, |h, i| {
        let p = im.pixels.get(i * n / 97).copied().unwrap_or_default();
        p.iter().fold(h, |h, v| {
            (h ^ v.to_bits() as u64).wrapping_mul(0x100_0000_01b3)
        })
    })
}
/// Fit previews are rendered from a camera image reduced to this multiple of the
/// output size, so detail processing still has headroom while cost falls with area.
const FIT_SUPERSAMPLE: u32 = 2;
impl PreviewRenderer {
    /// A hardware device is optional; failure leaves a fully working CPU renderer.
    pub fn with_gpu() -> Self {
        match gpu::Processor::new() {
            Ok(gpu) => Self {
                gpu: Some(gpu),
                ..Self::default()
            },
            Err(error) => Self {
                fallback: Some(error.to_string()),
                ..Self::default()
            },
        }
    }
    pub fn adapter_name(&self) -> Option<&str> {
        self.gpu.as_ref().map(gpu::Processor::name)
    }
    pub fn fallback_reason(&self) -> Option<&str> {
        self.fallback.as_deref()
    }
    pub fn used_gpu(&self) -> bool {
        self.used_gpu
    }
    pub fn render(
        &mut self,
        image: &CameraImage,
        recipe: &Recipe,
        max_edge: u32,
        region: Option<[u32; 4]>,
        cancel: &AtomicBool,
    ) -> Result<Rendered> {
        self.used_gpu = false;
        anyhow::ensure!(
            !cancel.load(std::sync::atomic::Ordering::Relaxed),
            "Render superseded"
        );
        if recipe.engine < 3 {
            return match region {
                Some(region) => super::render_region_legacy(image, recipe, region),
                None => super::render_legacy(image, recipe, max_edge),
            };
        }
        let edge = max_edge.saturating_mul(FIT_SUPERSAMPLE);
        if region.is_none() && max_edge > 0 && image.width.max(image.height) > edge + edge / 4 {
            let key = ReducedKey {
                pixels: image.pixels.as_ptr() as usize,
                width: image.width,
                height: image.height,
                edge,
                sample: sample_bits(image),
            };
            let reduced = match &self.reduced {
                Some((k, im)) if *k == key => im.clone(),
                _ => {
                    let im = std::sync::Arc::new(super::preview(image, edge));
                    self.reduced = Some((key, im.clone()));
                    im
                }
            };
            return quality::render_preview(&reduced, recipe, max_edge, None, cancel, self);
        }
        quality::render_preview(image, recipe, max_edge, region, cancel, self)
    }
    pub(crate) fn finish(
        &mut self,
        image: &Rendered,
        recipe: &Recipe,
        max_edge: u32,
        cancel: &AtomicBool,
    ) -> Option<Rendered> {
        let gpu = self.gpu.as_mut()?;
        match gpu.finish(image, recipe, max_edge, cancel) {
            Ok(out) => {
                self.used_gpu = true;
                Some(out)
            }
            Err(error) => {
                if !cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    self.fallback = Some(error.to_string());
                    self.gpu = None; // Do not retry a failing device on every slider movement.
                }
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(width: u32, height: u32, value: f32) -> CameraImage {
        CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: (0..width * height)
                .map(|i| [value + (i % 7) as f32 * 0.01, value, value * 0.5])
                .collect(),
            metadata: crate::raw::Metadata {
                width,
                height,
                wb: [1.; 3],
                daylight_wb: [1.; 3],
                matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
                ..Default::default()
            },
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        }
    }
    #[test]
    fn fit_uses_reduced_image_and_never_reuses_another_photo() {
        let mut p = PreviewRenderer::default();
        let r = Recipe::default();
        let cancel = AtomicBool::new(false);
        let a = image(400, 300, 0.2);
        let fit = p.render(&a, &r, 60, None, &cancel).unwrap();
        assert_eq!(fit.width.max(fit.height), 60);
        assert_eq!(p.reduced.as_ref().unwrap().1.width, 120);
        let b = image(400, 300, 0.6);
        let other = p.render(&b, &r, 60, None, &cancel).unwrap();
        assert_ne!(fit.pixels, other.pixels);
        // Regions always use the full image.
        let region = p.render(&a, &r, 0, Some([0, 0, 10, 10]), &cancel).unwrap();
        assert_eq!(region.width, 10);
    }
}
