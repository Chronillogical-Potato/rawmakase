//! The edit of the photo open in Develop, apart from the window that shows it:
//! its History and whether it still needs saving.
pub mod history;
pub mod save_state;

use crate::develop::Recipe;
use history::{History, Step};
use save_state::SaveState;

/// The edit of the photo open in Develop: its settings, the History of how they
/// came to be, and whether they still need saving.
#[derive(Default)]
pub struct EditSession {
    pub recipe: Recipe,
    pub history: History,
    pub save: SaveState,
}
impl EditSession {
    /// Records the change from `before` to the current settings as one History
    /// step, named `step` or for what changed, to be saved. Returns whether the
    /// settings changed.
    pub fn commit(&mut self, before: Recipe, step: Option<Step>) -> bool {
        if let Some(step) = step {
            self.history.label(step);
        }
        let changed = self.history.record(before, &self.recipe);
        if changed {
            self.save.mark_changed();
        }
        changed
    }
    /// What a frame of the editor did to the settings, from `before`: an edit is
    /// recorded once `gesture` (a drag, a dial turned) ends, as one step. Undo and
    /// History clicks are not recorded again. Returns whether the settings changed
    /// this frame, to be saved either way.
    pub fn observe(&mut self, before: Recipe, gesture: bool) -> bool {
        let changed = self.history.observe(before, &self.recipe, gesture);
        if changed {
            self.save.mark_changed();
        }
        changed
    }
}

/// A number that orders the changes made in the same frame, edits and catalog
/// commands alike.
pub fn sequence() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_is_one_named_step_to_save() {
        let mut session = EditSession::default();
        let before = session.recipe.clone();
        session.recipe.exposure = 1.;
        assert!(session.commit(before, Some(Step::new("Auto Settings", ""))));
        let (steps, applied) = session.history.steps();
        assert_eq!((steps[0].name.as_str(), applied), ("Auto Settings", 1));
        assert!(session.save.needs_save());
    }

    #[test]
    fn a_commit_that_changes_nothing_records_and_saves_nothing() {
        let mut session = EditSession::default();
        let before = session.recipe.clone();
        assert!(!session.commit(before, Some(Step::new("Straighten", "Auto"))));
        assert_eq!(session.history.steps().1, 0);
        assert!(!session.save.needs_save());
        // Its name is not left for the next edit.
        let before = session.recipe.clone();
        session.recipe.exposure = 1.;
        session.commit(before, None);
        assert_ne!(session.history.steps().0[0].name, "Straighten");
    }

    #[test]
    fn a_gesture_is_saved_as_it_goes_and_recorded_once_it_ends() {
        let mut session = EditSession::default();
        let start = session.recipe.clone();
        session.recipe.exposure = 0.5;
        assert!(session.observe(start, true));
        assert!(session.save.needs_save());
        assert_eq!(session.history.steps().1, 0);
        let mid = session.recipe.clone();
        session.recipe.exposure = 1.;
        session.observe(mid, false);
        assert_eq!(session.history.steps().1, 1);
        assert!(session.history.undo(&mut session.recipe));
        assert_eq!(session.recipe.exposure, 0.);
    }
}
