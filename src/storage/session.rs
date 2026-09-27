use super::{atomic_json, data_dir};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs::File, path::PathBuf};
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
}
pub fn load_session() -> Session {
    File::open(data_dir().join("session.json"))
        .ok()
        .and_then(|f| serde_json::from_reader(f).ok())
        .unwrap_or_default()
}
pub fn save_session(session: &Session) -> Result<()> {
    atomic_json(&data_dir().join("session.json"), session)
}
