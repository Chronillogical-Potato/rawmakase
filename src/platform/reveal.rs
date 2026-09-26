//! Showing a file in the system file manager.
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// The platform's own name for the action, e.g. "Reveal in Finder".
pub const LABEL: &str = if cfg!(target_os = "macos") {
    "Reveal in Finder"
} else if cfg!(windows) {
    "Show in Explorer"
} else {
    "Show in File Manager"
};

/// Opens the file manager with `path` selected. On Linux this asks the
/// desktop's FileManager1 service to select the file and falls back to
/// opening the containing folder.
pub fn reveal(path: &Path) -> Result<()> {
    if cfg!(target_os = "macos") {
        Command::new("open").arg("-R").arg(path).spawn()?;
    } else if cfg!(windows) {
        let mut select = std::ffi::OsString::from("/select,");
        select.push(path);
        Command::new("explorer").arg(select).spawn()?;
    } else {
        let uri = format!("file://{}", path.display());
        let selected = Command::new("dbus-send")
            .args([
                "--session",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
            ])
            .arg(format!("array:string:{uri}"))
            .arg("string:")
            .status()
            .is_ok_and(|s| s.success());
        if !selected {
            let folder = path.parent().context("File has no containing folder")?;
            Command::new("xdg-open").arg(folder).spawn()?;
        }
    }
    Ok(())
}
