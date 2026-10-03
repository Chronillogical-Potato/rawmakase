//! Copy Settings, Paste Settings and Paste from Previous: settings moving from one
//! photo to another by group (see `develop::settings_groups`).
use super::Editor;
use crate::develop::{
    Recipe,
    settings_groups::{self, GroupSelection, Target},
};

/// Settings copied from a photo, and the groups Paste applies.
#[derive(Clone, Debug)]
pub(super) struct Clipboard {
    pub(super) recipe: Recipe,
    pub(super) groups: GroupSelection,
}

impl Editor {
    pub(super) fn copy_settings(&mut self) {
        self.clipboard = Some(Clipboard {
            recipe: self.document.recipe.clone(),
            groups: GroupSelection::default(),
        });
        self.status = "Settings copied".into();
    }
    /// Pastes the copied settings. Spot removal and masks belong to their photo and
    /// stay as they were, as with Lightroom's default Paste Settings.
    pub(super) fn paste_settings(&mut self) {
        if let Some(Clipboard { recipe, groups }) = self.clipboard.clone() {
            self.apply_settings(&recipe, &groups, "Paste Settings");
        }
    }
    /// Lightroom's Paste Settings from Previous: the settings of the photo open before
    /// this one, with the groups Paste uses by default.
    pub(super) fn paste_previous(&mut self) {
        if let Some(previous) = self.previous_settings.clone() {
            self.apply_settings(&previous, &GroupSelection::default(), "Paste from Previous");
        }
    }
    /// `from`'s settings in `groups` over the open photo's, as one History step.
    fn apply_settings(&mut self, from: &Recipe, groups: &GroupSelection, step: &str) {
        // The header's metadata, or the decoded image's while the header is pending.
        let Some(metadata) = self
            .document
            .metadata
            .clone()
            .or_else(|| self.document.full().map(|im| im.metadata.clone()))
        else {
            return;
        };
        let out = settings_groups::transfer(
            from,
            &self.document.recipe,
            groups,
            Target {
                metadata: &metadata,
                profiles: &self.document.profiles,
            },
        );
        self.document
            .history
            .label(super::history::Step::new(step, ""));
        self.document.recipe = out.recipe;
        self.ensure_upright();
        self.status = if out.notes.is_empty() {
            "Settings pasted".into()
        } else {
            format!("Settings pasted · {}", out.notes.join(" · "))
        };
    }
}
