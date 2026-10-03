//! Copy Settings, Paste Settings and Paste from Previous: settings moving from one
//! photo to another by group (see `develop::settings_groups`).
use super::Editor;
use crate::develop::{
    Recipe,
    settings_groups::{self, GroupSelection, Source, Target},
};
use crate::raw::Metadata;

/// A photo's settings with its camera, as Copy or leaving a photo keeps them.
#[derive(Clone, Debug)]
pub(super) struct Settings {
    pub(super) recipe: Recipe,
    pub(super) metadata: Metadata,
}

/// Settings copied from a photo, and the groups Paste applies.
#[derive(Clone, Debug)]
pub(super) struct Clipboard {
    pub(super) settings: Settings,
    pub(super) groups: GroupSelection,
}

impl Editor {
    /// The open photo's settings, once its camera is known.
    pub(super) fn current_settings(&self) -> Option<Settings> {
        Some(Settings {
            recipe: self.document.recipe.clone(),
            metadata: self.document_metadata()?,
        })
    }
    /// The header's metadata, or the decoded image's while the header is pending.
    fn document_metadata(&self) -> Option<Metadata> {
        self.document
            .metadata
            .clone()
            .or_else(|| self.document.full().map(|im| im.metadata.clone()))
    }
    pub(super) fn copy_settings(&mut self) {
        let Some(settings) = self.current_settings() else {
            return;
        };
        self.clipboard = Some(Clipboard {
            settings,
            groups: GroupSelection::default(),
        });
        self.status = "Settings copied".into();
    }
    /// Pastes the copied settings. Spot removal and masks belong to their photo and
    /// stay as they were, as with Lightroom's default Paste Settings.
    pub(super) fn paste_settings(&mut self) {
        if let Some(Clipboard { settings, groups }) = self.clipboard.clone() {
            self.apply_settings(&settings, &groups, "Paste Settings");
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
    fn apply_settings(&mut self, from: &Settings, groups: &GroupSelection, step: &str) {
        let Some(metadata) = self.document_metadata() else {
            return;
        };
        let out = settings_groups::transfer(
            Source {
                recipe: &from.recipe,
                metadata: &from.metadata,
            },
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
