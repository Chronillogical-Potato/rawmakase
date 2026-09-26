//! Stateful desktop preview backend. Export remains on the reference CPU path.
use super::{Geometry, Recipe, Rendered, gpu, pyramid::Pyramid, quality, stage_cache::StageCache};
use crate::raw::CameraImage;
use anyhow::Result;
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Default)]
pub struct PreviewRenderer {
    pub(crate) gpu: Option<gpu::Processor>,
    fallback: Option<String>,
    used_gpu: bool,
    /// Resolution pyramid of the current photo's recovered image, for Fit and
    /// zoomed-out renders. Holding the recovered image keeps its identity unique.
    pyramid: Option<Pyramid>,
    /// Stage results reused while only color and tone change.
    cache: StageCache,
}
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
        if region.is_none()
            && max_edge > 0
            && let Some(out) = self.render_fit(image, recipe, max_edge, cancel)?
        {
            return Ok(out);
        }
        let mut cache = std::mem::take(&mut self.cache);
        let out = quality::render_preview(
            image,
            recipe,
            max_edge,
            region,
            cancel,
            self,
            Some(&mut cache),
        );
        self.cache = cache;
        out
    }
    /// Fit and zoomed-out views from the smallest pyramid level with at least one
    /// pixel per output pixel. `None` when the output is the full resolution.
    fn render_fit(
        &mut self,
        image: &CameraImage,
        recipe: &Recipe,
        max_edge: u32,
        cancel: &AtomicBool,
    ) -> Result<Option<Rendered>> {
        let full = Geometry::new(image, recipe, 0);
        let long = full.width.max(full.height);
        if long <= max_edge {
            return Ok(None);
        }
        let source = quality::recovered(image, cancel)?;
        let pyramid = self.pyramid_for(source);
        let size = quality::output_size(full.width, full.height, max_edge);
        let needed = size.0.max(size.1) as f32 * image.width.max(image.height) as f32 / long as f32;
        let level = pyramid.level_for(needed);
        let source = pyramid.source().clone();
        quality::render_level(&level, &source, recipe, size, cancel, &mut self.cache).map(Some)
    }
    fn pyramid_for(&mut self, source: Arc<CameraImage>) -> &mut Pyramid {
        let current = self.pyramid.as_ref();
        if !current.is_some_and(|p| Arc::ptr_eq(p.source(), &source)) {
            self.pyramid = Some(Pyramid::new(source));
        }
        self.pyramid.as_mut().unwrap()
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
    fn fit_uses_a_pyramid_level_and_never_reuses_another_photo() {
        let mut p = PreviewRenderer::default();
        let r = Recipe::default();
        let cancel = AtomicBool::new(false);
        let a = image(400, 300, 0.2);
        let fit = p.render(&a, &r, 60, None, &cancel).unwrap();
        assert_eq!((fit.width, fit.height), (60, 45));
        let source = p.pyramid.as_ref().unwrap().source().clone();
        assert!(Arc::ptr_eq(&source, a.recovered.get().unwrap()));
        assert_eq!(p.pyramid.as_mut().unwrap().level_for(60.).width, 100);
        let b = image(400, 300, 0.6);
        let other = p.render(&b, &r, 60, None, &cancel).unwrap();
        assert_ne!(fit.pixels, other.pixels);
        assert!(!Arc::ptr_eq(p.pyramid.as_ref().unwrap().source(), &source));
        // Regions and full-size views always use the full image.
        let region = p.render(&a, &r, 0, Some([0, 0, 10, 10]), &cancel).unwrap();
        assert_eq!(region.width, 10);
        let full = p.render(&a, &r, 400, None, &cancel).unwrap();
        assert_eq!(
            full.pixels,
            quality::render(&a, &r, 400, None).unwrap().pixels
        );
    }
    /// A textured photo with detail, local and spatial effects: the pyramid Fit stays
    /// close to the export render resized to the same size.
    #[test]
    fn fit_approximates_the_resized_export() {
        let (w, h) = (640, 424);
        let mut im = image(w, h, 0.);
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            let v = 0.25 + 0.2 * (x * 0.05).sin() * (y * 0.031).cos() + 0.05 * (x * 0.9).sin();
            *p = [v * 1.1, v, v * 0.7 + x / w as f32 * 0.2];
        }
        let mut r = Recipe {
            sharpening: 0.8,
            exposure: 0.4,
            straighten: 2.,
            crop: [0.05, 0.1, 0.95, 0.9],
            ..Default::default()
        };
        r.effects.clarity = 0.4;
        r.effects.texture = 0.3;
        r.effects.vignette = -0.4;
        r.effects.grain = 0.3;
        let cancel = AtomicBool::new(false);
        for edge in [100, 180, 300] {
            let expected = quality::render(&im, &r, edge, None).unwrap();
            let fit = PreviewRenderer::default()
                .render(&im, &r, edge, None, &cancel)
                .unwrap();
            assert_eq!((fit.width, fit.height), (expected.width, expected.height));
            let error = fit
                .pixels
                .iter()
                .flatten()
                .zip(expected.pixels.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .sum::<f32>()
                / (fit.pixels.len() * 3) as f32;
            assert!(error < 0.01, "edge {edge}: mean error {error}");
        }
    }
    /// Every edit, including ones that change only cached stages, must render exactly
    /// as a renderer without cached state would.
    #[test]
    fn cached_stages_never_serve_a_stale_result() {
        let (w, h) = (300, 200);
        let mut im = image(w, h, 0.);
        for (i, p) in im.pixels.iter_mut().enumerate() {
            let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
            let v = 0.3 + 0.2 * (x * 0.07).sin() * (y * 0.05).cos();
            *p = [v * 1.2, v, v * 0.6];
        }
        let cancel = AtomicBool::new(false);
        let mut warm = PreviewRenderer::default();
        let mut r = Recipe {
            shadows: 0.3,
            ..Default::default()
        };
        r.effects.clarity = 0.3;
        let edits: [&dyn Fn(&mut Recipe); 9] = [
            &|_| {},
            &|r| r.exposure = 0.5,
            &|r| r.effects.clarity = -0.2,
            &|r| r.effects.texture = 0.4,
            &|r| r.wb[0] = 1.3,
            &|r| r.crop = [0.1, 0., 0.9, 1.],
            &|r| r.noise_luma = 0.4,
            &|r| r.contrast = 0.3,
            &|r| r.engine = 3,
        ];
        for edit in edits {
            edit(&mut r);
            for (edge, region) in [(80, None), (0, Some([20, 30, 50, 40])), (150, None)] {
                let cached = warm.render(&im, &r, edge, region, &cancel).unwrap();
                let fresh = PreviewRenderer::default()
                    .render(&im, &r, edge, region, &cancel)
                    .unwrap();
                assert_eq!(cached.pixels, fresh.pixels, "{r:?} {edge} {region:?}");
            }
        }
        assert_eq!(warm.cache.samples.len(), 2);
    }
}
