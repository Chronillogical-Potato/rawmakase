//! Results of the stages before the per-pixel color pipeline, kept between preview
//! renders: local-tone blurs, the local-tone image, geometry/lens-warp samples and the
//! reduced image the engine 4 Shadows/Highlights map is built from.
//! Each key holds only the recipe fields its stage reads, so exposure, curve, HSL
//! and grading edits reuse all three and rerun only the per-pixel stage.
//!
//! When a stage starts reading another recipe field, add it to that stage's key here.
use super::{
    Geometry, Recipe,
    effects::Effects,
    pipeline::{Samples, Toned},
    quality::LocalBlurs,
};
use crate::raw::CameraImage;
use anyhow::Result;
use std::sync::Arc;

/// Most recent entries kept per stage: Fit, the quick Fit shown when leaving 100%, and
/// the 100% view's reduced preview and full region, so switching views reuses them.
const ENTRIES: usize = 4;
/// Byte budget per stage. Larger results are computed but not kept.
const BUDGET: usize = 512 << 20;

#[derive(Default)]
pub(crate) struct StageCache {
    pub(crate) blurs: Lru<BlurKey, LocalBlurs>,
    pub(crate) local: Lru<LocalKey, Vec<f32>>,
    pub(crate) samples: Lru<SampleKey, Samples>,
    pub(crate) reduced: Lru<ReducedKey, CameraImage>,
    /// Mask weights of a region.
    pub(crate) masks: Lru<MaskKey, super::masks::MaskWeights>,
    /// Brush masks rasterised in image space.
    pub(crate) rasters: super::masks::RasterCache,
}

