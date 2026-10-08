//! Lazy loading of the ONNX Runtime shared library and the model session.
//!
//! The library is never linked: it is opened at run time from a path this
//! module chooses, so an absent or unusable runtime is an
//! [`InferenceError::RuntimeUnavailable`] value and nothing else in the app is
//! affected. The first successfully opened library is kept for the life of the
//! process (ONNX Runtime cannot be unloaded safely).

use std::env;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ort::ep;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::{RunOptions, Session as OrtSession};
use ort::value::TensorRef;

use crate::auto::{self, Proposal, SIDE};
use crate::error::InferenceError;
use crate::manifest::{ModelSpec, PANOPTIC, SUBJECT, all_files};
use crate::panoptic::{self, Labels};
use crate::process::{self, Coverage, Prompt, RgbImage};

/// Environment variable naming the ONNX Runtime library file to load.
pub const RUNTIME_ENV: &str = "RAWMAKASE_ORT_LIB";

/// The ONNX Runtime version the packaged library is pinned to. Any release at
/// or above the `ort` crate's minimum API (1.17) loads; this is the one that
/// is tested and shipped for every platform, because 1.23.2 is the last release
/// with an official macOS x86_64 build.
pub const PINNED_RUNTIME_VERSION: &str = "1.23.2";

/// How often a running inference polls the caller's cancel flag.
const CANCEL_POLL: Duration = Duration::from_millis(15);

/// Upper bound on the intra-op threads: beyond this the encoder kernels
/// stop scaling and only compete with the UI and render threads.
const MAX_THREADS: usize = 8;

/// The library file names to look for, most specific first.
pub fn runtime_file_names() -> &'static [&'static str] {
    if cfg!(target_os = "windows") {
        &["onnxruntime.dll"]
    } else if cfg!(target_os = "macos") {
        &["libonnxruntime.dylib"]
    } else {
        &["libonnxruntime.so", "libonnxruntime.so.1"]
    }
}

/// Directories next to the running executable that a package may place the
/// runtime in. System search paths are deliberately absent: on Windows a bare
/// `onnxruntime.dll` resolves to the older copy in System32.
pub fn runtime_search_dirs() -> Vec<PathBuf> {
    let Some(exe_dir) = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    else {
        return Vec::new();
    };
    let mut dirs = vec![exe_dir.clone(), exe_dir.join("lib")];
    if cfg!(target_os = "macos") {
        // Contents/MacOS/rawmakase -> Contents/Frameworks
        dirs.insert(0, exe_dir.join("../Frameworks"));
    } else if cfg!(target_os = "linux") {
        dirs.push(exe_dir.join("../lib/rawmakase"));
        dirs.push(exe_dir.join("../lib"));
    }
    dirs
}

/// Options for [`Subject::load_with`].
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Explicit runtime library file. Takes precedence over
    /// [`RUNTIME_ENV`] and the package locations.
    pub runtime_library: Option<PathBuf>,
    /// Intra-op threads; `None` picks [`default_threads`].
    pub threads: Option<usize>,
}

/// Threads for one inference: the available parallelism capped at 8. ONNX
/// Runtime's own default would use every core including the efficiency cores.
pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map_or(4, usize::from)
        .clamp(1, MAX_THREADS)
}

/// Namespace for loading the selection model.
pub struct Subject;

impl Subject {
    /// Opens the runtime (searching [`RUNTIME_ENV`] and the package locations)
    /// and the model in folder `dir`.
    pub fn load(dir: &Path) -> Result<Session, InferenceError> {
        Self::load_with(dir, &LoadOptions::default())
    }

