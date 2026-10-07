//! The renderer: developing a decoded camera image with a recipe, on the CPU
//! (the reference) and through the GPU preview port. It builds over the model and
//! file formats without LibRaw, the catalog or the GUI, and its tests need no GPU
//! (those that do are ignored).
use rawmakase_interop::xmp;
#[cfg(test)]
use rawmakase_model::xml;
use rawmakase_model::{camera_data, camera_profiles, color, lens, model, optics, rendered};

pub mod develop;
