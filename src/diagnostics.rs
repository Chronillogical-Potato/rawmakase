//! What a bug report needs to know about this copy of RAWmakase and the
//! system it runs on. Unlike the usage report (`stats`), this is not behind
//! the `telemetry` feature: nothing here is sent anywhere.
use std::path::Path;

/// How this copy was installed: an AppImage, else as fastframe-update finds
/// it to decide whether it may update itself: the release download it came
/// from, or the package manager that owns it. Inspecting the install can run
/// the package manager, so this runs off the UI thread.
pub fn installation_channel() -> &'static str {
    use fastframe_update::{Kind, PackageManager, Unsupported};
    if runs_from_appimage() {
        return "linux-appimage";
    }
    match crate::updates::installation() {
        Ok(installation) => match installation.kind {
            Kind::AppImage => "linux-appimage",
            Kind::MacBundle => "macos-dmg",
            Kind::WindowsInstaller => "windows-installer",
            Kind::Portable if cfg!(windows) => "windows-zip",
            Kind::Portable => "linux-tarball",
        },
        Err(reason) => match reason {
            // Run from the disk image or a quarantine copy: still the DMG.
            Unsupported::MoveToApplications => "macos-dmg",
            Unsupported::Homebrew => "homebrew",
            Unsupported::SystemPackage(PackageManager::Apt) => "deb",
            Unsupported::SystemPackage(PackageManager::Dnf) => "rpm",
            // pacman can't tell the release package from an AUR build.
            Unsupported::SystemPackage(PackageManager::Pacman) => "arch-package",
            Unsupported::Flatpak => "flatpak",
            Unsupported::Snap => "snap",
            Unsupported::Nix => "nix",
            Unsupported::Cargo => "cargo",
            _ => "unknown",
        },
    }
}

/// The AppImage runtime sets `APPDIR` to where it mounted or extracted the
/// image. Programs started from the app inherit it, so only an executable
/// inside that directory is the AppImage itself.
fn runs_from_appimage() -> bool {
    let Some(app_dir) = std::env::var_os("APPDIR") else {
        return false;
    };
    std::env::current_exe().is_ok_and(|executable| is_inside(&executable, Path::new(&app_dir)))
}

/// Whether `executable`, already resolved, is inside `directory`.
fn is_inside(executable: &Path, directory: &Path) -> bool {
    std::fs::canonicalize(directory).is_ok_and(|directory| executable.starts_with(directory))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_executable_inside_the_app_dir_is_an_appimage() {
        let dir = tempfile::tempdir().unwrap();
        let app_dir = dir.path().join("app dir");
        std::fs::create_dir(&app_dir).unwrap();
        let app_dir = std::fs::canonicalize(app_dir).unwrap();
        let inside = app_dir.join("usr/lib/rawmakase/rawmakase");
        assert!(is_inside(&inside, &app_dir));
        // A tarball or package copy started from an AppImage inherits APPDIR.
        let outside = app_dir.with_file_name("app dir2").join("rawmakase");
        assert!(!is_inside(&outside, &app_dir));
        assert!(!is_inside(&inside, &dir.path().join("gone")));
    }
}
