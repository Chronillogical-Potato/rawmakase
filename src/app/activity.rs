//! Mutually exclusive foreground operations. Background previews are independent.
#[derive(Default)]
pub(super) enum Activity {
    #[default]
    Idle,
    ChoosingFile,
}
impl Activity {
    pub fn is_busy(&self) -> bool {
        !matches!(self, Self::Idle)
    }
    pub fn is_dialog(&self) -> bool {
        matches!(self, Self::ChoosingFile)
    }
    pub fn begin_dialog(&mut self) -> bool {
        if self.is_busy() {
            return false;
        }
        *self = Self::ChoosingFile;
        true
    }
    pub fn finish_dialog(&mut self) {
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
}
