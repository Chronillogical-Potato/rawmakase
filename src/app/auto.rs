//! The Basic panel's Auto, estimated off the UI thread.
use super::{
    Editor,
    history::Step,
    worker::{AutoKind, Event},
};
use crate::develop::Recipe;

/// Everything an estimate was fitted against: `r` without the settings Auto chooses.
fn inputs(r: &Recipe) -> Recipe {
    Recipe {
        wb: [1.; 3],
        temperature: 0.,
        tint: 0.,
        exposure: 0.,
        contrast: 0.,
        highlights: 0.,
        shadows: 0.,
        whites: 0.,
        blacks: 0.,
        ..r.clone()
    }
}

impl Editor {
    /// Whether the photo is exactly as Auto last left it, so running it again would
    /// change nothing. Any other edit can change what Auto measures, so it counts too.
    pub(super) fn auto_in_effect(&self) -> bool {
        self.document.auto_applied.as_ref() == Some(&self.document.recipe)
    }

    /// Starts Auto for the open photo; the estimate arrives as [`Event::Auto`]. Does
    /// nothing while the photo is still decoding or an estimate is already running.
    pub(super) fn start_auto(&mut self, kind: AutoKind) {
        let Some(im) = self.document.full().cloned() else {
            return;
        };
        if self.document.auto.is_running() {
            return;
        }
        let (_, cancel) = self.document.auto.start();
        let id = self.load.id();
        let base = self.document.recipe.clone();
        self.document.auto_input = Some(inputs(&base));
        let tx = self.tx.clone();
        let ctx = self.context.clone();
        std::thread::spawn(move || {
            // A panic still sends a result, so Auto does not stay disabled.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match kind {
                AutoKind::Settings => crate::develop::auto_adjust_cancellable(&im, &base, &cancel),
                AutoKind::WhiteBalance => {
                    crate::develop::auto_white_balance_cancellable(&im, &base, &cancel)
                }
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the estimate failed unexpectedly")))
            .map(Box::new)
            .map_err(|e| format!("Auto: {e:#}"));
            let _ = tx.send(Event::Auto { id, kind, result });
            ctx.request_repaint();
        });
    }

    /// Applies an Auto estimate as one History step. Only the fields Auto sets are
    /// taken, so edits made while it ran are kept.
    pub(super) fn auto_ready(&mut self, kind: AutoKind, result: Result<Box<Recipe>, String>) {
        self.document.auto.invalidate();
        // The estimate was fitted to the photo as it was when Auto started. If any other
        // setting changed since, it no longer fits, and neither does an error it hit:
        // run again with the edit kept.
        let fitted = self
            .document
            .auto_input
            .take()
            .or_else(|| result.as_deref().ok().map(inputs));
        if fitted.is_some_and(|f| f != inputs(&self.document.recipe)) {
            self.start_auto(kind);
            return;
        }
        let auto = match result {
            Ok(auto) => auto,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        // A drag still under way is recorded first, so undoing it keeps Auto.
        if self.document.history.in_gesture() {
            self.document.history.finish_gesture(&self.document.recipe);
            self.document.save.mark_changed();
        }
        let old = self.document.recipe.clone();
        let r = &mut self.document.recipe;
        r.wb = auto.wb;
        r.temperature = auto.temperature;
        r.tint = auto.tint;
        let step = match kind {
            AutoKind::Settings => {
                r.exposure = auto.exposure;
                r.contrast = auto.contrast;
                r.highlights = auto.highlights;
                r.shadows = auto.shadows;
                r.whites = auto.whites;
                r.blacks = auto.blacks;
                self.document.auto_applied = Some(r.clone());
                Step::new("Auto Settings", "")
            }
            AutoKind::WhiteBalance => Step::new("White Balance", "Auto"),
        };
        self.document.history.label(step);
        self.history(old);
        self.schedule();
    }
}
