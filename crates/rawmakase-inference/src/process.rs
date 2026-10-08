//! Pure pre- and post-processing: image to model tensor and model matte back to
//! coverage in the input image's own frame. No runtime and no model needed.
//!
//! Resampling is a separable triangle (bilinear) filter whose support widens
//! when shrinking, so a 1536 px input going into a 1024 px tensor is
//! area-filtered rather than aliased. All arithmetic is `f32`; the matte is
//! turned into `u8` only as the very last step.

use crate::error::InferenceError;
use crate::manifest::{Activation, ModelSpec, Resize};

/// The longest side of a returned [`Coverage`].
pub const MAX_COVERAGE_SIDE: usize = 4096;
/// The longest side accepted as input. Larger images are the caller's to
/// downscale first; the cap keeps index arithmetic far from overflow.
pub const MAX_INPUT_SIDE: usize = 16_384;
/// Tolerance for a "probability" output slightly outside 0..=1 from rounding.
const PROBABILITY_SLACK: f32 = 1e-3;

/// 8-bit sRGB, interleaved `R G B`, row-major, `data.len() == width * height * 3`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbImage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// 8-bit coverage (0 = background, 255 = subject), row-major, one byte per
/// pixel, at the aspect ratio of the image it was computed from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// An integer rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// Where the image sits inside the model's square input; the inverse mapping
/// for the output is derived from the same value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub source_width: usize,
    pub source_height: usize,
    pub model_size: usize,
    /// The part of the square that holds image content (all of it when
    /// stretching).
    pub content: Rect,
}

impl Geometry {
    pub fn new(
        source_width: usize,
        source_height: usize,
        spec: &ModelSpec,
    ) -> Result<Self, InferenceError> {
        if source_width == 0 || source_height == 0 {
            return Err(InferenceError::OutputInvalid(
                "the input image is empty".into(),
            ));
        }
        if source_width > MAX_INPUT_SIDE || source_height > MAX_INPUT_SIDE {
            return Err(InferenceError::OutputInvalid(format!(
                "the input image {source_width}x{source_height} exceeds {MAX_INPUT_SIDE} px per side"
            )));
        }
        let size = spec.input_size;
        let content = match spec.resize {
            Resize::Stretch => Rect {
                x: 0,
                y: 0,
                width: size,
                height: size,
            },
            Resize::Letterbox { .. } => {
                let scale = size as f64 / source_width.max(source_height) as f64;
                let width = ((source_width as f64 * scale).round() as usize).clamp(1, size);
                let height = ((source_height as f64 * scale).round() as usize).clamp(1, size);
                Rect {
                    x: (size - width) / 2,
                    y: (size - height) / 2,
                    width,
                    height,
                }
            }
        };
        Ok(Self {
            source_width,
            source_height,
            model_size: size,
            content,
        })
    }

    /// The size of the coverage returned for this input: the input's own size,
    /// scaled down to [`MAX_COVERAGE_SIDE`] if larger.
    pub fn output_size(&self) -> (usize, usize) {
        let longest = self.source_width.max(self.source_height);
        if longest <= MAX_COVERAGE_SIDE {
            return (self.source_width, self.source_height);
        }
        let scale = MAX_COVERAGE_SIDE as f64 / longest as f64;
        let scaled = |v: usize| ((v as f64 * scale).round() as usize).clamp(1, MAX_COVERAGE_SIDE);
        (scaled(self.source_width), scaled(self.source_height))
    }
}

