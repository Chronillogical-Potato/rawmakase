//! Bounded edit history; a pointer gesture is a single transaction.
use crate::develop::Recipe;
use std::collections::VecDeque;

const LIMIT: usize = 100;

#[derive(Default)]
pub(super) struct History {
    undo: VecDeque<Recipe>,
    redo: Vec<Recipe>,
    gesture: Option<Recipe>,
    replaying: bool,
}
impl History {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn in_gesture(&self) -> bool {
        self.gesture.is_some()
    }
    pub fn begin_frame(&mut self) {
        self.replaying = false;
    }

    pub fn record(&mut self, before: Recipe, after: &Recipe) -> bool {
        if before == *after {
            return false;
        }
        if self.undo.len() == LIMIT {
            self.undo.pop_front();
        }
        self.undo.push_back(before);
        self.redo.clear();
        true
    }
    pub fn undo(&mut self, current: &mut Recipe) -> bool {
        self.replaying = true;
        self.gesture = None;
        let Some(previous) = self.undo.pop_back() else {
            return false;
        };
        self.redo.push(std::mem::replace(current, previous));
        true
    }
    pub fn redo(&mut self, current: &mut Recipe) -> bool {
        self.replaying = true;
        self.gesture = None;
        let Some(next) = self.redo.pop() else {
            return false;
        };
        self.undo.push_back(std::mem::replace(current, next));
        true
    }
    /// Observe UI edits after drawing. Undo/redo must not create a new undo entry.
    pub fn observe(&mut self, before: Recipe, after: &Recipe, pointer_down: bool) -> bool {
        let changed = before != *after;
        if changed && !self.replaying {
            if pointer_down {
                self.gesture.get_or_insert(before);
            } else if self.gesture.is_none() {
                self.record(before, after);
            }
        }
        if !pointer_down && let Some(before) = self.gesture.take() {
            self.record(before, after);
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drag_is_one_undo_step_and_replay_does_not_record_itself() {
        let mut history = History::default();
        let mut recipe = Recipe::default();
        let original = recipe.clone();
        for value in [0.25, 0.5, 1.] {
            history.begin_frame();
            let before = recipe.clone();
            recipe.exposure = value;
            assert!(history.observe(before, &recipe, true));
        }
        history.observe(recipe.clone(), &recipe, false);
        assert_eq!(history.undo.len(), 1);
        let before = recipe.clone();
        assert!(history.undo(&mut recipe));
        assert_eq!(recipe, original);
        history.observe(before, &recipe, false);
        assert!(!history.can_undo());
        assert!(history.can_redo());
        history.redo(&mut recipe);
        assert_eq!(recipe.exposure, 1.);
    }
    #[test]
    fn new_edit_discards_redo_and_history_is_bounded() {
        let mut history = History::default();
        let mut recipe = Recipe::default();
        for step in 1..=150 {
            let before = recipe.clone();
            recipe.exposure = step as f32 / 100.;
            history.record(before, &recipe);
        }
        assert_eq!(history.undo.len(), LIMIT);
        history.undo(&mut recipe);
        let before = recipe.clone();
        recipe.exposure = -1.;
        history.record(before, &recipe);
        assert!(!history.can_redo());
        for _ in 0..LIMIT {
            assert!(history.undo(&mut recipe));
        }
        assert!(!history.undo(&mut recipe));
        assert!((recipe.exposure - 0.5).abs() < f32::EPSILON);
    }
    #[test]
    fn unchanged_gesture_and_document_reset_leave_no_history() {
        let mut history = History::default();
        let recipe = Recipe::default();
        let mut changed = recipe.clone();
        changed.exposure = 1.;
        history.observe(recipe.clone(), &changed, true);
        history.observe(changed, &recipe, false);
        assert!(!history.can_undo());
        history.observe(
            recipe.clone(),
            &Recipe {
                exposure: 1.,
                ..Default::default()
            },
            true,
        );
        history = History::default();
        assert!(!history.in_gesture());
        assert!(!history.can_undo());
        assert!(!history.can_redo());
    }
}
