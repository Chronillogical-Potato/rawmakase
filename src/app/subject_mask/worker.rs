//! The thread that runs the selection model. One request at a time, one loaded
//! session reused between them, and the image embedding of the photo last asked
//! about, so a second click on it takes milliseconds instead of a second. Cancelling raises the request's flag, which the
//! runtime notices within a few milliseconds; quitting hands the thread to the shared
//! shutdown deadline rather than joining it, and the thread keeps its session until
//! it ends, so a native call is never left running on unloaded code.
use super::{Action, Done, Failure, Feature, Generated};
use crate::app::task::Stopping;
use crate::app::worker::Event;
use crate::camera_data::CameraImage;
use crate::model::masks::BitmapSource;
use crate::model::recipe::Recipe;
use crate::storage::bitmaps::Bitmap;
use crate::storage::{FNV_OFFSET, fnv1a, mask_assets};
use eframe::egui;
use rawmakase_inference::{
    Analysis, Coverage, InferenceError, LoadOptions, Prompt, RgbImage, SUBJECT, Session, Subject,
};
use rawmakase_remote::{ClientError, RemoteAnalysis, RemoteSession};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Sender},
};
use std::time::Duration;

/// The longest side of the photo rendered for the model. The network looks at
/// 1024 pixels; more only makes the stored raster larger.
const INPUT_EDGE: u32 = 1280;

/// A selection to make, as the photo and edit were when it was asked for.
pub(super) struct Job {
    pub(super) load: u64,
    pub(super) generation: u64,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) image: Arc<CameraImage>,
    pub(super) recipe: Recipe,
    /// The model's folder, when the selection runs on this computer.
    pub(super) model: Option<PathBuf>,
    /// The remote worker to ask instead, when one is configured.
    pub(super) endpoint: Option<String>,
    pub(super) action: Action,
}

enum Message {
    Run(Box<Job>, Sender<Event>, egui::Context),
    /// Let go of the model, then say so.
    Unload(Sender<()>),
}

#[derive(Default)]
pub(super) struct Worker {
    messages: Option<Sender<Message>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Worker {
    /// Queues `job`; its outcome arrives as an [`Event::Selection`], whatever
    /// happens, so the drawer never waits on a thread that is gone.
    pub(super) fn submit(&mut self, job: Job, tx: Sender<Event>, ctx: egui::Context) {
        let (load, generation) = (job.load, job.generation);
        let failed = |why: &str| {
            let _ = tx.send(Event::Selection(Box::new(Done {
                load,
                generation,
                result: Err(Failure::Failed(why.into())),
            })));
        };
        if self.thread.as_ref().is_some_and(|t| t.is_finished()) {
            self.messages = None;
            self.thread = None;
        }
        if self.messages.is_none() {
            let (messages, inbox) = mpsc::channel();
            match std::thread::Builder::new()
                .name("subject-select".into())
                .spawn(move || run(inbox))
            {
                Ok(thread) => {
                    self.messages = Some(messages);
                    self.thread = Some(thread);
                }
                Err(e) => return failed(&format!("could not start the selection thread: {e}")),
            }
        }
        let sent = self
            .messages
            .as_ref()
            .is_some_and(|m| m.send(Message::Run(Box::new(job), tx.clone(), ctx)).is_ok());
        if !sent {
            self.messages = None;
            failed("the selection thread stopped");
        }
    }
    /// A closure that makes the thread release the model and waits (briefly) for it
    /// to; for removing the file off the UI thread.
    pub(super) fn unloader(&self) -> impl FnOnce() + Send + 'static + use<> {
        let messages = self.messages.clone();
        move || {
            if let Some(messages) = messages {
                let (ack, done) = mpsc::channel();
                if messages.send(Message::Unload(ack)).is_ok() {
                    // A run that cannot stop leaves the removal waiting only this long.
                    let _ = done.recv_timeout(Duration::from_secs(30));
                }
            }
        }
    }
    /// Closes the mailbox and returns the thread to wait for under the shutdown
    /// deadline.
    pub(super) fn stop(&mut self) -> Stopping {
        self.messages = None;
        Stopping::new(self.thread.take())
    }
}

