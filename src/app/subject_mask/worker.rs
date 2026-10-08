//! The thread that runs the selection model. One request at a time, one loaded
//! session reused between them, and the image embedding of the photo last asked
//! about, so a second click on it takes milliseconds instead of a second. Cancelling raises the request's flag, which the
//! runtime notices within a few milliseconds; quitting hands the thread to the shared
//! shutdown deadline rather than joining it, and the thread keeps its session until
//! it ends, so a native call is never left running on unloaded code.
use super::{Done, Failure, Generated};
use crate::app::task::Stopping;
use crate::app::worker::Event;
use crate::camera_data::CameraImage;
use crate::model::masks::{BitmapSource, FEATURE_SUBJECT};
use crate::model::recipe::Recipe;
use crate::storage::bitmaps::Bitmap;
use crate::storage::{FNV_OFFSET, fnv1a, mask_assets};
use eframe::egui;
use rawmakase_inference::{
    Embedding, InferenceError, LoadOptions, Prompt, RgbImage, SUBJECT, Session, Subject,
};
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
    /// The model's folder.
    pub(super) model: PathBuf,
    pub(super) prompt: Prompt,
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

/// The loaded model and the embedding of the photo last selected on.
#[derive(Default)]
struct State {
    session: Option<Session>,
    embedding: Option<(String, Embedding)>,
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
    if state.session.is_none() {
        state.session = Some(load(&job.model)?);
    }
    let session = state.session.as_mut().expect("loaded above");
    if state.embedding.as_ref().is_none_or(|(k, _)| *k != key) {
        state.embedding = None;
        let embedding = session.embed(&rgb, &job.cancel).map_err(failure)?;
        state.embedding = Some((key.clone(), embedding));
    }
    let (_, embedding) = state.embedding.as_ref().expect("embedded above");
    let coverage = session
        .segment(embedding, &job.prompt, &job.cancel)
        .map_err(failure)?;
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    // Exactly nothing selected is the only emptiness decided here.
    if coverage.data.iter().all(|v| *v == 0) {
        return Err(Failure::NoSubject);
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
            feature: FEATURE_SUBJECT.into(),
            model: format!("{}@{}", SUBJECT.id, &SUBJECT.files[1].sha256[..16]),
            input: prompt_key(&key, &job.prompt),
        },
    })
}

/// What the result was made from: the photo as the model saw it and the prompt.
fn prompt_key(input: &str, prompt: &Prompt) -> String {
    let mut text = input.to_string();
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