pub(crate) struct Lru<K, V> {
    /// Most recently used first.
    entries: Vec<(K, Arc<V>, usize)>,
}
impl<K, V> Default for Lru<K, V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}
impl<K: PartialEq, V> Lru<K, V> {
    pub(crate) fn get_or_try(
        &mut self,
        key: K,
        bytes: impl Fn(&V) -> usize,
        make: impl FnOnce() -> Result<V>,
    ) -> Result<Arc<V>> {
        if let Some(i) = self.entries.iter().position(|(k, ..)| *k == key) {
            let entry = self.entries.remove(i);
            let value = entry.1.clone();
            self.entries.insert(0, entry);
            return Ok(value);
        }
        let value = Arc::new(make()?);
        let size = bytes(&value);
        if size <= BUDGET {
            self.entries.insert(0, (key, value.clone(), size));
            self.entries.truncate(ENTRIES);
            while self.entries.iter().map(|e| e.2).sum::<usize>() > BUDGET {
                self.entries.pop();
            }
        }
        Ok(value)
    }
    /// The value for `key`, made most recently used.
    pub(crate) fn get(&mut self, key: &K) -> Option<Arc<V>> {
        let i = self.entries.iter().position(|(k, ..)| k == key)?;
        let entry = self.entries.remove(i);
        let value = entry.1.clone();
        self.entries.insert(0, entry);
        Some(value)
    }
    pub(crate) fn insert(&mut self, key: K, value: Arc<V>, size: usize) {
        if size <= BUDGET {
            self.entries.retain(|(k, ..)| *k != key);
            self.entries.insert(0, (key, value, size));
            self.entries.truncate(ENTRIES);
            while self.entries.iter().map(|e| e.2).sum::<usize>() > BUDGET {
                self.entries.pop();
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Identity of a shared value. Keys hold the value, so its address stays unique.
pub(crate) struct Same<T>(Arc<T>);
impl<T> Clone for Same<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}
impl<T> PartialEq for Same<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

/// Local-tone blurs: log luminance after white balance, profile matrix and lens
/// vignetting, before exposure.
#[derive(Clone, PartialEq)]
pub(crate) struct BlurKey {
    image: Same<CameraImage>,
    scale: u32,
    texture: bool,
    recipe: Recipe,
}
impl BlurKey {
    pub(crate) fn new(image: &Arc<CameraImage>, r: &Recipe, scale: f32, texture: bool) -> Self {
        Self {
            image: Same(image.clone()),
            scale: scale.to_bits(),
            texture,
            recipe: Recipe {
                engine: r.engine,
                wb: r.wb,
                temperature: r.temperature,
                profile: r.profile.clone(),
                lens_builtin: r.lens_builtin,
                lens_profile: r.lens_profile,
                lens_vignetting: r.lens_vignetting,
                ..Default::default()
            },
        }
    }
}
/// The local-tone gain: blurs plus the sliders applied to them. Exposure only
/// matters to Shadows and Highlights. Keyed by the blurs' inputs rather than the blurs
/// themselves, so the gain is found again even when the blurs were too large to keep
/// (a 61-megapixel photo's), and exposure edits at 100% do not recompute them.
#[derive(Clone, PartialEq)]
pub(crate) struct LocalKey {
    blurs: BlurKey,
    sliders: [u32; 5],
}
impl LocalKey {
    pub(crate) fn new(blurs: BlurKey, r: &Recipe) -> Self {
        let exposure = if r.shadows != 0. || r.highlights != 0. {
            r.exposure + r.camera_exposure
        } else {
            0.
        };
        Self {
            blurs,
            sliders: [
                exposure,
                r.shadows,
                r.highlights,
                r.effects.clarity,
                r.effects.texture,
            ]
            .map(f32::to_bits),
        }
    }
}
/// The toned image reduced for the engine 4 Shadows/Highlights map: the camera image
/// and its local-tone gain.
#[derive(PartialEq)]
pub(crate) struct ReducedKey {
    image: Same<CameraImage>,
    gain: Option<LocalKey>,
}
impl ReducedKey {
    pub(crate) fn new(toned: &Toned) -> Self {
        Self {
            image: Same(toned.image.clone()),
            gain: toned.gain_key.clone(),
        }
    }
}
/// Samples of an output region: geometry, lens correction and noise reduction.
#[derive(PartialEq)]
pub(crate) struct SampleKey {
    image: Same<CameraImage>,
    gain: Option<LocalKey>,
    size: [u32; 2],
    region: [u32; 4],
    spread: u32,
    recipe: Recipe,
}
impl SampleKey {
    pub(crate) fn new(
        toned: &Toned,
        r: &Recipe,
        g: &Geometry,
        region: [u32; 4],
        spread: f32,
    ) -> Self {
        let e = &r.effects;
        Self {
            image: Same(toned.image.clone()),
            gain: toned.gain_key.clone(),
            size: [g.width, g.height],
            region,
            spread: spread.to_bits(),
            recipe: Recipe {
                engine: r.engine,
                crop: r.crop,
                rotation: r.rotation,
                straighten: r.straighten,
                flip_x: r.flip_x,
                flip_y: r.flip_y,
                transform: r.transform,
                lens_builtin: r.lens_builtin,
                lens_profile: r.lens_profile,
                lens_distortion: r.lens_distortion,
                lens_vignetting: r.lens_vignetting,
                noise_luma: r.noise_luma,
                noise_chroma: r.noise_chroma,
                effects: Effects {
                    luma_detail: e.luma_detail,
                    luma_contrast: e.luma_contrast,
                    chroma_detail: e.chroma_detail,
                    chroma_smoothness: e.chroma_smoothness,
                    ..Default::default()
                },
                ..Default::default()
            },
        }
    }
}

/// Mask weights of a region: where its samples come from, and the shapes of the
/// masks that render (not their sliders, which apply later). Range components read
/// the developed colours, so then the rest of the recipe counts too.
#[derive(PartialEq)]
pub(crate) struct MaskKey {
    samples: SampleKey,
    masks: Vec<(Vec<super::masks::MaskComponent>, bool)>,
    recipe: Option<Recipe>,
}
impl MaskKey {
    pub(crate) fn new(
        toned: &Toned,
        r: &Recipe,
        g: &Geometry,
        region: [u32; 4],
        spread: f32,
        ranges: bool,
    ) -> Self {
        Self {
            samples: SampleKey::new(toned, r, g, region, spread),
            masks: r
                .masks
                .iter()
                .filter(|m| m.is_active())
                .map(|m| (m.components.clone(), m.invert))
                .collect(),
            recipe: ranges.then(|| Recipe {
                masks: Vec::new(),
                ..r.clone()
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lru_keeps_recent_entries_within_budget() -> Result<()> {
        let mut lru: Lru<u32, Vec<u8>> = Lru::default();
        let mut made = 0;
        let mut get = |lru: &mut Lru<u32, Vec<u8>>, key: u32, size: usize| {
            lru.get_or_try(key, Vec::len, || {
                made += 1;
                Ok(vec![0; size])
            })
            .map(|_| made)
        };
        let n = ENTRIES as u32;
        for key in 1..=n {
            assert_eq!(get(&mut lru, key, 10)?, key);
        }
        // Hits make nothing; key 1 becomes the most recently used.
        assert_eq!(get(&mut lru, 1, 10)?, n);
        // A new key evicts the least recently used one, key 2.
        assert_eq!(get(&mut lru, n + 1, 10)?, n + 1);
        assert_eq!(get(&mut lru, 1, 10)?, n + 1);
        assert_eq!(get(&mut lru, 2, 10)?, n + 2);
        // Oversized results are returned but not kept.
        assert_eq!(get(&mut lru, n + 2, BUDGET + 1)?, n + 3);
        assert_eq!(lru.len(), ENTRIES);
        assert!(
            lru.get_or_try(0, Vec::len, || anyhow::bail!("cancelled"))
                .is_err()
        );
        assert_eq!(lru.len(), ENTRIES);
        Ok(())
    }
}
