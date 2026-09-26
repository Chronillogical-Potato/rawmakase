//! Generation and cancellation belong to the operation that owns them.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

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
