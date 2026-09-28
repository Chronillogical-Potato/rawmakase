//! Autosave writes on a thread of its own, with its own catalog connection:
//! a catalog commit waits for the disk (fsync), which on a busy disk took long
//! enough to stall the interface mid-edit. Saves before navigation stay
//! synchronous, after waiting for the one in flight.
use crate::{catalog::Catalog, develop::Recipe, export::ExportOptions};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
};

/// An edit to write, as it was when the save started.
pub(super) struct Job {
    pub target: Target,
    pub raw: PathBuf,
    pub recipe: Recipe,
    pub export: ExportOptions,
}
pub(super) enum Target {
    Catalog {
        path: PathBuf,
        photo: i64,
    },
    /// A sidecar beside the RAW, for photos opened outside a catalog.
    Sidecar,
}
/// Where the edit was saved, or why it was not.
pub(super) type Done = Result<PathBuf, String>;

#[derive(Default)]
pub(super) struct Autosave {
    worker: Option<(Sender<Job>, Receiver<Done>)>,
    in_flight: bool,
}
impl Autosave {
    /// Starts saving `job`, or hands it back if the saver cannot run.
    pub fn submit(&mut self, job: Job, ctx: &egui::Context) -> Result<(), Box<Job>> {
        debug_assert!(!self.in_flight);
        if self.worker.is_none() {
            let (jobs, rx) = mpsc::channel();
            let (tx, done) = mpsc::channel();
            let ctx = ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("autosave".into())
                .spawn(move || run(rx, tx, ctx));
            if spawned.is_err() {
                return Err(Box::new(job));
            }
            self.worker = Some((jobs, done));
        }
        let (jobs, _) = self.worker.as_ref().expect("started above");
        if let Err(mpsc::SendError(job)) = jobs.send(job) {
            self.worker = None;
            return Err(Box::new(job));
        }
        self.in_flight = true;
        Ok(())
    }
    pub fn busy(&self) -> bool {
        self.in_flight
    }
    /// The finished save, if one finished.
    pub fn poll(&mut self) -> Option<Done> {
        self.receive(|done| done.try_recv().ok())
    }
    /// The save in flight, once it finishes.
    pub fn wait(&mut self) -> Option<Done> {
        self.receive(|done| Some(done.recv().unwrap_or_else(|_| Err(LOST.into()))))
    }
    fn receive(&mut self, get: impl FnOnce(&Receiver<Done>) -> Option<Done>) -> Option<Done> {
        if !self.in_flight {
            return None;
        }
        let (_, done) = self.worker.as_ref()?;
        let result = get(done)?;
        self.in_flight = false;
        if result.as_ref().is_err_and(|e| e == LOST) {
            self.worker = None;
        }
        Some(result)
    }
}

const LOST: &str = "The autosave thread stopped";

fn run(jobs: Receiver<Job>, done: Sender<Done>, ctx: egui::Context) {
    let mut catalog = None;
    for job in jobs {
        let result = save(&mut catalog, &job).map_err(|e| e.to_string());
        if done.send(result).is_err() {
            return;
        }
        ctx.request_repaint();
    }
}

/// Keeps the catalog open between saves.
fn save(catalog: &mut Option<Catalog>, job: &Job) -> anyhow::Result<PathBuf> {
    match &job.target {
        Target::Sidecar => crate::storage::save(&job.raw, &job.recipe, &job.export),
        Target::Catalog { path, photo } => {
            if catalog.as_ref().is_none_or(|c: &Catalog| &c.path != path) {
                *catalog = None;
                *catalog = Some(Catalog::open(path)?);
            }
            let c = catalog.as_ref().expect("opened above");
            c.save_edit(*photo, &job.raw, &job.recipe, &job.export)?;
            Ok(path.clone())
        }
    }
}
