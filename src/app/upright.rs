//! The Transform panel's Upright analysis, run off the UI thread.
use super::{Editor, worker::Event};
use crate::develop::{Recipe, UprightMode};

/// What the analysis measures: the photo's orientation and lens correction.
fn inputs(r: &Recipe) -> (u8, bool, bool, bool, bool, u32) {
    (
        r.rotation,
        r.flip_x,
        r.flip_y,
        r.lens_builtin,
        r.lens_profile,
        r.lens_distortion.to_bits(),
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
        let base = self.document.recipe.clone();
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
    pub(super) fn ensure_upright(&mut self) {
        let u = &self.document.recipe.upright;
        if !matches!(u.mode, UprightMode::Off | UprightMode::Guided)
            && u.corrections.len() <= u.mode.code()
            && !self.document.upright.is_running()
        {
            self.start_upright();
        }
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
        if self.document.recipe.upright.corrections != analysed.upright.corrections {
            self.ensure_upright();
            return;
        }
        if inputs(analysed) != inputs(&self.document.recipe) {
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
        for r in
            std::iter::once(&mut self.document.recipe).chain(self.document.history.states_mut())
        {
            if fits(r) {
                // An imported Guided correction has no analysis to replace it.
                let guided = r
                    .upright
                    .corrections
                    .get(UprightMode::Guided.code())
                    .copied();
                r.upright.corrections = corrections.clone();
                r.upright.corrections.extend(guided);
                // Lightroom's own analysis details no longer describe these corrections.
                r.upright.lightroom.clear();
            }
        }
        self.document.save.mark_changed();
        self.schedule();
    }
}