    pub fn load_with(dir: &Path, options: &LoadOptions) -> Result<Session, InferenceError> {
        load_runtime(options.runtime_library.as_deref())?;
        let spec = SUBJECT;
        check_files(dir)?;
        let threads = options.threads.unwrap_or_else(default_threads).max(1);
        // The runtime API panics on internal misuse; a panic must never reach
        // the app from a background job.
        let (encoder, decoder, panoptic) = catch_unwind(AssertUnwindSafe(|| {
            Ok::<_, InferenceError>((
                build_session(
                    &dir.join(spec.encoder_file),
                    threads,
                    GraphOptimizationLevel::All,
                )?,
                build_session(
                    &dir.join(spec.decoder_file),
                    threads,
                    GraphOptimizationLevel::All,
                )?,
                // The half-precision graph breaks the full fusion pass.
                build_session(
                    &dir.join(PANOPTIC.file.name),
                    threads,
                    GraphOptimizationLevel::Level1,
                )?,
            ))
        }))
        .map_err(|_| {
            InferenceError::Failed("ONNX Runtime panicked while loading the model".into())
        })??;
        Ok(Session {
            encoder,
            decoder,
            panoptic,
            spec,
        })
    }
}

/// What the image encoder made of one photo: reused for every click on it. The
/// photo itself is kept for the edge refinement of each result.
pub struct Embedding {
    features: [([usize; 4], Vec<f32>); 3],
    image: RgbImage,
}
impl Embedding {
    /// Bytes held.
    pub fn bytes(&self) -> usize {
        self.features
            .iter()
            .map(|(_, v)| v.len() * 4)
            .sum::<usize>()
            + self.image.data.len()
    }
}

/// A loaded model. Not `Sync`: one run at a time per session, which is also what
/// ONNX Runtime's `Run` expects here. Dropping it releases the model's memory.
pub struct Session {
    encoder: OrtSession,
    decoder: OrtSession,
    panoptic: OrtSession,
    spec: ModelSpec,
}

/// What the models made of one photo: reused for every selection on it. Subject and Sky
/// are chosen from it without running a model again; clicks need the embedding.
pub struct Analysis {
    embedding: Embedding,
    labels: Labels,
    proposals: Vec<Proposal>,
}
impl Analysis {
    /// The photo's subject as coverage in its frame, all zero when nothing stands out.
    pub fn subject(&self) -> Result<Coverage, InferenceError> {
        auto::subject(&self.embedding.image, &self.proposals, &self.labels.subject)
    }
    /// The photo's sky, all zero when there is none.
    pub fn sky(&self) -> Result<Coverage, InferenceError> {
        auto::sky(&self.embedding.image, &self.proposals, &self.labels.sky)
    }
    /// The embedding, for [`Session::segment`].
    pub fn embedding(&self) -> &Embedding {
        &self.embedding
    }
    /// Bytes held.
    pub fn bytes(&self) -> usize {
        self.embedding.bytes()
            + (self.labels.subject.len() + self.labels.sky.len()) * 4
            + self.proposals.iter().map(|p| p.mask.len()).sum::<usize>()
    }
}

/// The decoder's answer to one prompt.
struct Decoded {
    /// Three candidate masks' logits, one after the other.
    logits: Vec<f32>,
    scores: Vec<f32>,
    object: f32,
}

impl Session {
    pub fn spec(&self) -> &ModelSpec {
        &self.spec
    }

