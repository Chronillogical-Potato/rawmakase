//! Bounded output jobs capture a recipe and its identity before leaving the UI
//! thread. Polling a job confirms the file was published, not just queued.
use super::{Error, Result};
use crate::export::{ExportSettings, Format, Replace, job};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum Status {
    Running,
    Completed,
    Failed,
}
#[derive(Clone, Serialize)]
struct OutputState {
    job_id: u64,
    status: Status,
    generation: u64,
    revision: u64,
    path: PathBuf,
    progress: f32,
    notice: Option<String>,
    error: Option<String>,
}
struct Job {
    state: Arc<Mutex<OutputState>>,
    cancel: Arc<AtomicBool>,
}
#[derive(Default)]
pub(super) struct Outputs {
    next: u64,
    jobs: BTreeMap<u64, Job>,
}
impl Drop for Outputs {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}
impl Outputs {
    pub fn state(&self, id: u64) -> Result<Value> {
        self.jobs
            .get(&id)
            .map(|j| {
                serde_json::to_value(
                    &*j.state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                )
                .expect("output state")
            })
            .ok_or_else(|| {
                Error::new(
                    "unknown_job",
                    "The output job does not exist or has expired",
                )
            })
    }
    pub fn start(
        &mut self,
        photo: job::Photo,
        path: PathBuf,
        edge: u32,
        generation: u64,
        revision: u64,
        ctx: eframe::egui::Context,
    ) -> Result<Value> {
        let active = self
            .jobs
            .values()
            .filter(|j| {
                j.state
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .status
                    == Status::Running
            })
            .count();
        if active >= 2 {
            return Err(Error::new("busy", "Two output jobs are already running"));
        }
        let format = Format::from_path(&path).ok_or_else(|| {
            Error::new(
                "invalid_path",
                "Use a .jpg, .jpeg, .tif or .tiff output path",
            )
        })?;
        if !path.is_absolute() {
            return Err(Error::new("invalid_path", "Output path must be absolute"));
        }
        if path.exists() {
            return Err(Error::new(
                "already_exists",
                "Output already exists; choose a new path",
            ));
        }
        if !path.parent().is_some_and(|p| p.is_dir()) {
            return Err(Error::new(
                "invalid_path",
                "Output directory does not exist",
            ));
        }
        while self.jobs.len() >= 32 {
            let old = self
                .jobs
                .iter()
                .find(|(_, j)| {
                    j.state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .status
                        != Status::Running
                })
                .map(|(id, _)| *id);
            if let Some(id) = old {
                self.jobs.remove(&id);
            } else {
                break;
            }
        }
        self.next += 1;
        let id = self.next;
        let initial = OutputState {
            job_id: id,
            status: Status::Running,
            generation,
            revision,
            path: path.clone(),
            progress: 0.,
            notice: None,
            error: None,
        };
        let state = Arc::new(Mutex::new(initial.clone()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (report, stop) = (state.clone(), cancel.clone());
        std::thread::Builder::new()
            .name("control-output".into())
            .spawn(move || {
                let settings = ExportSettings {
                    format,
                    resize: edge > 0,
                    long_edge: edge.max(1),
                    ..Default::default()
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    job::run(photo, &settings, &path, Replace::NoClobber, &stop, |p| {
                        report
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .progress = p;
                    })
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Output rendering failed unexpectedly")));
                let mut report = report
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match result {
                    Ok(notice) => {
                        report.status = Status::Completed;
                        report.notice = notice;
                        report.progress = 1.;
                    }
                    Err(e) => {
                        report.status = Status::Failed;
                        report.error = Some(format!("{e:#}"));
                    }
                }
                ctx.request_repaint();
            })
            .map_err(|e| Error::new("start_failed", e.to_string()))?;
        self.jobs.insert(id, Job { state, cancel });
        Ok(serde_json::to_value(initial).expect("output state"))
    }
}
