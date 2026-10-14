//! The one whole-window mode: F hides the workspace bar, the panels, the
//! filmstrip and the views' toolbars, so the photo, or the grid, fills the
//! window on its own backdrop. F again, or Escape, brings them back. The window
//! itself is left alone: no display is filled and nothing about it changes, as
//! Lightroom's screen modes do.
use super::Module;

/// The module whose chrome `F` hid; the normal window in `None`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Chrome {
    hidden: Option<Module>,
}

impl Chrome {
    /// Whether `module`'s chrome is hidden, leaving only the photo.
    pub(super) fn hidden(self, module: Module) -> bool {
        self.hidden == Some(module)
    }
    /// Whether `module`'s chrome is drawn.
    pub(super) fn shown(self, module: Module) -> bool {
        !self.hidden(module)
    }
    /// F, or Escape while it is hidden: hide `module`'s chrome, or bring it
    /// back. Only what is drawn changes: the panels keep the layout they are
    /// saved with.
    pub(super) fn toggle(&mut self, module: Module) {
        self.hidden = match self.hidden {
            Some(hidden) if hidden == module => None,
            _ => Some(module),
        };
    }
    /// Enters `module`, whose chrome is drawn again: the mode belongs to the
    /// module it was turned on in, so G and D always land on a window with
    /// its bars, not on a hidden one.
    pub(super) fn entered(&mut self, module: Module) {
        if self.hidden.is_some_and(|hidden| hidden != module) {
            self.hidden = None;
        }
    }
}