/// Resamples `image` into the model's planar `[1, 3, S, S]` float tensor
/// (returned flat, channel-major) and returns the geometry to undo it.
pub fn preprocess(
    image: &RgbImage,
    spec: &ModelSpec,
) -> Result<(Vec<f32>, Geometry), InferenceError> {
    let geometry = Geometry::new(image.width, image.height, spec)?;
    let expected = image.width * image.height * 3;
    if image.data.len() != expected {
        return Err(InferenceError::OutputInvalid(format!(
            "the input image has {} bytes, expected {expected} for {}x{} RGB",
            image.data.len(),
            image.width,
            image.height
        )));
    }
    let size = spec.input_size;
    let plane_len = size * size;
    let mut tensor = vec![0.0f32; 3 * plane_len];
    let fill = match spec.resize {
        Resize::Letterbox { fill } => fill,
        Resize::Stretch => 0.0,
    };
    let pixels = image.width * image.height;
    let mut plane = vec![0.0f32; pixels];
    for channel in 0..3 {
        for (dst, rgb) in plane.iter_mut().zip(image.data.as_chunks::<3>().0) {
            *dst = f32::from(rgb[channel]) / 255.0;
        }
        let out = &mut tensor[channel * plane_len..(channel + 1) * plane_len];
        let pad = (fill - spec.mean[channel]) / spec.std[channel];
        out.fill(pad);
        let resized = resample(
            &plane,
            image.width,
            Rect {
                x: 0,
                y: 0,
                width: image.width,
                height: image.height,
            },
            geometry.content.width,
            geometry.content.height,
        );
        for (row, line) in resized.chunks_exact(geometry.content.width).enumerate() {
            let start = (geometry.content.y + row) * size + geometry.content.x;
            for (dst, v) in out[start..start + line.len()].iter_mut().zip(line) {
                *dst = (v - spec.mean[channel]) / spec.std[channel];
            }
        }
    }
    Ok((tensor, geometry))
}

