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
use ort::value::{TensorElementType, TensorRef, ValueType};

use crate::error::InferenceError;
use crate::manifest::{ModelSpec, SUBJECT};
use crate::process::{self, Coverage, RgbImage};

/// Environment variable naming the ONNX Runtime library file to load.
pub const RUNTIME_ENV: &str = "RAWMAKASE_ORT_LIB";

/// The ONNX Runtime version the packaged library is pinned to. Any release at
/// or above the `ort` crate's minimum API (1.17) loads; this is the one that
/// is tested and shipped for every platform, because 1.23.2 is the last release
/// with an official macOS x86_64 build.
pub const PINNED_RUNTIME_VERSION: &str = "1.23.2";

/// How often a running inference polls the caller's cancel flag.
const CANCEL_POLL: Duration = Duration::from_millis(15);

/// Upper bound on the intra-op threads: beyond this the Swin/IS-Net kernels
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

/// Namespace for loading the automatic subject model.
pub struct Subject;

impl Subject {
    /// Opens the runtime (searching [`RUNTIME_ENV`] and the package locations)
    /// and the model file at `model_path`.
    pub fn load(model_path: &Path) -> Result<Session, InferenceError> {
        Self::load_with(model_path, &LoadOptions::default())
    }

    pub fn load_with(model_path: &Path, options: &LoadOptions) -> Result<Session, InferenceError> {
        load_runtime(options.runtime_library.as_deref())?;
        let spec = SUBJECT;
        check_model_file(model_path, &spec)?;
        let threads = options.threads.unwrap_or_else(default_threads).max(1);
        // The runtime API panics on internal misuse; a panic must never reach
        // the app from a background job.
        let session = catch_unwind(AssertUnwindSafe(|| {
            build_session(model_path, &spec, threads)
        }))
        .map_err(|_| {
            InferenceError::Failed("ONNX Runtime panicked while loading the model".into())
        })??;
        Ok(Session { session, spec })
    }
}

/// A loaded model. Not `Sync`: one inference at a time per session, which is
/// also what ONNX Runtime's `Run` expects here. Dropping it releases the
/// model's memory.
pub struct Session {
    session: OrtSession,
    spec: ModelSpec,
}

impl Session {
    pub fn spec(&self) -> &ModelSpec {
        &self.spec
    }

    /// Computes the subject coverage of `image`, at the image's own size (capped
    /// at [`process::MAX_COVERAGE_SIDE`]).
    ///
    /// `cancel` is checked before preprocessing, polled while the network runs
    /// (which terminates the run) and checked again before the result is
    /// returned, so a raised flag never yields a coverage.
    pub fn select(
        &mut self,
        image: &RgbImage,
        cancel: &AtomicBool,
    ) -> Result<Coverage, InferenceError> {
        if cancel.load(Ordering::Relaxed) {
            return Err(InferenceError::Cancelled);
        }
        let (tensor, geometry) = process::preprocess(image, &self.spec)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(InferenceError::Cancelled);
        }
        let size = self.spec.input_size;
        let input =
            TensorRef::from_array_view(([1usize, 3, size, size], &tensor[..])).map_err(failed)?;
        let run_options = RunOptions::new().map_err(failed)?;
        let spec = self.spec;
        let session = &mut self.session;
        let done = AtomicBool::new(false);
        let (coverage, shape, data) = std::thread::scope(|scope| {
            scope.spawn(|| {
                while !done.load(Ordering::Acquire) {
                    if cancel.load(Ordering::Relaxed) {
                        let _ = run_options.terminate();
                        return;
                    }
                    std::thread::sleep(CANCEL_POLL);
                }
            });
            let result = catch_unwind(AssertUnwindSafe(|| {
                let outputs = session
                    .run_with_options(ort::inputs![spec.input_name => input], &run_options)
                    .map_err(failed)?;
                let output = outputs.get(spec.output_name).ok_or_else(|| {
                    InferenceError::ModelInvalid(format!(
                        "the model has no output `{}`",
                        spec.output_name
                    ))
                })?;
                let (shape, data) = output.try_extract_tensor::<f32>().map_err(failed)?;
                Ok::<_, InferenceError>((
                    shape.iter().copied().collect::<Vec<i64>>(),
                    data.to_vec(),
                ))
            }));
            done.store(true, Ordering::Release);
            match result {
                Ok(Ok((shape, data))) => (Ok(()), shape, data),
                Ok(Err(e)) => (Err(e), Vec::new(), Vec::new()),
                Err(_) => (
                    Err(InferenceError::Failed(
                        "ONNX Runtime panicked during inference".into(),
                    )),
                    Vec::new(),
                    Vec::new(),
                ),
            }
        });
        if cancel.load(Ordering::Relaxed) {
            return Err(InferenceError::Cancelled);
        }
        coverage?;
        let mut coverage = process::postprocess(&data, &shape, &self.spec, &geometry)?;
        crate::refine::refine(&mut coverage, image);
        Ok(coverage)
    }
}