    /// Runs the image encoder on `image`. This is the expensive step (about a second);
    /// what it returns serves any number of [`Session::segment`] calls on the photo.
    ///
    /// `cancel` is checked before and polled while the network runs (which terminates
    /// the run), so a raised flag never yields an embedding.
    pub fn embed(
        &mut self,
        image: &RgbImage,
        cancel: &AtomicBool,
    ) -> Result<Embedding, InferenceError> {
        cancelled(cancel)?;
        let tensor =
            process::preprocess(image, self.spec.input_size, self.spec.mean, self.spec.std)?;
        cancelled(cancel)?;
        let size = self.spec.input_size;
        let encoder = &mut self.encoder;
        let features = guarded(cancel, |options| {
            let input = TensorRef::from_array_view(([1usize, 3, size, size], &tensor[..]))
                .map_err(failed)?;
            let outputs = encoder
                .run_with_options(ort::inputs!["pixel_values" => input], options)
                .map_err(failed)?;
            let mut features = Vec::with_capacity(3);
            for index in 0..3 {
                let name = format!("image_embeddings.{index}");
                let output = outputs.get(name.as_str()).ok_or_else(|| {
                    InferenceError::ModelInvalid(format!("the encoder has no output `{name}`"))
                })?;
                let (shape, data) = output.try_extract_tensor::<f32>().map_err(failed)?;
                features.push((shape.iter().copied().collect::<Vec<i64>>(), data.to_vec()));
            }
            Ok(features)
        })?;
        cancelled(cancel)?;
        let mut checked = Vec::with_capacity(3);
        for (shape, data) in features {
            let dims: Option<[usize; 4]> = (shape.len() == 4 && shape[0] == 1)
                .then(|| {
                    let mut dims = [0usize; 4];
                    for (out, d) in dims.iter_mut().zip(&shape) {
                        *out = usize::try_from(*d).ok()?;
                    }
                    Some(dims)
                })
                .flatten();
            let count = dims.and_then(|d| d.iter().try_fold(1usize, |n, d| n.checked_mul(*d)));
            match dims {
                Some(dims) if count == Some(data.len()) => checked.push((dims, data)),
                _ => {
                    return Err(InferenceError::OutputInvalid(format!(
                        "an encoder output has shape {shape:?}"
                    )));
                }
            }
        }
        let features: [([usize; 4], Vec<f32>); 3] = checked
            .try_into()
            .map_err(|_| InferenceError::ModelInvalid("the encoder outputs changed".into()))?;
        Ok(Embedding {
            features,
            image: image.clone(),
        })
    }

    /// The mask for `prompt` on an embedded photo, as 8-bit coverage at the photo's
    /// own size (capped at [`process::MAX_COVERAGE_SIDE`]), its edge moved onto the
    /// photo's. All zero when the model finds no object at the prompt.
    pub fn segment(
        &mut self,
        embedding: &Embedding,
        prompt: &Prompt,
        cancel: &AtomicBool,
    ) -> Result<Coverage, InferenceError> {
        let decoded = self.decode(embedding, prompt, cancel)?;
        let (width, height) = process::output_size(&embedding.image);
        if decoded.object < 0.0 {
            return Ok(Coverage {
                width,
                height,
                data: vec![0; width * height],
            });
        }
        let side = self.spec.mask_size;
        let best = decoded.best();
        let mut coverage = process::postprocess(
            &decoded.logits[best * side * side..(best + 1) * side * side],
            &self.spec,
            width,
            height,
        )?;
        crate::refine::refine(&mut coverage, &embedding.image);
        Ok(coverage)
    }

    /// Everything automatic selection needs of a photo: the embedding, the people, animals and sky labels and
    /// the outlines SAM 2 draws from a grid of points. A few seconds; cancellable.
    pub fn analyze(
        &mut self,
        image: &RgbImage,
        cancel: &AtomicBool,
    ) -> Result<Analysis, InferenceError> {
        let embedding = self.embed(image, cancel)?;
        let labels = self.panoptic(image, cancel)?;
        let side = self.spec.mask_size;
        const GRID: usize = 7;
        let mut found: Vec<Proposal> = Vec::new();
        for j in 0..GRID {
            for i in 0..GRID {
                let prompt = Prompt {
                    points: vec![crate::process::Point {
                        x: (i as f32 + 0.5) / GRID as f32,
                        y: (j as f32 + 0.5) / GRID as f32,
                        positive: true,
                    }],
                    bounds: None,
                };
                let decoded = self.decode(&embedding, &prompt, cancel)?;
                if decoded.object < 0.0 {
                    continue;
                }
                for k in 0..3 {
                    if decoded.scores[k] > 0.6 {
                        found.push(Proposal {
                            mask: decoded.logits[k * side * side..(k + 1) * side * side]
                                .iter()
                                .map(|l| *l > 0.0)
                                .collect(),
                            score: decoded.scores[k],
                        });
                    }
                }
            }
        }
        debug_assert_eq!(side, SIDE);
        Ok(Analysis {
            embedding,
            labels,
            proposals: distinct(found),
        })
    }

