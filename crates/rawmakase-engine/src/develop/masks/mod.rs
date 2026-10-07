//! Lightroom's Masking panel: masks built from brush, gradient and range components,
//! each with its own local adjustment.
//!
//! Positions are in image space (see [`crate::model::image_frame::ImageFrame`]): normalised to the
//! oriented photo before lens correction, Transform, crop and straightening, so masks
//! stay on the photo when those change. Sizes are fractions of the long edge. The masks
//! themselves are [`crate::model::masks`]'s; this module renders their weights.
mod brush;
mod eval;
pub(crate) mod local;
mod range;

pub use brush::Space;
pub(crate) use eval::{MaskWeights, RasterCache, Selection, Weigher};
pub(crate) use local::{LocalDelta, LocalMath};
pub use range::oklab;

/// Weights (0–1) of mask `index` over a rendered preview, for the mask overlay. `out`
/// is the render: the whole photo at its own size, or `region` of the full-size
/// output. Range components read `out`'s colours. `None` when the mask does not exist
/// or has no components.
pub fn overlay_weights(
    image: &crate::camera_data::CameraImage,
    recipe: &crate::model::recipe::Recipe,
    index: usize,
    out: &crate::rendered::Rendered,
    region: Option<[u32; 4]>,
) -> Option<Vec<f32>> {
    recipe
        .masks
        .get(index)
        .filter(|m| !m.components.is_empty())?;
    let mut g = crate::develop::Geometry::new(image, recipe, 0);
    let region = match region {
        Some(r) if r[2] == out.width && r[3] == out.height => r,
        Some(_) => return None,
        None => {
            // The Fit render's pixels cover the whole output at its own size.
            (g.width, g.height) = (out.width, out.height);
            [0, 0, out.width, out.height]
        }
    };
    let weigher = Weigher::new(image, &recipe.masks, Selection::One(index));
    let w = weigher.weights(image, recipe, &g, region, Some(out));
    Some(w.data.iter().map(|v| *v as f32 / 255.).collect())
}

#[cfg(test)]
mod tests;
