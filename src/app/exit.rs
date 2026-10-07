//! Quitting, however it happens: the steps of docs/shutdown.md's exit sequence.
use super::{Editor, task, workflow::Flushed};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

/// How long quitting waits for the edit to save and the stopped workers to end. The
/// preview renderer can wait up to 10 s for one GPU submission, but a quit that
/// hangs is worse than a render cut off; the loaders and exports write through
/// temporary files.
pub(super) const DEADLINE: Duration = Duration::from_secs(3);
/// How long quitting then waits for the temporary files of cut-off exports to be
/// deleted: they are on the folder an export stalled on, which can stall this too.
const CLEANUP: Duration = Duration::from_millis(500);

impl Editor {
    /// Quit on macOS closes the window without a close request, so the close
    /// guard never sees it: the edit and the place in the catalog are saved here
    /// too. After a window close the guard has already flushed, and this finds
    /// nothing to do. Then the workers that write or hold the GPU are stopped and
    /// waited for, before eframe drops the device.
    pub(super) fn exit(&mut self) -> task::Waited {
        self.exit_within(DEADLINE)
    }
    /// The exit sequence, with `deadline` for all of it. The workers wind down while
    /// the edit saves.
    pub(super) fn exit_within(&mut self, deadline: Duration) -> task::Waited {
        let until = Instant::now() + deadline;
        self.cancel_jobs();
        self.close_channels();
        let mut stopping = self.stop_workers();
        // Nothing but the terminal is left to say so in: the edit stays as autosave
        // last saved it.
        match self.flush_by(until) {
            Flushed::Saved => {}
            Flushed::Failed => eprintln!("{}", self.status),
            Flushed::Late => eprintln!(
                "Edits not saved: the catalog did not answer within {} s",
                deadline.as_secs_f32()
            ),
        }
        self.remember_place(super::workspace::LayoutEdit::Settled);
        // Only now: the save went through it.
        stopping.push(self.autosave.close());
        let waited = task::wait_for(stopping, until.saturating_duration_since(Instant::now()));
        // Exports cut off at the deadline end with the process, which leaves their
        // temporary files behind.
        let removing = std::thread::Builder::new()
            .name("remove-unfinished".into())
            .spawn(crate::export::remove_unfinished)
            .ok();
        task::wait_for(vec![task::Stopping::new(removing)], CLEANUP);
        waited
    }
    /// Every job in progress stops at its next check.
    fn cancel_jobs(&mut self) {
        self.load.invalidate();
        self.preview.task.invalidate();
        self.preview.before.task.invalidate();
        self.reference.cancel_load();
        self.prefetch_cancel.store(true, Ordering::Relaxed);
        self.automation.cancel_outputs();
    }
    /// Drops the channel ends workers wait on, so none waits for the interface.
    fn close_channels(&mut self) {
        self.updates.close();
        self.controls.close_requests();
    }
    /// The workers the exit sequence waits for, asked to stop.
    fn stop_workers(&mut self) -> Vec<task::Stopping> {
        let mut stopping = Vec::from(self.loader.stop());
        stopping.extend([
            self.renderer.stop(),
            self.reference_loader.stop(),
            task::Stopping::new(self.exports.close()),
        ]);
        stopping.extend(self.automation.stop_outputs());
        stopping.extend(self.controls.stop());
        if let Some(library) = &mut self.library {
            stopping.extend(library.close_previews());
        }
        stopping
    }
}