    /// Where the photo's people, animals and sky are, on the [`SIDE`] grid.
    fn panoptic(
        &mut self,
        image: &RgbImage,
        cancel: &AtomicBool,
    ) -> Result<Labels, InferenceError> {
        cancelled(cancel)?;
        process::check_image(image)?;
        let scale = PANOPTIC.long_edge as f64 / image.width.max(image.height) as f64;
        let width = ((image.width as f64 * scale).round() as usize).max(1);
        let height = ((image.height as f64 * scale).round() as usize).max(1);
        let tensor = process::preprocess_sized(image, width, height, PANOPTIC.mean, PANOPTIC.std)?;
        cancelled(cancel)?;
        let network = &mut self.panoptic;
        let (logits, masks, queries, classes, mask_w, mask_h) = guarded(cancel, |options| {
            let input = TensorRef::from_array_view(([1usize, 3, height, width], &tensor[..]))
                .map_err(failed)?;
            // Every pixel is real: the model takes this mask at a fixed size.
            let valid = vec![1i64; 64 * 64];
            let mask =
                TensorRef::from_array_view(([1usize, 64, 64], &valid[..])).map_err(failed)?;
            let outputs = network
                .run_with_options(
                    ort::inputs!["pixel_values" => input, "pixel_mask" => mask],
                    options,
                )
                .map_err(failed)?;
            let get = |name: &str| -> Result<(Vec<i64>, Vec<f32>), InferenceError> {
                let output = outputs.get(name).ok_or_else(|| {
                    InferenceError::ModelInvalid(format!(
                        "the panoptic model has no output `{name}`"
                    ))
                })?;
                let (shape, data) = output.try_extract_tensor::<f32>().map_err(failed)?;
                Ok((shape.iter().copied().collect(), data.to_vec()))
            };
            let (logit_shape, logits) = get("logits")?;
            let (mask_shape, masks) = get("pred_masks")?;
            if logit_shape.len() != 3
                || mask_shape.len() != 4
                || logit_shape[0] != 1
                || mask_shape[0] != 1
            {
                return Err(InferenceError::OutputInvalid(format!(
                    "the panoptic output shapes are {logit_shape:?} and {mask_shape:?}"
                )));
            }
            let dim = |v: i64| usize::try_from(v).unwrap_or(0);
            Ok((
                logits,
                masks,
                dim(logit_shape[1]),
                dim(logit_shape[2]),
                dim(mask_shape[3]),
                dim(mask_shape[2]),
            ))
        })?;
        panoptic::decode(&PANOPTIC, &logits, &masks, queries, classes, mask_w, mask_h)
    }

    fn decode(
        &mut self,
        embedding: &Embedding,
        prompt: &Prompt,
        cancel: &AtomicBool,
    ) -> Result<Decoded, InferenceError> {
        cancelled(cancel)?;
        prompt.validate()?;
        let (points, labels, bounds) = prompt.tensors(&self.spec);
        let n = labels.len();
        let boxes = bounds.len() / 4;
        let decoder = &mut self.decoder;
        let (logits, scores, object) = guarded(cancel, |options| {
            let [f0, f1, f2] = &embedding.features;
            let outputs = decoder
                .run_with_options(
                    ort::inputs![
                        "input_points" => TensorRef::from_array_view(([1usize, 1, n, 2], &points[..])).map_err(failed)?,
                        "input_labels" => TensorRef::from_array_view(([1usize, 1, n], &labels[..])).map_err(failed)?,
                        "input_boxes" => TensorRef::from_array_view(([1usize, boxes, 4], &bounds[..])).map_err(failed)?,
                        "image_embeddings.0" => view(f0)?,
                        "image_embeddings.1" => view(f1)?,
                        "image_embeddings.2" => view(f2)?,
                    ],
                    options,
                )
                .map_err(failed)?;
            let get = |name: &str| -> Result<Vec<f32>, InferenceError> {
                let output = outputs.get(name).ok_or_else(|| {
                    InferenceError::ModelInvalid(format!("the decoder has no output `{name}`"))
                })?;
                let (_, data) = output.try_extract_tensor::<f32>().map_err(failed)?;
                Ok(data.to_vec())
            };
            Ok((
                get("pred_masks")?,
                get("iou_scores")?,
                get("object_score_logits")?,
            ))
        })?;
        let side = self.spec.mask_size;
        if scores.len() != 3
            || logits.len() != 3 * side * side
            || object.len() != 1
            || scores.iter().any(|s| !s.is_finite())
            || !object[0].is_finite()
        {
            return Err(InferenceError::OutputInvalid(format!(
                "the decoder returned {} masks of {} values",
                scores.len(),
                logits.len()
            )));
        }
        Ok(Decoded {
            logits,
            scores,
            object: object[0],
        })
    }
}

