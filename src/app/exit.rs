//! Quitting, however it happens: the steps of docs/shutdown.md's exit sequence.
use super::{Editor, task};
use std::{sync::atomic::Ordering, time::Duration};

/// How long quitting waits for the stopped workers. The preview renderer can wait
/// up to 10 s for one GPU submission, but a quit that hangs is worse than a
/// render cut off; the loaders and exports write through temporary files.
const DEADLINE: Duration = Duration::from_secs(3);

impl Editor {
    /// Quit on macOS closes the window without a close request, so the close
    /// guard never sees it: the edit and the place in the catalog are saved here
    /// too. After a window close the guard has already flushed, and this finds
    /// nothing to do. Then the workers that write or hold the GPU are stopped and
    /// waited for, before eframe drops the device.
    pub(super) fn exit(&mut self) -> task::Waited {
        // Nothing is left to report a failure to: the edit stays as autosave last
        // saved it.
        let _ = self.flush();
        self.remember_place(super::workspace::LayoutEdit::Settled);
        self.cancel_jobs();
        task::wait_for(self.stop_workers(), DEADLINE)
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
    /// The workers the exit sequence waits for, asked to stop.
    fn stop_workers(&mut self) -> Vec<task::Stopping> {
        vec![
            self.loader.stop(),
            self.renderer.stop(),
            self.reference_loader.stop(),
            task::Stopping::new(self.exports.close()),
        ]
    }
}
