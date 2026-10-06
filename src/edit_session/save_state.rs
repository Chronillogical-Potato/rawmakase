//! Autosave policy. Protected edits can be exported, but never become write jobs.
use std::time::{Duration, Instant};

/// How long edits must pause before autosave writes them.
const SETTLE: Duration = Duration::from_millis(600);

#[derive(Default)]
pub enum SaveState {
    #[default]
    Clean,
    Pending(Instant),
    /// Being written in the background; `changed` is when the edit changed
    /// again since, and so still needs saving afterwards.
    Saving {
        changed: Option<Instant>,
    },
    Failed {
        retry_after: Instant,
        error: String,
    },
    Protected(String),
}
impl SaveState {
    pub fn is_protected(&self) -> bool {
        matches!(self, Self::Protected(_))
    }
    pub fn needs_save(&self) -> bool {
        matches!(
            self,
            Self::Pending(_) | Self::Saving { .. } | Self::Failed { .. }
        )
    }
    pub fn mark_changed(&mut self) {
        match self {
            Self::Protected(_) => {}
            Self::Saving { changed } => *changed = Some(Instant::now()),
            _ => *self = Self::Pending(Instant::now()),
        }
    }
    /// How long until autosave is due, while it waits for edits to pause or for a
    /// failed save's retry; the interface wakes then rather than polling.
    pub fn due_in(&self) -> Option<Duration> {
        match self {
            Self::Pending(at) => Some(SETTLE.saturating_sub(at.elapsed())),
            Self::Failed { retry_after, .. } => {
                Some(retry_after.saturating_duration_since(Instant::now()))
            }
            _ => None,
        }
    }
    pub fn ready(&self) -> bool {
        match self {
            Self::Pending(at) => at.elapsed() > SETTLE,
            Self::Failed { retry_after, .. } => Instant::now() >= *retry_after,
            _ => false,
        }
    }
    pub fn saved(&mut self) {
        *self = Self::Clean;
    }
    pub fn saving(&mut self) {
        *self = Self::Saving { changed: None };
    }
    /// A background save finished. Returns false when it no longer applies:
    /// the document was reloaded or its edits discarded meanwhile.
    pub fn finished(&mut self, result: Result<(), String>) -> bool {
        let Self::Saving { changed } = *self else {
            return false;
        };
        match (result, changed) {
            (Err(error), _) => self.failed(error),
            (Ok(()), Some(at)) => *self = Self::Pending(at),
            (Ok(()), None) => self.saved(),
        }
        true
    }
    pub fn protect(&mut self, reason: String) {
        *self = Self::Protected(reason);
    }
    pub fn failed(&mut self, error: String) {
        *self = Self::Failed {
            retry_after: Instant::now() + Duration::from_millis(600),
            error,
        };
    }
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Failed { error, .. } | Self::Protected(error) => Some(error),
            _ => None,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn autosave_is_due_once_edits_settle_and_never_when_clean() {
        let mut state = SaveState::default();
        assert_eq!(state.due_in(), None);
        state.mark_changed();
        let due = state.due_in().unwrap();
        assert!(due <= SETTLE && due > SETTLE / 2, "{due:?}");
        state.saving();
        assert_eq!(state.due_in(), None);
    }
    #[test]
    fn protected_edits_never_autosave_and_failures_remain_pending() {
        let mut state = SaveState::default();
        state.mark_changed();
        assert!(state.needs_save());
        state.failed("Read-only device".into());
        assert!(state.needs_save());
        assert_eq!(state.message(), Some("Read-only device"));
        state.protect("Conflicting source identity".into());
        state.mark_changed();
        assert!(!state.needs_save());
        assert!(!state.ready());
        assert_eq!(state.message(), Some("Conflicting source identity"));
        state.saved();
        assert!(!state.is_protected());
        assert!(!state.needs_save());
    }
    #[test]
    fn an_edit_during_a_background_save_still_needs_saving() {
        let mut state = SaveState::default();
        state.mark_changed();
        state.saving();
        assert!(state.needs_save());
        assert!(!state.ready());
        assert!(state.finished(Ok(())));
        assert!(!state.needs_save());

        state.mark_changed();
        state.saving();
        state.mark_changed();
        assert!(state.finished(Ok(())));
        assert!(matches!(state, SaveState::Pending(_)));

        state.saving();
        assert!(state.finished(Err("Disk full".into())));
        assert_eq!(state.message(), Some("Disk full"));

        state.saving();
        state.saved();
        assert!(!state.finished(Ok(())));
    }
}
