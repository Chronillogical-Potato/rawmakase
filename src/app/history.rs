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
    /// Identifies the state this step leaves, set when it is recorded.
    state: u64,
}
impl Step {
    pub(super) fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            state: 0,
        }
    }
}

/// A state in History: how many steps are applied, counted from the first
/// step ever recorded so it holds when old steps are dropped, and which
/// state that is, since after undoing and editing the same count can name
/// another branch.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Mark {
    applied: usize,
    state: u64,
}

/// A change to the recipe for the shared undo log: a recorded step or a
/// click in the History panel, with the History positions around it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Recorded {
    /// Orders it among Library commands made in the same frame.
    pub(super) sequence: u64,
    pub(super) before: Recipe,
    pub(super) after: Recipe,
    pub(super) at_before: Mark,
    pub(super) at_after: Mark,
}

pub(super) struct History {
    /// Tells this photo's History apart from earlier ones, e.g. after the
    /// photo was opened again.
    id: u64,
    /// Steps dropped from the front to stay within the limit, and the state
    /// the oldest remaining one starts from.
    dropped: usize,
    origin: u64,
    /// Changes not yet handed to the shared undo log.
    recorded: Vec<Recorded>,
    /// States before each step, oldest first, with the step that left them.
    undo: VecDeque<(Recipe, Step)>,
    /// States after each undone step, the next one last.
    redo: Vec<(Recipe, Step)>,
    gesture: Option<Recipe>,
    replaying: bool,
    /// The name for the next recorded step; otherwise it is derived.
    label: Option<Step>,
}
impl Default for History {
    fn default() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            id: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            dropped: 0,
            origin: 0,
            recorded: Vec::new(),
            undo: VecDeque::new(),
            redo: Vec::new(),
            gesture: None,
            replaying: false,
            label: None,
        }
    }
}
impl History {
    pub fn id(&self) -> u64 {
        self.id
    }
    /// The state applied now.
    pub fn mark(&self) -> Mark {
        Mark {
            applied: self.dropped + self.undo.len(),
            state: self.undo.back().map_or(self.origin, |(_, s)| s.state),
        }
    }
    /// The changes made since the last call, for the shared undo log.
    pub fn take_recorded(&mut self) -> Vec<Recorded> {
        std::mem::take(&mut self.recorded)
    }
    /// A click on a History step: `go_to`, recorded as one change.
    pub fn jump(&mut self, applied: usize, current: &mut Recipe) -> bool {
        let (before, at_before) = (current.clone(), self.mark());
        if !self.go_to(applied, current) {
            return false;
        }
        self.recorded.push(Recorded {
            sequence: super::undo::sequence(),
            before,
            after: current.clone(),
            at_before,
            at_after: self.mark(),
        });
        true
    }
    /// Undo or redo from the shared log: back to state `at` when it is still
    /// here; otherwise `target` is set as a new step named `step`.
    pub fn restore(&mut self, at: Mark, target: &Recipe, current: &mut Recipe, step: Step) {
        // Not an edit of the user's for `observe` to record.
        self.replaying = true;
        // History's own states are kept up to date (e.g. by Upright's
        // analysis), so they win over the copy in the log.
        // Moves only when that state is still in History, so a miss leaves
        // History as it was and records the fallback from the state shown.
        if self.state_at(at.applied) == Some(at.state) {
            self.go_to(at.applied - self.dropped, current);
        } else {
            self.set(target, current, step);
        }
    }
    /// The state with `applied` steps applied, among the steps History has,
    /// done and undone.
    fn state_at(&self, applied: usize) -> Option<u64> {
        let index = applied.checked_sub(self.dropped)?;
        if index == 0 {
            return Some(self.origin);
        }
        self.undo
            .iter()
            .map(|(_, s)| s)
            .chain(self.redo.iter().rev().map(|(_, s)| s))
            .nth(index - 1)
            .map(|s| s.state)
    }
    /// Sets `target` as a new step named `step`, without handing it to the
    /// shared undo log.
    pub fn set(&mut self, target: &Recipe, current: &mut Recipe, step: Step) {
        self.replaying = true;
        let before = std::mem::replace(current, target.clone());
        self.label(step);
        self.push(before, current);
    }
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
    #[cfg(test)]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    #[cfg(test)]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn in_gesture(&self) -> bool {
        self.gesture.is_some()
    }
    /// Records a pointer gesture still in progress, up to `current`, so a step made
    /// outside the UI (an asynchronous result) lands after it; the rest of the drag
    /// becomes a step of its own.
    pub fn finish_gesture(&mut self, current: &Recipe) {
        if let Some(before) = self.gesture.take() {
            self.record(before, current);
        }
    }
    pub fn begin_frame(&mut self) {
        self.replaying = false;
    }

    pub fn record(&mut self, before: Recipe, after: &Recipe) -> bool {
        let at_before = self.mark();
        let recorded = before.clone();
        if !self.push(before, after) {
            return false;
        }
        self.recorded.push(Recorded {
            sequence: super::undo::sequence(),
            before: recorded,
            after: after.clone(),
            at_before,
            at_after: self.mark(),
        });
        true
    }
    /// Adds a step without handing it to the shared undo log.
    fn push(&mut self, before: Recipe, after: &Recipe) -> bool {
        let label = self.label.take();
        if before == *after {
            return false;
        }
        static STATES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let mut step = label.unwrap_or_else(|| describe(&before, after));
        step.state = STATES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if self.undo.len() == LIMIT
            && let Some((_, oldest)) = self.undo.pop_front()
        {
            self.dropped += 1;
            self.origin = oldest.state;
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
    /// Every recorded state, to update what is derived from the photo rather than
    /// edited, such as Upright's analysis.
    pub fn states_mut(&mut self) -> impl Iterator<Item = &mut Recipe> {
        self.undo
            .iter_mut()
            .chain(self.redo.iter_mut())
            .map(|(r, _)| r)
            .chain(self.gesture.as_mut())
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
    } else if (a.lens_builtin, a.lens_profile, a.lens_ca)
        != (b.lens_builtin, b.lens_profile, b.lens_ca)
    {
        "Lens Corrections"
    } else if a.transform != b.transform || a.upright != b.upright {
        "Transform"
    } else if a.retouch != b.retouch {
        return Step::new(retouch_step(b, a), "");
    } else if a.masks != b.masks {
        return mask_step(b, a);
    } else {
        "Edit"
    };
    Step::new(name, "")
}

/// Lightroom names spot edits by mode: "Spot Removal", "Clone", "Delete Spot".
fn retouch_step(before: &Recipe, after: &Recipe) -> &'static str {
    use crate::develop::retouch::RetouchMode;
    if after.retouch.len() < before.retouch.len() {
        return "Delete Spot";
    }
    let changed = after
        .retouch
        .iter()
        .zip(
            before
                .retouch
                .iter()
                .map(Some)
                .chain(std::iter::repeat(None)),
        )
        .find(|(a, b)| Some(*a) != *b)
        .map(|(a, _)| a.mode);
    match changed {
        Some(RetouchMode::Clone) => "Clone",
        _ => "Spot Removal",
    }
}
/// "Brush Mask" for a new mask, "Mask 2: Exposure" for a changed slider, otherwise
/// what happened to which mask.
fn mask_step(before: &Recipe, after: &Recipe) -> Step {
    let (b, a) = (&before.masks, &after.masks);
    if a.len() > b.len() {
        let kind = a
            .last()
            .and_then(|m| m.components.first())
            .map_or("New", |c| c.shape.kind());
        return Step::new(format!("{kind} Mask"), "");
    }
    if a.len() < b.len() {
        return Step::new("Delete Mask", "");
    }
    let Some(i) = (0..a.len()).find(|i| a[*i] != b[*i]) else {
        return Step::new("Masking", "");
    };
    let (m, old) = (&a[i], &b[i]);
    let name = if m.name.is_empty() {
        format!("Mask {}", i + 1)
    } else {
        m.name.clone()
    };
    if m.components.len() != old.components.len() {
        let what = if m.components.len() > old.components.len() {
            m.components.last().map_or("Add", |c| c.shape.kind())
        } else {
            "Remove Component"
        };
        return Step::new(format!("{name}: {what}"), "");
    }
    if m.components != old.components {
        let kind = m
            .components
            .iter()
            .zip(&old.components)
            .find(|(x, y)| x != y)
            .map_or("Mask", |(c, _)| c.shape.kind());
        return Step::new(format!("{name}: {kind}"), "");
    }
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
    fn retouch_and_mask_edits_have_lightroom_names() {
        use crate::develop::{masks, retouch};
        let before = Recipe::default();
        let mut after = before.clone();
        after.retouch.push(retouch::RetouchOp {
            mode: retouch::RetouchMode::Heal,
            shape: retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.01,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.02, 0.],
        });
        assert_eq!(describe(&before, &after).name, "Spot Removal");
        assert_eq!(describe(&after, &before).name, "Delete Spot");
        let mut masked = before.clone();
        masked.masks.push(masks::MaskGroup {
            components: vec![masks::MaskComponent::new(masks::MaskShape::Brush {
                strokes: Vec::new(),
            })],
            ..Default::default()
        });
        masked.masks.push(masked.masks[0].clone());
        let mut exposed = masked.clone();
        exposed.masks[1].adjust.exposure = 1.;
        assert_eq!(describe(&before, &masked).name, "Brush Mask");
        assert_eq!(describe(&masked, &exposed).name, "Mask 2");
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