fn run(inbox: mpsc::Receiver<Message>) {
    let mut state = State::default();
    for message in inbox {
        match message {
            Message::Unload(ack) => {
                state = State::default();
                let _ = ack.send(());
            }
            Message::Run(job, tx, ctx) => {
                let (load, generation) = (job.load, job.generation);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    select(&job, &mut state)
                }))
                .unwrap_or_else(|_| {
                    // The session may be half-built after a panic.
                    state = State::default();
                    Err(Failure::Failed("the selection stopped unexpectedly".into()))
                });
                let _ = tx.send(Event::Selection(Box::new(Done {
                    load,
                    generation,
                    result,
                })));
                ctx.request_repaint();
            }
        }
    }
}

/// The loaded model and the embedding of the photo last selected on. Over the
/// remote endpoint, the worker's answer for the photo last asked about, so its
/// clicks are answered from what it kept.
#[derive(Default)]
struct State {
    session: Option<Session>,
    analysis: Option<(String, Analysis)>,
    remote_analysis: Option<(String, RemoteAnalysis)>,
}

fn select(job: &Job, state: &mut State) -> Result<Generated, Failure> {
    let cancelled = || job.cancel.load(Ordering::Relaxed);
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    let input =
        crate::develop::masks::selection_input(&job.image, &job.recipe, INPUT_EDGE, &job.cancel)
            .map_err(|e| {
                if cancelled() {
                    Failure::Cancelled
                } else {
                    Failure::Failed(format!("{e:#}"))
                }
            })?;
    let rgb = RgbImage {
        width: input.width as usize,
        height: input.height as usize,
        data: input.rgb8(),
    };
    let key = input_key(&rgb);
    let (coverage, feature, nothing, input) = match &job.endpoint {
        Some(endpoint) => select_remotely(endpoint, &rgb, &key, job, state)?,
        None => select_locally(&rgb, &key, job, state)?,
    };
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    // Exactly nothing selected is the only emptiness decided here.
    if coverage.data.iter().all(|v| *v == 0) {
        return Err(nothing);
    }
    let (width, height) = (coverage.width as u32, coverage.height as u32);
    let id = mask_assets::register(Bitmap {
        width,
        height,
        channels: 1,
        depth: 1,
        data: coverage.data,
    })
    .map_err(|e| Failure::Failed(format!("{e:#}")))?;
    Ok(Generated {
        id,
        width,
        height,
        source: BitmapSource {
            feature: feature.provenance().into(),
            model: format!("{}@{}", SUBJECT.id, &SUBJECT.files[1].sha256[..16]),
            input,
        },
    })
}

/// What the local models make of the photo, and the prompt on it.
fn select_locally(
    rgb: &RgbImage,
    key: &str,
    job: &Job,
    state: &mut State,
) -> Result<(Coverage, Feature, Failure, String), Failure> {
    let Some(model) = &job.model else {
        return Err(Failure::ModelMissing);
    };
    if state.session.is_none() {
        state.session = Some(load(model)?);
    }
    let session = state.session.as_mut().expect("loaded above");
    if state.analysis.as_ref().is_none_or(|(k, _)| *k != key) {
        state.analysis = None;
        let analysis = session.analyze(rgb, &job.cancel).map_err(failure)?;
        state.analysis = Some((key.to_string(), analysis));
    }
    let (_, analysis) = state.analysis.as_ref().expect("analysed above");
    Ok(match &job.action {
        Action::Auto(Feature::Sky) => (
            analysis.sky().map_err(failure)?,
            Feature::Sky,
            Failure::NoSky,
            prompt_key(key, "sky", &Prompt::default()),
        ),
        Action::Auto(feature) => (
            analysis.subject().map_err(failure)?,
            *feature,
            Failure::NoSubject,
            prompt_key(key, "subject", &Prompt::default()),
        ),
        Action::Click(prompt) => (
            session
                .segment(analysis.embedding(), prompt, &job.cancel)
                .map_err(failure)?,
            Feature::Subject,
            Failure::NothingThere,
            prompt_key(key, "click", prompt),
        ),
    })
}

