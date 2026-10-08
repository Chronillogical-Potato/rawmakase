//! The pinned contract of the one automatic-subject model.
//!
//! Everything a preprocessing/postprocessing step or an installer needs to know
//! about the model lives in [`SUBJECT`]; nothing else in the crate hard-codes
//! a size, a tensor name or a normalization constant.

/// Hugging Face repository the pinned artifact is mirrored to. Placeholder:
/// the maintainer fills this in when publishing; the installer must refuse to
/// use it while it still contains `OWNER`.
pub const REPO: &str = "OWNER/rawmakase-models";

/// Immutable commit of [`REPO`] holding the artifact. Placeholder, to be filled
/// in with a full 40-character commit hash; never a branch name.
pub const REVISION: &str = "REVISION_PLACEHOLDER";

/// How the image is brought to the model's square input.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Resize {
    /// Resample the whole image to `input_size` x `input_size`, ignoring its
    /// aspect ratio. The model was trained this way; the output is stretched
    /// back the same way.
    Stretch,
    /// Fit inside the square keeping the aspect ratio and fill the rest with
    /// `fill` (a value in 0..=1 of the image range, before normalization). The
    /// output is cropped to the content rectangle and then resampled.
    Letterbox { fill: f32 },
}

/// What the model's output tensor holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    /// Raw logits: a sigmoid is applied here.
    Logit,
    /// Probabilities in 0..=1 already (the graph ends in a sigmoid). Values
    /// outside that range by more than a small tolerance are rejected rather
    /// than min/max-normalized.
    Probability,
}

/// One model's contract: identity, artifact, tensors and pre/post-processing.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub id: &'static str,
    /// Version of this contract (bumped when anything below changes in a way
    /// that can change the produced matte, including the preprocessing code).
    pub version: u32,
    /// File name of the artifact as stored under the model directory.
    pub file_name: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
    /// Where the bytes were obtained for evaluation (upstream, unmodified).
    pub upstream_url: &'static str,
    /// Path of the artifact inside [`REPO`] at [`REVISION`].
    pub repo_path: &'static str,
    /// Square side the network is fed; the exported graph has a fixed shape.
    pub input_size: usize,
    /// Per-channel mean/std applied to RGB scaled to 0..=1: `(v - mean) / std`.
    pub mean: [f32; 3],
    pub std: [f32; 3],
    pub resize: Resize,
    pub input_name: &'static str,
    /// The output carrying the matte. The model has further outputs, which are
    /// ignored.
    pub output_name: &'static str,
    pub activation: Activation,
    /// Version of the preprocessing/postprocessing code paths in this crate for
    /// this model (resampling filters, rounding). Part of any cache key.
    pub processing_version: u32,
    pub license: &'static str,
    pub attribution: &'static str,
}

impl ModelSpec {
    /// The URL a downloader fetches: the Hugging Face resolve URL pinned to
    /// [`REVISION`].
    pub fn pinned_url(&self) -> String {
        format!(
            "https://huggingface.co/{REPO}/resolve/{REVISION}/{}",
            self.repo_path
        )
    }

    /// Whether [`REPO`] and [`REVISION`] have been filled in.
    pub fn is_published() -> bool {
        !REPO.contains("OWNER") && !REVISION.contains("PLACEHOLDER")
    }
}

/// IS-Net "general use" (DIS, Xuebin Qin et al.), the ONNX conversion
/// distributed by the rembg project.
pub const SUBJECT: ModelSpec = ModelSpec {
    id: "isnet-general-use",
    version: 1,
    file_name: "isnet-general-use.onnx",
    size_bytes: 178_648_008,
    sha256: "60920e99c45464f2ba57bee2ad08c919a52bbf852739e96947fbb4358c0d964a",
    upstream_url: "https://github.com/danielgatis/rembg/releases/download/v0.0.0/isnet-general-use.onnx",
    repo_path: "isnet-general-use/isnet-general-use.onnx",
    input_size: 1024,
    mean: [0.5, 0.5, 0.5],
    std: [1.0, 1.0, 1.0],
    resize: Resize::Stretch,
    input_name: "input_image",
    output_name: "output_image",
    activation: Activation::Probability,
    processing_version: 2,
    license: "Code: Apache-2.0 (xuebinqin/DIS). Conversion: rembg release v0.0.0 (MIT). \
              Weights: no separate license is published for isnet-general-use.pth; \
              confirm with the authors before redistribution.",
    attribution: "IS-Net (DIS: Highly Accurate Dichotomous Image Segmentation, Qin et al., \
                  ECCV 2022), https://github.com/xuebinqin/DIS, Apache-2.0; ONNX conversion \
                  from https://github.com/danielgatis/rembg.",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_lowercase_sha256_hex() {
        assert_eq!(SUBJECT.sha256.len(), 64);
        assert!(
            SUBJECT
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
    }

    #[test]
    fn placeholders_are_unpublished_until_filled() {
        assert!(!ModelSpec::is_published());
        assert!(SUBJECT.pinned_url().starts_with("https://huggingface.co/"));
    }
}