fn failed(error: impl std::fmt::Display) -> InferenceError {
    InferenceError::Failed(error.to_string())
}

fn check_model_file(path: &Path, spec: &ModelSpec) -> Result<(), InferenceError> {
    let meta = std::fs::metadata(path).map_err(|e| {
        InferenceError::ModelInvalid(format!("cannot read {}: {e}", path.display()))
    })?;
    if !meta.is_file() {
        return Err(InferenceError::ModelInvalid(format!(
            "{} is not a file",
            path.display()
        )));
    }
    if meta.len() != spec.size_bytes {
        return Err(InferenceError::ModelInvalid(format!(
            "{} is {} bytes, expected {}",
            path.display(),
            meta.len(),
            spec.size_bytes
        )));
    }
    Ok(())
}

fn build_session(
    model_path: &Path,
    spec: &ModelSpec,
    threads: usize,
) -> Result<OrtSession, InferenceError> {
    let session = OrtSession::builder()
        .map_err(failed)?
        .with_optimization_level(GraphOptimizationLevel::All)
        .map_err(failed)?
        .with_intra_threads(threads)
        .map_err(failed)?
        .with_inter_threads(1)
        .map_err(failed)?
        .with_intra_op_spinning(false)
        .map_err(failed)?
        // Fixed-shape model, one run at a time: the memory-pattern planner and
        // the CPU arena only keep memory around between runs.
        .with_memory_pattern(false)
        .map_err(failed)?
        .with_execution_providers([ep::CPU::default().with_arena_allocator(false).build()])
        .map_err(failed)?
        .commit_from_file(model_path)
        .map_err(|e| {
            InferenceError::ModelInvalid(format!("cannot load {}: {e}", model_path.display()))
        })?;
    validate_tensors(&session, spec)?;
    Ok(session)
}

fn validate_tensors(session: &OrtSession, spec: &ModelSpec) -> Result<(), InferenceError> {
    let size = spec.input_size as i64;
    let expect = |what: &str, ty: &ValueType, channels: i64| -> Result<(), InferenceError> {
        match ty {
            ValueType::Tensor {
                ty: TensorElementType::Float32,
                shape,
                ..
            } if shape.iter().copied().collect::<Vec<_>>() == [1, channels, size, size] => Ok(()),
            other => Err(InferenceError::ModelInvalid(format!(
                "{what} is {other:?}, expected float32 [1, {channels}, {size}, {size}]"
            ))),
        }
    };
    let inputs = session.inputs();
    let [input] = inputs else {
        return Err(InferenceError::ModelInvalid(format!(
            "the model has {} inputs, expected 1",
            inputs.len()
        )));
    };
    if input.name() != spec.input_name {
        return Err(InferenceError::ModelInvalid(format!(
            "input is `{}`, expected `{}`",
            input.name(),
            spec.input_name
        )));
    }
    expect("the input", input.dtype(), 3)?;
    let output = session
        .outputs()
        .iter()
        .find(|o| o.name() == spec.output_name)
        .ok_or_else(|| {
            InferenceError::ModelInvalid(format!("the model has no output `{}`", spec.output_name))
        })?;
    expect("the matte output", output.dtype(), 1)
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
    fn a_wrong_sized_model_file_is_invalid_before_the_runtime_is_touched() {
        let dir =
            std::env::temp_dir().join(format!("rawmakase-inference-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let model = dir.join(SUBJECT.file_name);
        std::fs::write(&model, b"short").unwrap();
        let err = check_model_file(&model, &SUBJECT).unwrap_err();
        std::fs::remove_dir_all(&dir).ok();
        assert!(matches!(err, InferenceError::ModelInvalid(_)));
        assert!(matches!(
            check_model_file(&dir.join("absent.onnx"), &SUBJECT),
            Err(InferenceError::ModelInvalid(_))
        ));
    }

    #[test]
    fn threads_are_bounded() {
        let n = default_threads();
        assert!((1..=MAX_THREADS).contains(&n));
    }
}