/// What the remote worker makes of the photo. The analysis it sent for this
/// photo is kept, so its clicks are answered without asking it to look again;
/// when it has forgotten the photo, it is asked once more.
fn select_remotely(
    endpoint: &str,
    rgb: &RgbImage,
    key: &str,
    job: &Job,
    state: &mut State,
) -> Result<(Coverage, Feature, Failure, String), Failure> {
    let session = RemoteSession::new(endpoint);
    // The worker keeps the photo's analysis under its own name; the one it
    // sent last is kept here, keyed the same way as a local one would be.
    let analysed = |state: &mut State| {
        if state
            .remote_analysis
            .as_ref()
            .is_none_or(|(k, _)| *k != key)
        {
            state.remote_analysis = None;
            let analysis = session.analyze(rgb, &job.cancel).map_err(remote_failure)?;
            state.remote_analysis = Some((key.to_string(), analysis));
        }
        Ok(state
            .remote_analysis
            .as_ref()
            .map(|(_, a)| a.clone())
            .expect("analysed above"))
    };
    match &job.action {
        Action::Auto(Feature::Sky) => {
            let analysis = analysed(state)?;
            Ok((
                analysis.sky,
                Feature::Sky,
                Failure::NoSky,
                prompt_key(key, "sky", &Prompt::default()),
            ))
        }
        Action::Auto(_) => {
            let analysis = analysed(state)?;
            Ok((
                analysis.subject,
                Feature::Subject,
                Failure::NoSubject,
                prompt_key(key, "subject", &Prompt::default()),
            ))
        }
        Action::Click(prompt) => {
            let analysis = analysed(state)?;
            let coverage = match session.segment(&analysis, prompt, &job.cancel) {
                Ok(coverage) => coverage,
                Err(ClientError::Expired) => {
                    // The worker keeps only the last photos: ask once more.
                    state.remote_analysis = None;
                    let analysis = analysed(state)?;
                    session
                        .segment(&analysis, prompt, &job.cancel)
                        .map_err(remote_failure)?
                }
                Err(e) => return Err(remote_failure(e)),
            };
            Ok((
                coverage,
                Feature::Subject,
                Failure::NothingThere,
                prompt_key(key, "click", prompt),
            ))
        }
    }
}

/// What a remote answer means to the drawer.
fn remote_failure(error: ClientError) -> Failure {
    match error {
        ClientError::Cancelled => Failure::Cancelled,
        ClientError::Unavailable(why) => Failure::RuntimeUnavailable(why),
        ClientError::Expired => Failure::Failed("the remote worker forgot this photo".into()),
        ClientError::Failed(why) => Failure::Failed(format!("the remote worker: {why}")),
    }
}

/// What the result was made from: the photo as the model saw it and the prompt.
fn prompt_key(input: &str, what: &str, prompt: &Prompt) -> String {
    let mut text = format!("{input}|{what}");
    for p in &prompt.points {
        text.push_str(&format!("|{:.4},{:.4},{}", p.x, p.y, p.positive));
    }
    if let Some(b) = prompt.bounds {
        text.push_str(&format!("|{b:.4?}"));
    }
    format!("{:016x}", fnv1a(FNV_OFFSET, text.as_bytes()))
}

fn load(model: &std::path::Path) -> Result<Session, Failure> {
    let bundled = super::models::runtime_dir();
    let runtime_library = rawmakase_inference::runtime::runtime_file_names()
        .iter()
        .map(|name| bundled.join(name))
        .find(|path| path.is_file());
    Subject::load_with(
        model,
        &LoadOptions {
            runtime_library,
            threads: None,
            // Selections made on this computer run on its CPU, as before; the
            // accelerators are the remote worker's concern.
            provider: None,
        },
    )
    .map_err(failure)
}

