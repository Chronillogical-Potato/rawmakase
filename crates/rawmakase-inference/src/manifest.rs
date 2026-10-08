//! The pinned contract of the one interactive selection model.
//!
//! Everything a pre/post-processing step or an installer needs to know about the
//! model lives in [`SUBJECT`]; nothing else in the crate hard-codes a size, a tensor
//! name or a normalization constant.

/// The Hugging Face repository the files are fetched from, and the immutable commit
/// they are pinned to. Never a branch: a file under this revision cannot change.
pub const SOURCE_REPO: &str = "onnx-community/sam2.1-hiera-small-ONNX";
pub const SOURCE_REVISION: &str = "a7df49d8de14b9d2e4504d1687b0d568f905fd8d";

/// One file of the model, as stored under the model's folder.
#[derive(Debug, Clone, Copy)]
pub struct ModelFile {
    pub name: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
}

/// The model's contract: identity, artifacts, tensors and preprocessing.
#[derive(Debug, Clone, Copy)]
pub struct ModelSpec {
    pub id: &'static str,
    /// Version of this contract (bumped when anything below changes in a way that can
    /// change the produced mask, including the pre/post-processing code).
    pub version: u32,
    /// Every file the model needs, in a folder of its own. A graph's weights sit in
    /// the `_data` file beside it and are read from there by name.
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
    /// edge refinement, rounding). Part of any cache key.
    pub processing_version: u32,
    pub license: &'static str,
    pub attribution: &'static str,
}

impl ModelSpec {
    /// Bytes to fetch in all.
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.size_bytes).sum()
    }
    /// The URL of `file`, pinned to [`SOURCE_REVISION`].
    pub fn url(&self, file: &ModelFile) -> String {
        format!(
            "https://huggingface.co/{SOURCE_REPO}/resolve/{SOURCE_REVISION}/onnx/{}",
            file.name
        )
    }
}

/// SAM 2.1 (Hiera small), Meta's Segment Anything Model 2, in the ONNX export of the
/// Hugging Face `onnx-community`: an image encoder run once per photo, and a prompt
/// encoder with mask decoder run for every click.
pub const SUBJECT: ModelSpec = ModelSpec {
    id: "sam2.1-hiera-small",
    version: 1,
    files: &[
        ModelFile {
            name: "vision_encoder.onnx",
            size_bytes: 467_440,
            sha256: "aacf1f7137bb6fffcf6bf166abcfabe28f57a76059254f3fb611c4a64a208119",
        },
        ModelFile {
            name: "vision_encoder.onnx_data",
            size_bytes: 162_476_288,
            sha256: "260fd1f0a34e72a3dc79a739e563b4facc0ba75504818b433a1f808e66637456",
        },
        ModelFile {
            name: "prompt_encoder_mask_decoder.onnx",
            size_bytes: 213_114,
            sha256: "079c59b261f723ff5c6a125e69b0170a957b21c58738c28d2b0394ecd0587d7f",
        },
        ModelFile {
            name: "prompt_encoder_mask_decoder.onnx_data",
            size_bytes: 20_958_208,
            sha256: "f9e59a584ab8ced21fa812c211bc01084204db1c9e92a5ef4fb3a49972b4e864",
        },
    ],
    encoder_file: "vision_encoder.onnx",
    decoder_file: "prompt_encoder_mask_decoder.onnx",
    input_size: 1024,
    mask_size: 256,
    mean: [0.485, 0.456, 0.406],
    std: [0.229, 0.224, 0.225],
    processing_version: 1,
    license: "Apache-2.0 (facebook/sam2.1-hiera-small, weights and code). The ONNX export is \
              the Hugging Face onnx-community's conversion of those weights.",
    attribution: "Segment Anything Model 2 (SAM 2), Ravi et al., Meta AI, 2024, \
                  https://github.com/facebookresearch/sam2, Apache-2.0; ONNX export by \
                  onnx-community on Hugging Face.",
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digests_are_lowercase_sha256_hex_and_revision_is_a_commit() {
        for file in SUBJECT.files {
            assert_eq!(file.sha256.len(), 64, "{}", file.name);
            assert!(
                file.sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            );
        }
        assert_eq!(SOURCE_REVISION.len(), 40);
        assert!(SOURCE_REVISION.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(SUBJECT.files.iter().any(|f| f.name == SUBJECT.encoder_file));
        assert!(SUBJECT.files.iter().any(|f| f.name == SUBJECT.decoder_file));
    }

    #[test]
    fn urls_are_pinned_to_the_commit() {
        let url = SUBJECT.url(&SUBJECT.files[0]);
        assert!(url.contains(SOURCE_REVISION) && !url.contains("/main/"));
        assert_eq!(SUBJECT.total_bytes(), 184_115_050);
    }
}
