//! A frame edits exactly one document generation, even when navigation happens mid-frame.
use super::Editor;
use crate::develop::Recipe;
use eframe::egui;

pub(super) struct EditFrame {
    generation: u64,
    recipe: Recipe,
    modes: RenderModes,
    overlay: super::worker::Overlay,
    aspect: f32,
    export: (u8, u32),
}
/// The view modes a render depends on.
#[derive(Clone, Copy, PartialEq, Eq)]
struct RenderModes {
    crop: bool,
    clipping: crate::develop::ClipOverlay,
    compare: bool,
    zoom: bool,
}
impl Editor {
    fn render_modes(&self) -> RenderModes {
        RenderModes {
            crop: self.view.is(super::state::Tool::Crop),
            clipping: self.view.clipping.overlay(),
            compare: self.view.compare,
            zoom: self.view.zoom.on,
        }
    }
    pub(super) fn begin_edit_frame(&mut self) -> EditFrame {
        self.document.history.begin_frame();
        // Before the frame looks at it, so reading it changes no crop.
        self.read_aspect();
        let frame = EditFrame {
            generation: self.load.id(),
            recipe: self.document.recipe.clone(),
            modes: self.render_modes(),
            overlay: self.overlay(),
            aspect: self.view.aspect,
            export: (self.document.export.quality, self.document.export.max_edge),
        };
        // The histogram sets it again while a triangle stays hovered, so the
        // warning goes once the pointer leaves or the histogram is not drawn.
        self.view.clipping.set_hover(None);
        frame
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
        // The Guided tool goes with the mode, however it was left: a reset, an undo, a
        // preset, with the Transform panel open or not.
        if self.view.is(super::state::Tool::Guided)
            && self.document.recipe.upright.mode != crate::develop::UprightMode::Guided
        {
            self.view.tool = super::state::Tool::None;
        }
        let edited = self.document.history.observe(
            frame.recipe,
            &self.document.recipe,
            ctx.input(|i| i.pointer.primary_down()),
        );
        if edited {
            self.document.save.mark_changed();
        }
        if edited || frame.modes != self.render_modes() || frame.overlay != self.overlay() {
            self.schedule();
        }
        if frame.export != (self.document.export.quality, self.document.export.max_edge) {
            self.document.save.mark_changed();
        }
    }
}