impl Decoded {
    /// The candidate the model rates highest.
    fn best(&self) -> usize {
        (0..3)
            .max_by(|a, b| self.scores[*a].total_cmp(&self.scores[*b]))
            .unwrap_or(0)
    }
}

/// The proposals, best first, without near-duplicates.
fn distinct(mut found: Vec<Proposal>) -> Vec<Proposal> {
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Proposal> = Vec::new();
    for p in found {
        let overlaps = |q: &Proposal| {
            let both = p
                .mask
                .iter()
                .zip(&q.mask)
                .filter(|(a, b)| **a && **b)
                .count();
            let either = p
                .mask
                .iter()
                .zip(&q.mask)
                .filter(|(a, b)| **a || **b)
                .count();
            either > 0 && both as f32 / either as f32 > 0.85
        };
        if !kept.iter().any(overlaps) {
            kept.push(p);
        }
    }
    kept
}

fn view(f: &([usize; 4], Vec<f32>)) -> Result<TensorRef<'_, f32>, InferenceError> {
    TensorRef::from_array_view((f.0, &f.1[..])).map_err(failed)
}

fn cancelled(cancel: &AtomicBool) -> Result<(), InferenceError> {
    if cancel.load(Ordering::Relaxed) {
        Err(InferenceError::Cancelled)
    } else {
        Ok(())
    }
}

/// Runs `work` with a watcher that terminates the run when `cancel` is raised, and a
/// panic in the runtime turned into an error.
fn guarded<T>(
    cancel: &AtomicBool,
    work: impl FnOnce(&RunOptions) -> Result<T, InferenceError>,
) -> Result<T, InferenceError> {
    let options = RunOptions::new().map_err(failed)?;
    let done = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Acquire) {
                if cancel.load(Ordering::Relaxed) {
                    let _ = options.terminate();
                    return;
                }
                std::thread::sleep(CANCEL_POLL);
            }
        });
        let result = catch_unwind(AssertUnwindSafe(|| work(&options)));
        done.store(true, Ordering::Release);
        result
    });
    if cancel.load(Ordering::Relaxed) {
        return Err(InferenceError::Cancelled);
    }
    match result {
        Ok(result) => result,
        Err(_) => Err(InferenceError::Failed(
            "ONNX Runtime panicked during inference".into(),
        )),
    }
}

fn failed(error: impl std::fmt::Display) -> InferenceError {
    InferenceError::Failed(error.to_string())
}

fn check_files(dir: &Path) -> Result<(), InferenceError> {
    for file in all_files() {
        let path = dir.join(file.name);
        let meta = std::fs::metadata(&path).map_err(|e| {
            InferenceError::ModelInvalid(format!("cannot read {}: {e}", path.display()))
        })?;
        if !meta.is_file() || meta.len() != file.size_bytes {
            return Err(InferenceError::ModelInvalid(format!(
                "{} is not the expected {} bytes",
                path.display(),
                file.size_bytes
            )));
        }
    }
    Ok(())
}

