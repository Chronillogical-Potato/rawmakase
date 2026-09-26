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
