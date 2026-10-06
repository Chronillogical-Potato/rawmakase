//! Where RAWmakase keeps its data, so a client finds the app's `control.json`
//! without the app's own code.
use std::path::PathBuf;

/// RAWMAKASE_DATA_DIR, else the platform's application-data folder.
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
pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}
/// $XDG_DATA_HOME, or its default ~/.local/share.
pub fn xdg_data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/share"))
}
