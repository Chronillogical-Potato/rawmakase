//! The Transform panel's Upright analysis, run off the UI thread.
use super::{Editor, worker::Event};
use crate::develop::{Recipe, UprightMode};

/// What the analysis measures: the photo's orientation and lens correction.
pub(super) fn inputs(r: &Recipe) -> (u8, bool, bool, crate::develop::upright::LensInputs) {
    (
        r.rotation,
        r.flip_x,
        r.flip_y,
        crate::develop::upright::LensInputs::of(r),
    )
}

impl Editor {
    /// Analyses the open photo for Upright; the corrections arrive as
    /// [`Event::Upright`]. Does nothing while the photo is still decoding.
    pub(super) fn start_upright(&mut self) {
        let Some(im) = self.document.full().cloned() else {
            return;
        };
        let (generation, _) = self.document.upright.start();
        let id = self.load.id();
        let base = self.document.edit.recipe.clone();
        let tx = self.tx.clone();
        let ctx = self.context.clone();
        std::thread::spawn(move || {
            // A panic still sends a result, so Upright does not wait forever.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::develop::upright::analyse(&im, &base)
            }))
            .map_err(|_| "Upright: the analysis failed unexpectedly".to_owned());
            let _ = tx.send(Event::Upright {
                id,
                generation,
                analysed: Box::new(base),
                result,
            });
            ctx.request_repaint();
        });
    }

    /// Analyses the photo when an Upright mode is chosen but has no correction yet, as
    /// after undoing to a state from before an analysis, or opening such a photo.
    /// Guided's guides are solved once the other modes' corrections are there to sit
    /// beside its own.
    pub(super) fn ensure_upright(&mut self) {
        let u = &self.document.edit.recipe.upright;
        if !u.needs_analysis() || self.document.upright.is_running() {
            return;
        }
        if u.mode == UprightMode::Guided && u.corrections.len() == UprightMode::Guided.code() {
            if let Some(im) = self.document.full().cloned()
                && let Some(issue) =
                    crate::develop::guided::store(&mut self.document.edit.recipe, &im.metadata)
            {
                self.status = issue.message().into();
            }
            return;
        }
        self.start_upright();
    }

    /// Stores Upright's corrections for every mode. The mode was chosen when the analysis
    /// started, so the photo only changes if a mode is selected.
    pub(super) fn upright_ready(
        &mut self,
        generation: u64,
        analysed: &Recipe,
        result: Result<Vec<[f32; 9]>, String>,
    ) {
        if generation != self.document.upright.id() {
            return;
        }
        self.document.upright.finish(generation);
        // Settings applied meanwhile (a preset, a History step) may bring their own
        // corrections; those win.
        if self.document.edit.recipe.upright.corrections != analysed.upright.corrections {
            self.ensure_upright();
            return;
        }
        if inputs(analysed) != inputs(&self.document.edit.recipe) {
            self.start_upright();
            return;
        }
        let corrections = match result {
            Ok(c) => c,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        // The analysis is part of the photo, not an edit: every state in History that
        // it fits gets it too, so undoing the mode choice leaves nothing half-applied.
        // States with corrections of their own (imported from Lightroom, or another
        // analysis) keep them.
        let fits = |r: &Recipe| {
            inputs(r) == inputs(analysed) && r.upright.corrections == analysed.upright.corrections
        };
        let metadata = self.document.full().map(|im| im.metadata.clone());
        let mut issue = None;
        for (i, r) in std::iter::once(&mut self.document.edit.recipe)
            .chain(self.document.edit.history.states_mut())
            // A Before copied from the edit before the analysis arrived.
            .chain(self.document.before.as_mut())
            .enumerate()
        {
            if fits(r) {
                let solved = crate::develop::upright::store(r, &corrections, metadata.as_ref());
                // Only the photo as shown speaks in the status line.
                if i == 0 {
                    issue = solved;
                }
            }
        }
        if let Some(issue) = issue {
            self.status = issue.message().into();
        }
        self.document.edit.save.mark_changed();
        self.schedule();
    }
}
