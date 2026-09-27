//! Lightroom's Masking panel: the selected mask and component, brush settings and the
//! drag in progress.
use eframe::egui;

#[derive(Default)]
pub(super) struct MaskTool {
    /// Index into the recipe's masks.
    pub(super) selected: Option<usize>,
    /// Show the selected mask as a red overlay (O).
    pub(super) overlay: bool,
}
impl super::Editor {
    /// The Masking panel's drawer.
    pub(super) fn mask_panel(&mut self, ui: &mut egui::Ui) {
        super::retouch_tool::hint(ui, "Masking arrives in the next step.");
    }
    /// Handles the pointer on the photo for the selected mask; returns whether the
    /// tool owns it.
    pub(super) fn mask_overlay(
        &mut self,
        _ui: &mut egui::Ui,
        _response: &eframe::egui::Response,
        _rect: eframe::egui::Rect,
    ) -> bool {
        false
    }
}
impl MaskTool {
    pub(super) fn clear_document(&mut self) {
        self.selected = None;
    }
}
