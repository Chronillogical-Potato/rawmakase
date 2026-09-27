//! Lightroom's Masking panel: the selected mask and component, brush settings and the
//! drag in progress.

#[derive(Default)]
pub(super) struct MaskTool {
    /// Index into the recipe's masks.
    pub(super) selected: Option<usize>,
    /// Show the selected mask as a red overlay (O).
    pub(super) overlay: bool,
}
impl MaskTool {
    pub(super) fn clear_document(&mut self) {
        self.selected = None;
    }
}
