use super::{atomic_json, data_dir, read_json_or_default};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
#[derive(Default, Serialize, Deserialize)]
pub struct Session {
    pub last_path: Option<PathBuf>,
    pub monitor: Option<PathBuf>,
    /// Titles of panel sections the user collapsed; all others start open.
    #[serde(default)]
    pub collapsed: std::collections::BTreeSet<String>,
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
    /// How the Library showed its photos last time.
    #[serde(default)]
    pub library_layout: LibraryLayout,
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
pub fn save_session(session: &Session) -> Result<()> {
    atomic_json(&data_dir().join("session.json"), session)
}
