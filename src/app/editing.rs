//! A frame edits exactly one document generation, even when navigation happens mid-frame.
use super::Editor;
use crate::develop::Recipe;
use eframe::egui;

pub(super) struct EditFrame {
    generation: u64,
    recipe: Recipe,
    modes: [bool; 4],
    aspect: f32,
    export: (u8, u32),
}
impl Editor {
    fn render_modes(&self) -> [bool; 4] {
        [
            self.view.crop_mode,
            self.view.clipping,
            self.view.compare,
            self.view.zoom100,
        ]
    }
    pub(super) fn begin_edit_frame(&mut self) -> EditFrame {
        self.document.history.begin_frame();
        EditFrame {
            generation: self.load.id(),
            recipe: self.document.recipe.clone(),
            modes: self.render_modes(),
            aspect: self.view.aspect,
            export: (self.document.export.quality, self.document.export.max_edge),
        }
    }
    pub(super) fn finish_edit_frame(&mut self, frame: EditFrame, ctx: &egui::Context) {
        let step =
            ctx.data_mut(|d| d.remove_temp::<(String, String)>(super::widgets::history_step_id()));
        if frame.generation != self.load.id() {
            return;
        }
        if let Some((name, value)) = step {
            self.document
                .history
                .label(super::history::Step::new(name, value));
        }
        if frame.aspect != self.view.aspect {
            self.fit_aspect();
        }
        let edited = self.document.history.observe(
            frame.recipe,
            &self.document.recipe,
            ctx.input(|i| i.pointer.primary_down()),
        );
        if edited {
            self.document.save.mark_changed();
        }
        if edited || frame.modes != self.render_modes() {
            self.schedule();
        }
        if frame.export != (self.document.export.quality, self.document.export.max_edge) {
            self.document.save.mark_changed();
        }
    }
}
