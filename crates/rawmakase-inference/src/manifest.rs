//! The pinned contract of the selection models.
//!
//! Everything a pre/post-processing step or an installer needs to know about them lives
//! here; nothing else in the crate hard-codes a size, a tensor name or a normalization
//! constant. Two models work together: [`SUBJECT`] (Segment Anything 2) draws crisp object
//! masks and answers clicks, and [`SALIENCY`] (IS-Net) says which objects are the photo's
//! subject.

/// One file of the models, as stored under their folder.
#[derive(Debug, Clone, Copy)]
pub struct ModelFile {
    pub name: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
    /// Where it is fetched from: pinned to an immutable revision or tag, and checked
    /// against `sha256` whatever the host sends.
    pub url: &'static str,
}

/// The interactive model's contract: identity, artifacts, tensors and preprocessing.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub id: &'static str,
    /// Version of this contract (bumped when anything below changes in a way that can
    /// change the produced mask, including the pre/post-processing code).
    pub version: u32,
    /// Every file the model needs. A graph's weights sit in the `_data` file beside it
    /// and are read from there by name.
    pub files: &'static [ModelFile],
    pub encoder_file: &'static str,
    pub decoder_file: &'static str,
    /// Square side the image encoder is fed: the photo is stretched to it.
    pub input_size: usize,
    /// Side of the low-resolution mask logits the decoder returns.
    pub mask_size: usize,
    /// Per-channel mean/std applied to RGB scaled to 0..=1: `(v - mean) / std`.
    pub mean: [f32; 3],
    pub std: [f32; 3],
    /// Version of the pre/post-processing code paths in this crate (resampling filters,
    /// edge refinement, automatic selection). Part of any cache key.
    pub processing_version: u32,
    pub license: &'static str,
    pub attribution: &'static str,
}

/// The saliency model's contract: one graph, a square input, a probability matte out.
#[derive(Debug, Clone, Copy)]
pub struct SaliencySpec {
    pub id: &'static str,
    pub file: ModelFile,
    pub input_size: usize,
    pub mean: [f32; 3],
    pub std: [f32; 3],
    pub input_name: &'static str,
    pub output_name: &'static str,
    pub license: &'static str,
    pub attribution: &'static str,
}

/// SAM 2.1 (Hiera small), Meta's Segment Anything Model 2, in the ONNX export of the
/// Hugging Face `onnx-community` (repository `onnx-community/sam2.1-hiera-small-ONNX`, at
/// an immutable commit): an image encoder run once per photo, and a prompt encoder with
/// mask decoder run for every prompt.
pub const SUBJECT: ModelSpec = ModelSpec {
    id: "sam2.1-hiera-small",
    version: 1,
    files: &[
        ModelFile {
            name: "vision_encoder.onnx",
            size_bytes: 467_440,
            sha256: "aacf1f7137bb6fffcf6bf166abcfabe28f57a76059254f3fb611c4a64a208119",
            url: "https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX/resolve/a7df49d8de14b9d2e4504d1687b0d568f905fd8d/onnx/vision_encoder.onnx",
        },
        ModelFile {
            name: "vision_encoder.onnx_data",
            size_bytes: 162_476_288,
            sha256: "260fd1f0a34e72a3dc79a739e563b4facc0ba75504818b433a1f808e66637456",
            url: "https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX/resolve/a7df49d8de14b9d2e4504d1687b0d568f905fd8d/onnx/vision_encoder.onnx_data",
        },
        ModelFile {
            name: "prompt_encoder_mask_decoder.onnx",
            size_bytes: 213_114,
            sha256: "079c59b261f723ff5c6a125e69b0170a957b21c58738c28d2b0394ecd0587d7f",
            url: "https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX/resolve/a7df49d8de14b9d2e4504d1687b0d568f905fd8d/onnx/prompt_encoder_mask_decoder.onnx",
        },
        ModelFile {
            name: "prompt_encoder_mask_decoder.onnx_data",
            size_bytes: 20_958_208,
            sha256: "f9e59a584ab8ced21fa812c211bc01084204db1c9e92a5ef4fb3a49972b4e864",
            url: "https://huggingface.co/onnx-community/sam2.1-hiera-small-ONNX/resolve/a7df49d8de14b9d2e4504d1687b0d568f905fd8d/onnx/prompt_encoder_mask_decoder.onnx_data",
        },
    ],
    encoder_file: "vision_encoder.onnx",
    decoder_file: "prompt_encoder_mask_decoder.onnx",
    input_size: 1024,
    mask_size: 256,
    mean: [0.485, 0.456, 0.406],
    std: [0.229, 0.224, 0.225],
    processing_version: 3,
    license: "Apache-2.0 (facebook/sam2.1-hiera-small, weights and code). The ONNX export is \
              the Hugging Face onnx-community's conversion of those weights.",
    attribution: "Segment Anything Model 2 (SAM 2), Ravi et al., Meta AI, 2024, \
                  https://github.com/facebookresearch/sam2, Apache-2.0; ONNX export by \
                  onnx-community on Hugging Face.",
};

/// IS-Net "general use" (DIS, Xuebin Qin et al.), the ONNX conversion distributed by the
/// rembg project. It decides which objects are the subject; SAM 2 draws their outlines.
pub const SALIENCY: SaliencySpec = SaliencySpec {
    id: "isnet-general-use",
    file: ModelFile {
        name: "isnet-general-use.onnx",
        size_bytes: 178_648_008,
        sha256: "60920e99c45464f2ba57bee2ad08c919a52bbf852739e96947fbb4358c0d964a",
        url: "https://github.com/danielgatis/rembg/releases/download/v0.0.0/isnet-general-use.onnx",
    },
    input_size: 1024,
    mean: [0.5, 0.5, 0.5],
    std: [1.0, 1.0, 1.0],
    input_name: "input_image",
    output_name: "output_image",
    license: "Code: Apache-2.0 (xuebinqin/DIS). Conversion: rembg release v0.0.0 (MIT). \
              No separate license is published for the isnet-general-use weights.",
    attribution: "IS-Net (DIS: Highly Accurate Dichotomous Image Segmentation, Qin et al., \
                  ECCV 2022), https://github.com/xuebinqin/DIS, Apache-2.0; ONNX conversion \
                  from https://github.com/danielgatis/rembg.",
};

/// Every file the selection feature needs, in one folder.
pub fn all_files() -> Vec<ModelFile> {
    SUBJECT
        .files
        .iter()
        .copied()
        .chain([SALIENCY.file])
        .collect()
}

/// Bytes of [`all_files`].
pub fn total_bytes() -> u64 {
    all_files().iter().map(|f| f.size_bytes).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_are_lowercase_sha256_hex_and_urls_are_pinned() {
        for file in all_files() {
            assert_eq!(file.sha256.len(), 64, "{}", file.name);
            assert!(
                file.sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            );
            assert!(file.url.starts_with("https://") && !file.url.contains("/main/"));
            assert!(file.url.ends_with(file.name), "{}", file.name);
        }
        assert!(SUBJECT.files.iter().any(|f| f.name == SUBJECT.encoder_file));
        assert!(SUBJECT.files.iter().any(|f| f.name == SUBJECT.decoder_file));
        assert_eq!(total_bytes(), 184_115_050 + 178_648_008);
    }
}
