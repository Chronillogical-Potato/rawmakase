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
        let uri = file_uri(path);
        // Waits for the reply, so a session without a file manager on the bus
        // fails here and falls back.
        let selected = Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
            ])
            .arg(format!("array:string:{uri}"))
            .arg("string:")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !selected {
            let folder = path.parent().context("File has no containing folder")?;
            Command::new("xdg-open").arg(folder).spawn()?;
        }
    }
    Ok(())
}

/// `path` as a file URI with every byte but unreserved ones and `/`
/// percent-encoded, so a comma (dbus-send's array separator) or a space can't
/// split or end it.
fn file_uri(path: &Path) -> String {
    use std::fmt::Write;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_encoded_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            uri.push(b as char);
        } else {
            let _ = write!(uri, "%{b:02X}");
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_uris_escape_what_dbus_send_would_split() {
        assert_eq!(
            super::file_uri(std::path::Path::new("/photos/a,b c.jpg")),
            "file:///photos/a%2Cb%20c.jpg"
        );
    }
}
