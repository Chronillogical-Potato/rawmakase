//! Generation and cancellation belong to the operation that owns them; [`spawn`]
//! runs one-off background work so that a panic still reaches the UI.
use super::worker::{Event, panic_message};
use eframe::egui;
use std::{
    panic::{self, AssertUnwindSafe},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

/// Runs `work` on a thread of its own; it reports through the events it sends.
/// Should it panic, `failed` sends whatever the UI needs to stop waiting for it,
/// given the panic's message. Either way the UI repaints afterwards to read them.
pub(super) fn spawn(
    events: Sender<Event>,
    ctx: egui::Context,
    work: impl FnOnce(&Sender<Event>) + Send + 'static,
    failed: impl FnOnce(&Sender<Event>, String) + Send + 'static,
) {
    std::thread::spawn(move || {
        if let Err(panic) = panic::catch_unwind(AssertUnwindSafe(|| work(&events))) {
            let message = format!("Stopped unexpectedly: {}", panic_message(&*panic));
            failed(&events, message);
        }
        ctx.request_repaint();
    });
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Running,
}

#[derive(Default)]
pub(super) struct Task {
    generation: u64,
    cancel: Arc<AtomicBool>,
    phase: Phase,
}
impl Task {
    pub fn id(&self) -> u64 {
        self.generation
    }
    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Running)
    }
    pub fn start(&mut self) -> (u64, Arc<AtomicBool>) {
        self.invalidate();
        self.cancel = Arc::new(AtomicBool::new(false));
        self.phase = Phase::Running;
        (self.generation, self.cancel.clone())
    }
    pub fn invalidate(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.generation += 1;
        self.phase = Phase::Idle;
    }
    pub fn finish(&mut self, generation: u64) {
        if generation == self.generation {
            self.phase = Phase::Idle;
        }
    }
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn spawned_work_reports_its_panic() {
        let (tx, rx) = std::sync::mpsc::channel();
        spawn(
            tx,
            egui::Context::default(),
            |_| panic!("scan failed"),
            |events, message| {
                let _ = events.send(Event::CatalogWorking(message));
            },
        );
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Event::CatalogWorking(message)) => {
                assert_eq!(message, "Stopped unexpectedly: scan failed");
            }
            _ => panic!("no failure reported"),
        }
    }
    #[test]
    fn spawned_work_that_finishes_reports_nothing_more() {
        let (tx, rx) = std::sync::mpsc::channel();
        spawn(
            tx,
            egui::Context::default(),
            |events| {
                let _ = events.send(Event::DialogClosed);
            },
            |_, message| panic!("{message}"),
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10)),
            Ok(Event::DialogClosed)
        ));
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_err());
    }
    #[test]
    fn superseded_and_dropped_tasks_are_cancelled_and_stale_completion_is_ignored() {
        let mut task = Task::default();
        let (first, cancelled) = task.start();
        let (second, active) = task.start();
        assert!(cancelled.load(Ordering::Relaxed));
        assert!(!active.load(Ordering::Relaxed));
        task.finish(first);
        assert!(task.is_running());
        task.finish(second);
        assert!(!task.is_running());
        let (_, active) = task.start();
        drop(task);
        assert!(active.load(Ordering::Relaxed));
    }
}