fn failure(error: InferenceError) -> Failure {
    match error {
        InferenceError::Cancelled => Failure::Cancelled,
        InferenceError::RuntimeUnavailable(why) => Failure::RuntimeUnavailable(why),
        InferenceError::ModelInvalid(why) => {
            Failure::Failed(format!("the installed model is not usable: {why}"))
        }
        InferenceError::Failed(why) | InferenceError::OutputInvalid(why) => Failure::Failed(why),
    }
}

/// Identifies what the model was given: the pixels, the model and the versions of
/// every step that shaped them. The same values execute the request.
fn input_key(rgb: &RgbImage) -> String {
    let header = [
        rgb.width.to_le_bytes().as_slice(),
        rgb.height.to_le_bytes().as_slice(),
        SUBJECT.id.as_bytes(),
        SUBJECT.files[1].sha256.as_bytes(),
        &SUBJECT.version.to_le_bytes(),
        &SUBJECT.processing_version.to_le_bytes(),
        &crate::develop::masks::SELECTION_INPUT_VERSION.to_le_bytes(),
    ]
    .concat();
    let hash = |seed| fnv1a(fnv1a(seed, &header), &rgb.data);
    format!("{:016x}{:016x}", hash(FNV_OFFSET), hash(0x84222325cbf29ce4))
}

/// Whether an ONNX Runtime library this copy can load is in a place it looks.
pub(super) fn runtime_present() -> bool {
    let names = rawmakase_inference::runtime::runtime_file_names();
    let mut dirs = rawmakase_inference::runtime::runtime_search_dirs();
    dirs.push(super::models::runtime_dir());
    dirs.iter()
        .any(|dir| names.iter().any(|name| dir.join(name).is_file()))
        || std::env::var_os(rawmakase_inference::RUNTIME_ENV).is_some_and(|v| !v.is_empty())
}

/// Where selections run, when the user has said they run elsewhere: the
/// endpoint of a `rawmakase-remote-worker`, from `remote-inference.json` in
/// this computer's data folder. Absent — the usual case — means the models run
/// here, as always. See docs/remote-inference.md.
///
/// ```json
/// { "endpoint": "127.0.0.1:8765" }
/// ```
#[derive(Default)]
pub(in crate::app) struct RemoteConfig {
    /// What the file said, and when: read again at most once a second, so the
    /// drawer notices an edit without stat on every frame. Tests set this
    /// directly.
    seen: std::cell::Cell<Option<(std::time::Instant, Option<String>)>>,
}

impl RemoteConfig {
    /// The endpoint to send selections to, if there is one.
    pub(super) fn endpoint(&self) -> Option<String> {
        match self.seen.take() {
            Some((at, endpoint)) if at.elapsed() < Duration::from_secs(1) => {
                self.seen.set(Some((at, endpoint.clone())));
                endpoint
            }
            _ => {
                let endpoint = read_endpoint();
                self.seen
                    .set(Some((std::time::Instant::now(), endpoint.clone())));
                endpoint
            }
        }
    }
    /// Pretends the file said this, for the tests; the next real read wins.
    #[cfg(test)]
    pub(super) fn force(&self, endpoint: Option<&str>) {
        self.seen.set(Some((
            std::time::Instant::now(),
            endpoint.map(str::to_string),
        )));
    }
}

/// The endpoint from the configuration file, when it is a usable one.
/// `RAWMAKASE_REMOTE_ENDPOINT` says the same thing for one launch, without
/// touching the file.
fn read_endpoint() -> Option<String> {
    if let Some(endpoint) = std::env::var_os("RAWMAKASE_REMOTE_ENDPOINT")
        .map(|v| v.to_string_lossy().trim().to_string())
        .filter(|text| !text.is_empty())
    {
        return Some(endpoint);
    }
    let path = crate::storage::local_data_dir().join("remote-inference.json");
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("endpoint")?
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty() && !text.chars().any(char::is_whitespace))
        .map(str::to_string)
}
