//! The Basic panel's Auto, estimated off the UI thread.
use super::{
    Editor,
    history::Step,
    worker::{AutoKind, Event},
};
use crate::develop::Recipe;

impl Editor {
    /// Starts Auto for the open photo; the estimate arrives as [`Event::Auto`]. Does
    /// nothing while the photo is still decoding or an estimate is already running.
    pub(super) fn start_auto(&mut self, kind: AutoKind) {
        let Some(im) = self.document.full().cloned() else {
            return;
        };
        if self.document.auto_running {
            return;
        }
        self.document.auto_running = true;
        let id = self.load.id();
        let base = self.document.recipe.clone();
        let tx = self.tx.clone();
        let ctx = self.context.clone();
        std::thread::spawn(move || {
            let result = match kind {
                AutoKind::Settings => crate::develop::auto_adjust(&im, &base),
                AutoKind::WhiteBalance => crate::develop::auto_white_balance(&im, &base),
            }
            .map(Box::new)
            .map_err(|e| format!("Auto: {e:#}"));
            let _ = tx.send(Event::Auto { id, kind, result });
            ctx.request_repaint();
        });
    }

    /// Applies an Auto estimate as one History step. Only the fields Auto sets are
    /// taken, so edits made while it ran are kept.
    pub(super) fn auto_ready(&mut self, kind: AutoKind, result: Result<Box<Recipe>, String>) {
        self.document.auto_running = false;
        let auto = match result {
            Ok(auto) => auto,
            Err(e) => {
                self.status = e;
                return;
            }
        };
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
                Step::new("Auto Settings", "")
            }
            AutoKind::WhiteBalance => Step::new("White Balance", "Auto"),
        };
        self.document.history.label(step);
        self.history(old);
        self.schedule();
    }
}
