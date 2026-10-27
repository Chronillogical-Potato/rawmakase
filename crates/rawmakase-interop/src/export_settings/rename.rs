//! Rename Photos' choices: how several photos are named, kept apart from
//! Export's as Lightroom keeps them. Saved as `rename.json` in the data
//! folder when a rename is accepted, so the next Rename Photos starts from
//! them. Only this release and later ones read it: a template is kept by its
//! tokens alone, and a field a later release adds is saved back untouched.
use super::{LaterFields, NameTemplate, Naming, TokenTemplate, known_or_default};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenameSettings {
    /// The preset that names the photos while `tokens` is `None`.
    #[serde(deserialize_with = "known_or_default")]
    pub naming: Naming,
    /// A template from the Filename Template Editor or one of RAWmakase's
    /// own, in Lightroom's form, which names them in place of `naming`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<String>,
    pub custom_text: String,
    /// Where Sequence starts.
    pub start_number: u32,
    /// Settings a later release added.
    #[serde(flatten)]
    pub later_fields: LaterFields,
}
impl Default for RenameSettings {
    fn default() -> Self {
        Self {
            naming: Naming::CustomNameSequence,
            tokens: None,
            custom_text: String::new(),
            start_number: 1,
            later_fields: LaterFields::default(),
        }
    }
}

fn path() -> PathBuf {
    crate::storage::data_dir().join("rename.json")
}

impl RenameSettings {
    /// The last rename's choices, else the defaults.
    pub fn load() -> Self {
        Self::load_from(&path()).unwrap_or_default()
    }
    pub fn save(&self) -> anyhow::Result<()> {
        self.save_to(&path())
    }
    fn load_from(path: &Path) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }
    fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        crate::storage::atomic_json(path, &serde_json::to_value(self)?)
    }
    /// The preset the user picks, which replaces a template.
    pub fn choose_naming(&mut self, naming: Naming) {
        self.naming = naming;
        self.tokens = None;
    }
    /// The template the user makes or picks, in Lightroom's form: a preset's
    /// own tokens choose that preset.
    pub fn choose_tokens(&mut self, tokens: &str) {
        match Naming::from_tokens(tokens) {
            Some(naming) => self.choose_naming(naming),
            None => self.tokens = Some(tokens.to_string()),
        }
    }
    /// The template the photos are named by. Sequence has the digits its
    /// token gives (one for a preset's), as in Lightroom.
    pub fn name_template(&self) -> NameTemplate {
        NameTemplate {
            naming: self.naming,
            text: self.custom_text.clone(),
            start: self.start_number,
            padding: 0,
            tokens: self.tokens.as_deref().map(TokenTemplate::parse),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export_settings::{BUILTIN, NameContext};

    #[test]
    fn rename_settings_round_trip_keeping_what_a_later_release_added() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("rename.json");
        // Nothing saved yet: the defaults.
        assert_eq!(RenameSettings::load_from(&file), None);
        let mut s = RenameSettings {
            custom_text: "Harbour".into(),
            start_number: 7,
            ..RenameSettings::default()
        };
        s.choose_tokens(BUILTIN[0].tokens);
        s.save_to(&file)?;
        assert_eq!(RenameSettings::load_from(&file).as_ref(), Some(&s));
        // A later release's field and preset.
        let mut saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
        saved["shoot"] = "Wedding".into();
        saved["naming"] = "ShootNameSequence".into();
        std::fs::write(&file, saved.to_string())?;
        let read = RenameSettings::load_from(&file).unwrap();
        assert_eq!(read.naming, Naming::default());
        assert_eq!(read.tokens.as_deref(), Some(BUILTIN[0].tokens));
        read.save_to(&file)?;
        let again: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
        assert_eq!(again["shoot"], "Wedding");
        Ok(())
    }

    #[test]
    fn a_presets_tokens_choose_the_preset_and_sequence_has_its_tokens_digits() {
        let mut s = RenameSettings::default();
        s.choose_tokens(BUILTIN[2].tokens);
        assert!(s.tokens.is_some());
        s.choose_tokens(Naming::FilenameSequence.tokens());
        assert_eq!(
            (s.naming, s.tokens.as_deref()),
            (Naming::FilenameSequence, None)
        );
        let at = NameContext {
            index: 11,
            total: 120,
            ..NameContext::ONE
        };
        let stem = |s: &RenameSettings| s.name_template().stem(Path::new("DSC0001.ARW"), &at).0;
        assert_eq!(stem(&s), "DSC0001-12");
        s.choose_tokens("{{image_name}}-{{naming_sequenceNumber_3Digits}}");
        assert_eq!(stem(&s), "DSC0001-012");
    }
}
