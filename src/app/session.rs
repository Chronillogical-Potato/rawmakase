//! The desktop session: the last photo and catalog place, the panels and
//! layouts as the user left them, and the preferences kept between launches.
use crate::storage::{data_dir, read_json_or_default};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Default, Serialize, Deserialize)]
pub struct Session {
    pub last_path: Option<PathBuf>,
    pub monitor: Option<PathBuf>,
    /// Titles of panel sections the user collapsed; all others start open.
    #[serde(default)]
    pub collapsed: std::collections::BTreeSet<String>,
    /// The sides of the window in Solo Mode, where opening a panel closes the
    /// others: "develop-left", "develop-right", "library-left", "library-right".
    #[serde(default)]
    pub solo: std::collections::BTreeSet<String>,
    /// The first-run setup was completed; until then it opens on launch.
    #[serde(default)]
    pub onboarding_done: bool,
    /// Where the user was in the last catalog: the Library folder, the
    /// selected photo, and whether Develop was open.
    #[serde(default)]
    pub library_source: String,
    #[serde(default)]
    pub selected_photo: Option<i64>,
    #[serde(default)]
    pub develop: bool,
    /// Which demosaic full-size decodes use.
    #[serde(default)]
    pub demosaic: crate::raw::Demosaic,
    /// The user turned off checking for updates, which is on by default.
    #[serde(default)]
    pub no_update_checks: bool,
    /// A release the user chose to skip; newer ones are still offered.
    #[serde(default)]
    pub skipped_version: Option<String>,
    /// The interface's palette file; None is RAWmakase's own greys.
    #[serde(default)]
    pub theme: Option<String>,
    /// The user picked `theme`. Until then Omarchy's theme is followed on
    /// an Omarchy desktop, and RAWmakase's greys elsewhere.
    #[serde(default)]
    pub theme_chosen: bool,
    /// Photo > Auto Advance: a rating, flag or label moves to the next photo.
    #[serde(default)]
    pub auto_advance: bool,
    /// The user turned off applying the Auto mix when first converting to black &
    /// white, which is on by default.
    #[serde(default)]
    pub no_auto_black_white_mix: bool,
    /// How the Library showed its photos last time.
    #[serde(default)]
    pub library_layout: LibraryLayout,
    /// The groups Copy Settings copied last time; None until it has been used.
    #[serde(default)]
    pub copy_groups: Option<crate::develop::settings_groups::GroupSelection>,
    /// The Crop tool's guide overlay.
    #[serde(default)]
    pub crop_guides: CropGuideLayout,
    /// Preferences > Raw Defaults: what photos without an edit start from.
    #[serde(
        default,
        deserialize_with = "crate::develop::defaults::lenient_settings"
    )]
    pub raw_defaults: crate::develop::defaults::RawDefaults,
}

/// The Crop tool's guide overlay, which way round it is and when it shows, by stable
/// names as [`LibraryLayout`]; a name this version does not know reads as the default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CropGuideLayout {
    /// "grid", "thirds", "diagonal", "triangle", "golden-ratio" or "golden-spiral".
    pub guide: String,
    pub orientation: u32,
    /// "always", "auto" or "never".
    pub show: String,
}

/// How the Library shows its photos: its view, filter bar, sort and grid,
/// kept across launches. Names are stable keys, so a layout from another
/// version reads as far as it is understood and the rest is the default.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryLayout {
    /// "grid", "loupe", "compare" or "survey".
    pub view: String,
    pub sort: String,
    pub reverse: bool,
    pub cell_style: String,
    pub thumb_size: Option<f32>,
    pub query: String,
    pub flags: Vec<i32>,
    pub rating: Option<i32>,
    pub rating_op: String,
    /// Colour names, "none" for no label and "other" for any other.
    pub labels: Vec<String>,
    pub kind: String,
    /// Cmd+L turned the filter bar off.
    pub filters_off: bool,
}
pub fn load_session() -> Session {
    read_json_or_default(&data_dir().join("session.json"))
}

#[cfg(test)]
mod raw_defaults_tests {
    use super::Session;
    use crate::develop::defaults::{DefaultChoice, RawDefaults};

    #[test]
    fn raw_defaults_survive_the_session_and_unreadable_ones_reset() {
        let mut settings = RawDefaults {
            camera_overrides: true,
            ..Default::default()
        };
        settings.set_camera("Canon EOS R5", DefaultChoice::Rawmakase);
        // A session from before raw defaults, or with ones that don't read, keeps
        // everything else.
        let old: Session =
            serde_json::from_str(r#"{"last_path":null,"monitor":null,"auto_advance":true}"#)
                .unwrap();
        assert_eq!(old.raw_defaults, RawDefaults::default());
        let odd: Session = serde_json::from_str(
            r#"{"last_path":null,"monitor":null,"auto_advance":true,"raw_defaults":7}"#,
        )
        .unwrap();
        assert!(odd.auto_advance);
        assert_eq!(odd.raw_defaults, RawDefaults::default());
        let session = Session {
            raw_defaults: settings.clone(),
            ..Default::default()
        };
        let json = serde_json::to_string(&session).unwrap();
        let back: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(back.raw_defaults, settings);
    }
}