fn build_session(
    model_path: &Path,
    threads: usize,
    level: GraphOptimizationLevel,
) -> Result<OrtSession, InferenceError> {
    OrtSession::builder()
        .map_err(failed)?
        .with_optimization_level(level)
        .map_err(failed)?
        .with_intra_threads(threads)
        .map_err(failed)?
        .with_inter_threads(1)
        .map_err(failed)?
        .with_intra_op_spinning(false)
        .map_err(failed)?
        .with_memory_pattern(false)
        .map_err(failed)?
        .with_execution_providers([ep::CPU::default().with_arena_allocator(false).build()])
        .map_err(failed)?
        .commit_from_file(model_path)
        .map_err(|e| {
            InferenceError::ModelInvalid(format!("cannot load {}: {e}", model_path.display()))
        })
}

/// Opens the ONNX Runtime library if it is not open yet. The first successful
/// load is process-wide; later calls with another path are no-ops.
pub fn load_runtime(explicit: Option<&Path>) -> Result<(), InferenceError> {
    let mut tried = Vec::new();
    let candidates = runtime_candidates(explicit);
    if candidates.is_empty() {
        return Err(InferenceError::RuntimeUnavailable(format!(
            "no ONNX Runtime library found; set {RUNTIME_ENV} or place {} next to the application",
            runtime_file_names()[0]
        )));
    }
    for path in candidates {
        match open_runtime(&path) {
            Ok(()) => return Ok(()),
            Err(why) => tried.push(format!("{}: {why}", path.display())),
        }
    }
    Err(InferenceError::RuntimeUnavailable(tried.join("; ")))
}

fn runtime_candidates(explicit: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = explicit {
        candidates.push(path.to_path_buf());
    }
    if let Some(path) = env::var_os(RUNTIME_ENV).filter(|v| !v.is_empty()) {
        candidates.push(PathBuf::from(path));
    }
    for dir in runtime_search_dirs() {
        for name in runtime_file_names() {
            let path = dir.join(name);
            if path.is_file() {
                candidates.push(path);
            }
        }
    }
    candidates
}

fn open_runtime(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("the runtime path must be absolute".into());
    }
    if !path.is_file() {
        return Err("no such file".into());
    }
    // `ort` validates the version and returns an error rather than panicking,
    // but a panic here must still never take the caller down.
    catch_unwind(AssertUnwindSafe(|| {
        ort::init_from(path).map(|builder| {
            builder.with_name("rawmakase").commit();
        })
    }))
    .map_err(|_| "loading the library panicked".to_string())?
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_runtime_is_an_error_value() {
        let missing = Path::new("/nonexistent/rawmakase/libonnxruntime.dylib");
        let err = load_runtime(Some(missing)).unwrap_err();
        assert!(
            matches!(err, InferenceError::RuntimeUnavailable(_)),
            "{err:?}"
        );
    }

    #[test]
    fn a_relative_runtime_path_is_refused() {
        let err = load_runtime(Some(Path::new("libonnxruntime.dylib"))).unwrap_err();
        assert!(matches!(err, InferenceError::RuntimeUnavailable(_)));
    }

    #[test]
    fn a_file_that_is_not_a_library_is_refused() {
        let dir =
            std::env::temp_dir().join(format!("rawmakase-inference-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join(runtime_file_names()[0]);
        std::fs::write(&fake, b"not a library").unwrap();
        let err = load_runtime(Some(&fake)).unwrap_err();
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            matches!(err, InferenceError::RuntimeUnavailable(_)),
            "{err:?}"
        );
    }

    #[test]
    fn a_missing_or_wrong_sized_model_file_is_invalid_before_the_runtime_is_touched() {
        let dir =
            std::env::temp_dir().join(format!("rawmakase-inference-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(
            check_files(&dir),
            Err(InferenceError::ModelInvalid(_))
        ));
        for file in all_files() {
            std::fs::write(dir.join(file.name), b"short").unwrap();
        }
        let err = check_files(&dir).unwrap_err();
        std::fs::remove_dir_all(&dir).ok();
        assert!(matches!(err, InferenceError::ModelInvalid(_)));
    }

    #[test]
    fn threads_are_bounded() {
        let n = default_threads();
        assert!((1..=MAX_THREADS).contains(&n));
    }
}
