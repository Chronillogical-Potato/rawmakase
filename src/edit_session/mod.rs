//! The edit of the photo open in Develop, apart from the window that shows it:
//! its History and whether it still needs saving.
pub mod history;
pub mod save_state;

use crate::model::recipe::Recipe;
use history::{History, Step};
use save_state::SaveState;

/// Whether a drag, a wheel scroll or a dial turn is still changing the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    /// Still under way: its change becomes one step once it ends.
    Held,
    /// None, or it ended this frame.
    Released,
}

/// The settings as a frame of the editor found them, from [`EditSession::begin`].
pub struct Frame {
    before: Recipe,
}
impl Frame {
    pub fn before(&self) -> &Recipe {
        &self.before
    }
}

/// What a frame did to the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameOutcome {
    Unchanged,
    /// Changed, by an edit, Undo or a History click, and marked for saving.
    Edited,
}

/// The edit of the photo open in Develop: its settings, the History of how they
/// came to be, and whether they still need saving.
#[derive(Default)]
pub struct EditSession {
    recipe: Recipe,
    history: History,
    save: SaveState,
    /// An editor frame is open (see [`EditSession::begin`]): the settings may be
    /// changed in place, and the frame records the change when it finishes.
    frame_open: bool,
}
impl EditSession {
    /// The settings.
    pub fn recipe(&self) -> &Recipe {
        &self.recipe
    }
    /// The settings to change in place, while an editor frame is open: the frame
    /// records the change as a History step, marks it for saving and turns on a
    /// switched-off panel it changed. Outside a frame, [`Self::change`] does that.
    pub fn recipe_mut(&mut self) -> &mut Recipe {
        // Tests drive the panels' handlers directly, with no frame around them.
        debug_assert!(
            self.frame_open || cfg!(test),
            "the settings change in place only inside an editor frame; use change()"
        );
        &mut self.recipe
    }
    /// Changes the settings outside an editor frame (a command, a preset, a result
    /// computed off the UI thread) as one History step, `step` or one named for what
    /// changed, to be saved. Returns what `edit` returns.
    pub fn change<T>(&mut self, step: Option<Step>, edit: impl FnOnce(&mut Recipe) -> T) -> T {
        let before = self.recipe.clone();
        let out = edit(&mut self.recipe);
        // A change that changed nothing is no step, and names none.
        if self.recipe != before {
            self.commit(before, step);
        }
        out
    }
    /// The settings, to store what was analysed from the photo (Upright's
    /// corrections, Guided's solution) rather than an edit: no History step.
    pub fn analysed_mut(&mut self) -> &mut Recipe {
        &mut self.recipe
    }
    /// Sets `id` to `value` with what a change to it implies (see
    /// [`crate::model::edit::setting_changed`]), inside an editor frame or a
    /// [`Self::change`].
    pub fn set_parameter(
        recipe: &mut Recipe,
        id: crate::model::params::ParameterId,
        value: f32,
        photo: Option<&crate::camera_data::Metadata>,
    ) {
        let previous = *id.value_mut(recipe);
        *id.value_mut(recipe) = value;
        crate::model::edit::setting_changed(recipe, id, previous, photo);
    }
    /// Starts over with `recipe`, recording nothing: a photo opened, a snapshot or
    /// History state shown, settings read back. History and save state are the
    /// caller's to set alongside, as the case needs.
    pub fn replace(&mut self, recipe: Recipe) -> Recipe {
        std::mem::replace(&mut self.recipe, recipe)
    }
    /// Ends a gesture still held (a drag, a wheel scroll), as one History step.
    pub fn finish_gesture(&mut self) {
        self.history.finish_gesture(&self.recipe);
    }
    /// The History read back from `saved`, for the current settings.
    pub fn restore_history(&mut self, saved: crate::model::saved_history::SavedHistory) {
        self.history = History::restored(saved, &self.recipe);
    }
    /// Returns the settings to `target`, the state History has at `at`, as the step
    /// `step` (Undo of a catalog command, say).
    pub fn restore(&mut self, at: history::Mark, target: &Recipe, step: Step) {
        self.history.restore(at, target, &mut self.recipe, step);
    }
    /// Sets the settings to `target` as History step `step`.
    pub fn set(&mut self, target: &Recipe, step: Step) {
        self.history.set(target, &mut self.recipe, step);
    }
    /// The settings and every state History keeps, to bring all up to date at once
    /// (Upright's analysis arriving, say).
    pub fn states_mut(&mut self) -> impl Iterator<Item = &mut Recipe> {
        std::iter::once(&mut self.recipe).chain(self.history.states_mut())
    }
    /// The History of how the settings came to be.
    pub fn history(&self) -> &History {
        &self.history
    }
    /// The History, to name, end or replace steps; stepping through it moves the
    /// settings with it (see [`Self::undo`] and [`Self::jump`]).
    pub fn history_mut(&mut self) -> &mut History {
        &mut self.history
    }
    /// Undoes the latest History step on the settings.
    pub fn undo(&mut self) -> bool {
        self.history.undo(&mut self.recipe)
    }
    /// Redoes the next History step on the settings.
    pub fn redo(&mut self) -> bool {
        self.history.redo(&mut self.recipe)
    }
    /// Shows History state `applied` (steps applied), as a History click does.
    pub fn jump(&mut self, applied: usize) -> bool {
        self.history.jump(applied, &mut self.recipe)
    }
    /// Whether the settings still need saving, and why not if they can't be.
    pub fn save_state(&self) -> &SaveState {
        &self.save
    }
    /// The save state, for autosave and for settings saved with the edit that are
    /// not History steps (export options).
    pub fn save_state_mut(&mut self) -> &mut SaveState {
        &mut self.save
    }
    /// Records the change from `before` to the current settings as one History
    /// step, named `step` or for what changed, to be saved. Returns whether the
    /// settings changed.
    pub fn commit(&mut self, before: Recipe, step: Option<Step>) -> bool {
        crate::model::edit::turn_on_edited_panel(&before, &mut self.recipe);
        if let Some(step) = step {
            self.history.label(step);
        }
        let changed = self.history.record(before, &self.recipe);
        if changed {
            self.save.mark_changed();
        }
        changed
    }
    /// Starts a frame of the editor, which may edit the settings.
    pub fn begin(&mut self) -> Frame {
        self.frame_open = true;
        self.history.begin_frame();
        Frame {
            before: self.recipe.clone(),
        }
    }
    /// Ends `frame`: an edit made while a `gesture` is held is recorded once it is
    /// released, as one step; Undo and History clicks are not recorded again. An
    /// edit that changed only a switched-off panel turns it on.
    pub fn finish(&mut self, frame: Frame, gesture: Gesture) -> FrameOutcome {
        self.frame_open = false;
        if !self.history.is_replaying() {
            crate::model::edit::turn_on_edited_panel(&frame.before, &mut self.recipe);
        }
        let held = gesture == Gesture::Held;
        if self.history.observe(frame.before, &self.recipe, held) {
            self.save.mark_changed();
            FrameOutcome::Edited
        } else {
            FrameOutcome::Unchanged
        }
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
    fn a_change_outside_a_frame_is_one_named_step_to_save() {
        let mut session = EditSession::default();
        let returned = session.change(Some(Step::new("Auto Settings", "")), |r| {
            r.exposure = 1.;
            r.exposure
        });
        assert_eq!(returned, 1.);
        assert_eq!(session.recipe().exposure, 1.);
        let (steps, applied) = session.history().steps();
        assert_eq!((steps[0].name.as_str(), applied), ("Auto Settings", 1));
        assert!(session.save_state().needs_save());
        assert!(session.undo());
        assert_eq!(session.recipe().exposure, 0.);
    }

    #[test]
    fn a_change_that_changes_nothing_records_and_saves_nothing() {
        let mut session = EditSession::default();
        session.change(Some(Step::new("Straighten", "Auto")), |r| r.exposure = 0.);
        assert_eq!(session.history().steps().1, 0);
        assert!(!session.save_state().needs_save());
    }

    #[test]
    fn what_was_analysed_is_stored_with_no_step() {
        let mut session = EditSession::default();
        session.analysed_mut().upright.corrections = vec![[0.; 9]];
        assert_eq!(session.history().steps().1, 0);
        assert!(!session.save_state().needs_save());
    }

    #[test]
    fn a_frame_records_what_changed_in_it() {
        let mut session = EditSession::default();
        let frame = session.begin();
        session.recipe_mut().exposure = 1.;
        assert_eq!(
            session.finish(frame, Gesture::Released),
            FrameOutcome::Edited
        );
        assert_eq!(session.history().steps().1, 1);
        assert!(session.save_state().needs_save());
    }

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
        let frame = session.begin();
        session.recipe.exposure = 0.5;
        assert_eq!(session.finish(frame, Gesture::Held), FrameOutcome::Edited);
        assert!(session.save.needs_save());
        assert_eq!(session.history.steps().1, 0);
        let frame = session.begin();
        session.recipe.exposure = 1.;
        session.finish(frame, Gesture::Released);
        assert_eq!(session.history.steps().1, 1);
        assert!(session.history.undo(&mut session.recipe));
        assert_eq!(session.recipe.exposure, 0.);
    }
}
