//! Data-directory policy shared by the app and its standalone control client.
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    std::env::var_os("RAWMAKASE_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            // Windows has no HOME; without this the data would land in the
            // current directory.
            cfg!(windows)
                .then(|| std::env::var_os("APPDATA"))
                .flatten()
                .map(|p| PathBuf::from(p).join("RAWmakase"))
        })
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                home().join("Library/Application Support/RAWmakase")
            } else if cfg!(windows) {
                // XDG_DATA_HOME is not consulted on Windows.
                home().join(".local/share/rawmakase")
            } else {
                xdg_data_home().join("rawmakase")
            }
        })
}
pub(super) fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}
/// $XDG_DATA_HOME, or its default ~/.local/share.
pub(super) fn xdg_data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"))
}
