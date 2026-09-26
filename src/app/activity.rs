//! Mutually exclusive foreground operations. Background previews are independent.
use std::path::PathBuf;

#[derive(Default)]
pub(super) enum Activity {
    #[default]
    Idle,
    ChoosingFile,
    ConfirmExport(PathBuf),
    Exporting,
}
impl Activity {
    pub fn is_busy(&self) -> bool {
        !matches!(self, Self::Idle)
    }
    pub fn is_dialog(&self) -> bool {
        matches!(self, Self::ChoosingFile)
    }
    pub fn is_exporting(&self) -> bool {
        matches!(self, Self::Exporting)
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
    pub fn await_overwrite(&mut self, path: PathBuf) -> bool {
        if self.is_busy() {
            return false;
        }
        *self = Self::ConfirmExport(path);
        true
    }
    pub fn pending_export(&self) -> Option<&PathBuf> {
        if let Self::ConfirmExport(path) = self {
            Some(path)
        } else {
            None
        }
    }
    pub fn cancel_overwrite(&mut self) {
        if matches!(self, Self::ConfirmExport(_)) {
            *self = Self::Idle;
        }
    }
    pub fn begin_export(&mut self) -> bool {
        if self.is_busy() {
            return false;
        }
        *self = Self::Exporting;
        true
    }
    pub fn finish_export(&mut self) {
        if self.is_exporting() {
            *self = Self::Idle;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_overwrite_blocks_other_operations_and_unrelated_completions() {
        let mut activity = Activity::default();
        assert!(activity.begin_dialog());
        assert!(!activity.begin_export());
        activity.finish_dialog();
        activity.await_overwrite("photo.jpg".into());
        assert!(!activity.begin_dialog());
        assert!(!activity.begin_export());
        activity.finish_export();
        activity.finish_dialog();
        assert_eq!(activity.pending_export(), Some(&PathBuf::from("photo.jpg")));
        activity.cancel_overwrite();
        assert!(activity.begin_export());
        assert!(!activity.begin_dialog());
        assert!(!activity.await_overwrite("another.jpg".into()));
        activity.finish_export();
        assert!(!activity.is_busy());
    }
}
