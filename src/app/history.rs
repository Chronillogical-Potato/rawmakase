//! Bounded edit history; a pointer gesture is a single transaction. Each step
//! is named, like Lightroom's History panel ("Exposure +0.50").
use crate::develop::Recipe;
use std::collections::VecDeque;

const LIMIT: usize = 100;

/// A History panel entry: what changed, and its new value when there is one.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Step {
    pub(super) name: String,
    pub(super) value: String,
}
impl Step {
    pub(super) fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

#[derive(Default)]
pub(super) struct History {
    /// States before each step, oldest first, with the step that left them.
    undo: VecDeque<(Recipe, Step)>,
    /// States after each undone step, the next one last.
    redo: Vec<(Recipe, Step)>,
    gesture: Option<Recipe>,
    replaying: bool,
    /// The name for the next recorded step; otherwise it is derived.
    label: Option<Step>,
}
impl History {
    /// Names the step being made, e.g. by the slider being dragged.
    pub fn label(&mut self, step: Step) {
        self.label = Some(step);
    }
    /// Every step, oldest first, and how many of them are applied.
    pub fn steps(&self) -> (Vec<&Step>, usize) {
        let steps = self
            .undo
            .iter()
            .map(|(_, s)| s)
            .chain(self.redo.iter().rev().map(|(_, s)| s))
            .collect();
        (steps, self.undo.len())
    }
    /// Undoes or redoes until `applied` steps are applied, as clicking a
    /// History step in Lightroom does. Later steps stay until a new edit.
    pub fn go_to(&mut self, applied: usize, current: &mut Recipe) -> bool {
        let mut moved = false;
        while self.undo.len() > applied && self.undo(current) {
            moved = true;
        }
        while self.undo.len() < applied && self.redo(current) {
            moved = true;
        }
        moved
    }
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
        let label = self.label.take();
        if before == *after {
            return false;
        }
        let step = label.unwrap_or_else(|| describe(&before, after));
        if self.undo.len() == LIMIT {
            self.undo.pop_front();
        }
        self.undo.push_back((before, step));
        self.redo.clear();
        true
    }
    pub fn undo(&mut self, current: &mut Recipe) -> bool {
        self.replaying = true;
        self.gesture = None;
        let Some((previous, step)) = self.undo.pop_back() else {
            return false;
        };
        self.redo.push((std::mem::replace(current, previous), step));
        true
    }
    pub fn redo(&mut self, current: &mut Recipe) -> bool {
        self.replaying = true;
        self.gesture = None;
        let Some((next, step)) = self.redo.pop() else {
            return false;
        };
        self.undo
            .push_back((std::mem::replace(current, next), step));
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

/// A name for an edit no control named: the panel it belongs to.
fn describe(before: &Recipe, after: &Recipe) -> Step {
    let (b, a) = (before, after);
    if a.preset_name != b.preset_name && !a.preset_name.is_empty() {
        return Step::new("Preset", crate::presets::display_name(&a.preset_name));
    }
    let profile = |r: &Recipe| r.profile.as_ref().map(|p| p.name.clone());
    if profile(a) != profile(b) {
        return Step::new("Profile", profile(a).unwrap_or_default());
    }
    if a.effects.monochrome != b.effects.monochrome {
        let treatment = if a.effects.monochrome {
            "Black & White"
        } else {
            "Color"
        };
        return Step::new("Treatment", treatment);
    }
    let name = if (a.temperature, a.tint, a.wb) != (b.temperature, b.tint, b.wb) {
        "White Balance"
    } else if (a.crop, a.straighten, a.rotation, a.flip_x, a.flip_y)
        != (b.crop, b.straighten, b.rotation, b.flip_x, b.flip_y)
    {
        "Crop"
    } else if a.curve != b.curve
        || a.effects.channels != b.effects.channels
        || a.effects.parametric != b.effects.parametric
    {
        "Tone Curve"
    } else if a.hsl != b.hsl || a.effects.gray_mix != b.effects.gray_mix {
        "HSL / Color"
    } else if a.grading != b.grading || a.effects.global_grade != b.effects.global_grade {
        "Color Grading"
    } else if (a.lens_builtin, a.lens_profile) != (b.lens_builtin, b.lens_profile) {
        "Lens Corrections"
    } else if a.transform != b.transform {
        "Transform"
    } else {
        "Edit"
    };
    Step::new(name, "")
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
    #[test]
    fn steps_are_named_and_go_to_moves_between_them() {
        let mut history = History::default();
        let mut recipe = Recipe::default();
        let original = recipe.clone();
        history.label(Step::new("Exposure", "+0.50"));
        let before = recipe.clone();
        recipe.exposure = 0.5;
        history.record(before, &recipe);
        let before = recipe.clone();
        recipe.temperature += 100.;
        history.record(before, &recipe);
        let (steps, applied) = history.steps();
        let names: Vec<_> = steps.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Exposure", "White Balance"]);
        assert_eq!(applied, 2);
        assert!(history.go_to(0, &mut recipe));
        assert_eq!(recipe, original);
        assert_eq!(history.steps().1, 0);
        assert!(history.go_to(1, &mut recipe));
        assert_eq!(recipe.exposure, 0.5);
        assert_eq!(recipe.temperature, original.temperature);
        assert_eq!(history.steps().0.len(), 2);
    }
}