/// Validates the model's matte tensor, applies the manifest's activation, undoes
/// the preprocessing mapping and quantizes to 8 bits.
///
/// `shape` is the output tensor's shape as the runtime reports it.
pub fn postprocess(
    raw: &[f32],
    shape: &[i64],
    spec: &ModelSpec,
    geometry: &Geometry,
) -> Result<Coverage, InferenceError> {
    let size = spec.input_size;
    let wanted = [1i64, 1, size as i64, size as i64];
    if shape != wanted {
        return Err(InferenceError::OutputInvalid(format!(
            "output shape {shape:?}, expected {wanted:?}"
        )));
    }
    if raw.len() != size * size {
        return Err(InferenceError::OutputInvalid(format!(
            "output has {} values, expected {}",
            raw.len(),
            size * size
        )));
    }
    let mut probability = Vec::with_capacity(raw.len());
    for &v in raw {
        if !v.is_finite() {
            return Err(InferenceError::OutputInvalid(
                "the output contains NaN or infinity".into(),
            ));
        }
        let p = match spec.activation {
            Activation::Logit => sigmoid(v),
            Activation::Probability => {
                if !(-PROBABILITY_SLACK..=1.0 + PROBABILITY_SLACK).contains(&v) {
                    return Err(InferenceError::OutputInvalid(format!(
                        "the output value {v} is outside 0..=1 for a probability output"
                    )));
                }
                v.clamp(0.0, 1.0)
            }
        };
        probability.push(p);
    }
    let (width, height) = geometry.output_size();
    let matte = resample(&probability, size, geometry.content, width, height);
    let data = matte
        .iter()
        .map(|&p| (p.clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect();
    Ok(Coverage {
        width,
        height,
        data,
    })
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Per-output-coordinate filter taps: the first source index and the weights.
struct Taps {
    first: usize,
    weights: Vec<f32>,
}

/// Triangle-filter taps mapping `dst_len` outputs onto `src_len` source samples
/// starting at `src_start`. The filter support grows with the shrink factor.
fn taps(src_start: usize, src_len: usize, dst_len: usize) -> Vec<Taps> {
    let scale = src_len as f64 / dst_len as f64;
    let filter_scale = scale.max(1.0);
    let support = filter_scale;
    (0..dst_len)
        .map(|i| {
            let center = (i as f64 + 0.5) * scale;
            let lo = ((center - support + 0.5).floor().max(0.0)) as usize;
            let hi = (((center + support + 0.5).floor()) as usize).min(src_len);
            let lo = lo.min(hi.saturating_sub(1));
            let mut weights: Vec<f32> = (lo..hi)
                .map(|x| {
                    let d = ((x as f64 + 0.5 - center) / filter_scale).abs();
                    (1.0 - d).max(0.0) as f32
                })
                .collect();
            let sum: f32 = weights.iter().sum();
            if sum > 0.0 {
                weights.iter_mut().for_each(|w| *w /= sum);
            } else {
                // Cannot happen for a triangle with these bounds; stay safe.
                weights = vec![1.0 / weights.len().max(1) as f32; weights.len().max(1)];
            }
            Taps {
                first: src_start + lo,
                weights,
            }
        })
        .collect()
}

/// Resamples the `window` of a single-channel `src` plane (row stride `stride`)
/// to `dst_width` x `dst_height`.
fn resample(
    src: &[f32],
    stride: usize,
    window: Rect,
    dst_width: usize,
    dst_height: usize,
) -> Vec<f32> {
    let xs = taps(window.x, window.width, dst_width);
    let ys = taps(window.y, window.height, dst_height);
    let first_row = ys.iter().map(|t| t.first).min().unwrap_or(window.y);
    let last_row = ys
        .iter()
        .map(|t| t.first + t.weights.len())
        .max()
        .unwrap_or(window.y);
    // Horizontal pass over only the rows the vertical pass will read.
    let mut rows = vec![0.0f32; (last_row - first_row) * dst_width];
    for (r, out_row) in rows.chunks_exact_mut(dst_width).enumerate() {
        let line = &src[(first_row + r) * stride..(first_row + r + 1) * stride];
        for (out, tap) in out_row.iter_mut().zip(&xs) {
            *out = tap
                .weights
                .iter()
                .zip(&line[tap.first..])
                .map(|(w, v)| w * v)
                .sum();
        }
    }
    let mut dst = vec![0.0f32; dst_width * dst_height];
    for (out_row, tap) in dst.chunks_exact_mut(dst_width).zip(&ys) {
        for (w, r) in tap.weights.iter().zip(tap.first - first_row..) {
            let row = &rows[r * dst_width..(r + 1) * dst_width];
            for (o, v) in out_row.iter_mut().zip(row) {
                *o += w * v;
            }
        }
    }
    dst
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::SUBJECT;

    fn letterbox_spec() -> ModelSpec {
        ModelSpec {
            input_size: 16,
            resize: Resize::Letterbox { fill: 0.5 },
            mean: [0.5; 3],
            std: [0.5; 3],
            ..SUBJECT
        }
    }

    fn small_stretch_spec() -> ModelSpec {
        ModelSpec {
            input_size: 8,
            ..SUBJECT
        }
    }

    fn solid(width: usize, height: usize, rgb: [u8; 3]) -> RgbImage {
        RgbImage {
            width,
            height,
            data: rgb
                .iter()
                .copied()
                .cycle()
                .take(width * height * 3)
                .collect(),
        }
    }

    #[test]
    fn stretch_content_is_the_whole_square() {
        let g = Geometry::new(300, 100, &small_stretch_spec()).unwrap();
        assert_eq!(
            g.content,
            Rect {
                x: 0,
                y: 0,
                width: 8,
                height: 8
            }
        );
    }

    #[test]
    fn letterbox_centres_a_wide_image() {
        let g = Geometry::new(400, 100, &letterbox_spec()).unwrap();
        assert_eq!(
            g.content,
            Rect {
                x: 0,
                y: 6,
                width: 16,
                height: 4
            }
        );
        let g = Geometry::new(100, 400, &letterbox_spec()).unwrap();
        assert_eq!(
            g.content,
            Rect {
                x: 6,
                y: 0,
                width: 4,
                height: 16
            }
        );
    }

    #[test]
    fn letterbox_never_collapses_to_zero() {
        let g = Geometry::new(1000, 1, &letterbox_spec()).unwrap();
        assert_eq!(g.content.height, 1);
        assert_eq!(g.content.width, 16);
    }

    #[test]
    fn output_keeps_input_size_up_to_the_cap() {
        let g = Geometry::new(1536, 1024, &SUBJECT).unwrap();
        assert_eq!(g.output_size(), (1536, 1024));
        let g = Geometry::new(8192, 4096, &SUBJECT).unwrap();
        assert_eq!(g.output_size(), (4096, 2048));
        let g = Geometry::new(4097, 3, &SUBJECT).unwrap();
        assert_eq!(g.output_size(), (4096, 3));
    }

    #[test]
    fn rejects_empty_oversized_and_malformed_inputs() {
        let spec = small_stretch_spec();
        assert!(Geometry::new(0, 5, &spec).is_err());
        assert!(Geometry::new(MAX_INPUT_SIDE + 1, 5, &spec).is_err());
        let bad = RgbImage {
            width: 2,
            height: 2,
            data: vec![0; 11],
        };
        assert!(matches!(
            preprocess(&bad, &spec),
            Err(InferenceError::OutputInvalid(_))
        ));
    }

    #[test]
    fn normalization_follows_mean_and_std() {
        let spec = ModelSpec {
            mean: [0.25, 0.5, 0.75],
            std: [0.5, 0.25, 1.0],
            ..small_stretch_spec()
        };
        let (tensor, _) = preprocess(&solid(5, 3, [255, 0, 255]), &spec).unwrap();
        let plane = 8 * 8;
        assert_eq!(tensor.len(), 3 * plane);
        assert!(tensor[..plane].iter().all(|&v| (v - 1.5).abs() < 1e-5));
        assert!(
            tensor[plane..2 * plane]
                .iter()
                .all(|&v| (v + 2.0).abs() < 1e-5)
        );
        assert!(tensor[2 * plane..].iter().all(|&v| (v - 0.25).abs() < 1e-5));
    }

    #[test]
    fn channels_are_planar_not_interleaved() {
        let spec = ModelSpec {
            mean: [0.0; 3],
            std: [1.0; 3],
            ..small_stretch_spec()
        };
        let (tensor, _) = preprocess(&solid(4, 4, [255, 0, 0]), &spec).unwrap();
        assert!(tensor[..64].iter().all(|&v| (v - 1.0).abs() < 1e-5));
        assert!(tensor[64..].iter().all(|&v| v.abs() < 1e-5));
    }

    #[test]
    fn letterbox_pads_with_the_normalized_fill() {
        let spec = letterbox_spec();
        let (tensor, g) = preprocess(&solid(32, 8, [255, 255, 255]), &spec).unwrap();
        // White is (1 - 0.5) / 0.5 = 1; the 0.5 fill normalizes to exactly 0.
        let at = |x: usize, y: usize| tensor[y * 16 + x];
        assert_eq!(
            g.content,
            Rect {
                x: 0,
                y: 6,
                width: 16,
                height: 4
            }
        );
        assert!(at(0, 0).abs() < 1e-6 && at(15, 15).abs() < 1e-6);
        assert!((at(8, 7) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn downscale_averages_instead_of_aliasing() {
        // Alternating columns of 0 and 255: a point sample would give 0 or 1,
        // an area filter gives about one half.
        let mut data = Vec::new();
        for _y in 0..4 {
            for x in 0..16 {
                let v = if x % 2 == 0 { 0 } else { 255 };
                data.extend_from_slice(&[v, v, v]);
            }
        }
        let spec = ModelSpec {
            mean: [0.0; 3],
            std: [1.0; 3],
            ..small_stretch_spec()
        };
        let (tensor, _) = preprocess(
            &RgbImage {
                width: 16,
                height: 4,
                data,
            },
            &spec,
        )
        .unwrap();
        // Interior columns only: the first and last renormalize at the edge.
        assert!(
            tensor[1..7].iter().all(|v| (v - 0.5).abs() < 0.02),
            "{:?}",
            &tensor[..8]
        );
    }

    fn matte_spec(activation: Activation, resize: Resize) -> ModelSpec {
        ModelSpec {
            input_size: 8,
            activation,
            resize,
            ..SUBJECT
        }
    }

    fn shape8() -> [i64; 4] {
        [1, 1, 8, 8]
    }

    #[test]
    fn stretch_round_trip_restores_input_aspect() {
        let spec = matte_spec(Activation::Probability, Resize::Stretch);
        let g = Geometry::new(24, 6, &spec).unwrap();
        // Left half foreground in model space.
        let raw: Vec<f32> = (0..64).map(|i| if i % 8 < 4 { 1.0 } else { 0.0 }).collect();
        let c = postprocess(&raw, &shape8(), &spec, &g).unwrap();
        assert_eq!((c.width, c.height), (24, 6));
        assert_eq!(c.data[0], 255);
        assert_eq!(c.data[23], 0);
        assert_eq!(c.data[5 * 24 + 2], 255);
    }

    #[test]
    fn letterbox_inverse_crops_the_padding_exactly() {
        let spec = matte_spec(Activation::Probability, Resize::Letterbox { fill: 0.0 });
        // 16x4 into 8x8: content is rows 3..5 (2 rows tall), full width.
        let g = Geometry::new(16, 4, &spec).unwrap();
        assert_eq!(
            g.content,
            Rect {
                x: 0,
                y: 3,
                width: 8,
                height: 2
            }
        );
        // Padding says 1 (foreground), content says 0: the output must be all
        // 0, proving none of the padding leaks into the result.
        let mut raw = vec![1.0f32; 64];
        raw[3 * 8..5 * 8].fill(0.0);
        let c = postprocess(&raw, &shape8(), &spec, &g).unwrap();
        assert_eq!((c.width, c.height), (16, 4));
        assert!(c.data.iter().all(|&v| v == 0), "{:?}", c.data);
        // And the converse: content 1 inside padding 0.
        let mut raw = vec![0.0f32; 64];
        raw[3 * 8..5 * 8].fill(1.0);
        let c = postprocess(&raw, &shape8(), &spec, &g).unwrap();
        assert!(c.data.iter().all(|&v| v == 255), "{:?}", c.data);
    }

    #[test]
    fn logits_go_through_a_sigmoid() {
        let spec = matte_spec(Activation::Logit, Resize::Stretch);
        let g = Geometry::new(8, 8, &spec).unwrap();
        let mut raw = vec![-20.0f32; 64];
        raw[0] = 0.0;
        raw[1] = 20.0;
        let c = postprocess(&raw, &shape8(), &spec, &g).unwrap();
        assert_eq!(c.data[0], 128);
        assert_eq!(c.data[1], 255);
        assert_eq!(c.data[2], 0);
    }

    #[test]
    fn output_is_not_min_max_normalized() {
        let spec = matte_spec(Activation::Probability, Resize::Stretch);
        let g = Geometry::new(8, 8, &spec).unwrap();
        let c = postprocess(&[0.2f32; 64], &shape8(), &spec, &g).unwrap();
        assert!(c.data.iter().all(|&v| v == 51));
    }

    #[test]
    fn rejects_nan_infinity_range_and_shape() {
        let spec = matte_spec(Activation::Probability, Resize::Stretch);
        let g = Geometry::new(8, 8, &spec).unwrap();
        let mut raw = vec![0.5f32; 64];
        raw[10] = f32::NAN;
        assert!(postprocess(&raw, &shape8(), &spec, &g).is_err());
        raw[10] = f32::INFINITY;
        assert!(postprocess(&raw, &shape8(), &spec, &g).is_err());
        raw[10] = 1.5;
        assert!(postprocess(&raw, &shape8(), &spec, &g).is_err());
        raw[10] = -0.5;
        assert!(postprocess(&raw, &shape8(), &spec, &g).is_err());
        let ok = vec![0.5f32; 64];
        assert!(postprocess(&ok, &[1, 1, 8, 7], &spec, &g).is_err());
        assert!(postprocess(&ok, &[1, 8, 8], &spec, &g).is_err());
        assert!(postprocess(&ok[..63], &shape8(), &spec, &g).is_err());
        let logit = matte_spec(Activation::Logit, Resize::Stretch);
        let mut raw = vec![0.0f32; 64];
        raw[0] = f32::NEG_INFINITY;
        assert!(postprocess(&raw, &shape8(), &logit, &g).is_err());
    }

    #[test]
    fn tiny_probability_overshoot_is_clamped() {
        let spec = matte_spec(Activation::Probability, Resize::Stretch);
        let g = Geometry::new(8, 8, &spec).unwrap();
        let c = postprocess(&[1.0005f32; 64], &shape8(), &spec, &g).unwrap();
        assert!(c.data.iter().all(|&v| v == 255));
    }

    #[test]
    fn upscaling_a_matte_is_smooth_and_bounded() {
        let spec = matte_spec(Activation::Probability, Resize::Stretch);
        let g = Geometry::new(64, 64, &spec).unwrap();
        let raw: Vec<f32> = (0..64).map(|i| if i % 8 < 4 { 1.0 } else { 0.0 }).collect();
        let c = postprocess(&raw, &shape8(), &spec, &g).unwrap();
        let row = &c.data[..64];
        assert!(
            row.windows(2).all(|w| w[0] >= w[1]),
            "monotone edge: {row:?}"
        );
        assert!(
            row[31] > 0 && row[31] < 255 || row[32] > 0 && row[32] < 255,
            "soft transition"
        );
    }
}
