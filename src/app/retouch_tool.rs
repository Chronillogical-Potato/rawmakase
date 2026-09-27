//! Spot removal (Lightroom's Remove panel, Heal and Clone modes): tool settings, the
//! selected spot and the drag in progress.
use crate::develop::retouch::RetouchMode;

pub(super) struct RetouchTool {
    pub(super) mode: RetouchMode,
    /// Brush size for new spots, as a fraction of the long edge.
    pub(super) size: f32,
    pub(super) feather: f32,
    pub(super) opacity: f32,
    /// Index into the recipe's retouch operations.
    pub(super) selected: Option<usize>,
    /// Hide the pins (H).
    pub(super) hide_pins: bool,
}
impl Default for RetouchTool {
    fn default() -> Self {
        Self {
            mode: RetouchMode::Heal,
            size: 0.012,
            feather: 0.5,
            opacity: 1.,
            selected: None,
            hide_pins: false,
        }
    }
}
impl RetouchTool {
    pub(super) fn clear_document(&mut self) {
        self.selected = None;
    }
}
