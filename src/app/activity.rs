//! Mutually exclusive foreground operations. Background previews are independent.
#[derive(Default)]
pub(super) enum Activity {
    #[default]
    Idle,
    ChoosingFile,
    /// Sync Settings is writing other photos' edits: moving to another photo or
    /// catalog waits for it.
    Syncing,
}
impl Activity {
    pub(crate) fn is_busy(&self) -> bool {
        !matches!(self, Self::Idle)
    }
    pub(crate) fn is_dialog(&self) -> bool {
        matches!(self, Self::ChoosingFile)
    }
    pub(crate) fn begin_dialog(&mut self) -> bool {
        if self.is_busy() {
            return false;
        }
        *self = Self::ChoosingFile;
        true
    }
    pub(crate) fn begin_sync(&mut self) -> bool {
        if self.is_busy() {
            return false;
        }
        *self = Self::Syncing;
        true
    }
    pub(crate) fn is_syncing(&self) -> bool {
        matches!(self, Self::Syncing)
    }
    pub(crate) fn finish_sync(&mut self) {
        if self.is_syncing() {
            *self = Self::Idle;
        }
    }
    pub(crate) fn finish_dialog(&mut self) {
        if self.is_dialog() {
            *self = Self::Idle;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_file_dialog_blocks_another_until_it_closes() {
        let mut activity = Activity::default();
        assert!(activity.begin_dialog());
        assert!(!activity.begin_dialog());
        activity.finish_dialog();
        assert!(!activity.is_busy());
    }
    #[test]
    fn a_sync_keeps_dialogs_and_other_syncs_waiting() {
        let mut activity = Activity::default();
        assert!(activity.begin_sync());
        assert!(activity.is_busy() && !activity.is_dialog());
        assert!(!activity.begin_dialog() && !activity.begin_sync());
        activity.finish_dialog();
        assert!(activity.is_syncing());
        activity.finish_sync();
        assert!(!activity.is_busy());
    }
}
