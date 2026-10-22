//! The system Trash, for file operations: macOS's through `NSFileManager`,
//! Windows' Recycle Bin through `IFileOperation`, and elsewhere the
//! freedesktop.org Trash, written here. Each fails closed: a file is in the
//! Trash, with a receipt naming where, or it is where it was. Where a volume
//! has no Trash the answer is [`TrashError::Unsupported`]; nothing is ever
//! deleted, or copied and deleted, instead.
//!
//! This is the trusted boundary the catalog's `Trash` seam documents: a
//! receipt says the system moved the file. The catalog still removes rows
//! only once the file is gone from its path as well.
use crate::catalog::file_ops::{Trash, TrashError, TrashReceipt};
use std::path::Path;

#[cfg(any(all(unix, not(target_vendor = "apple")), all(test, unix)))]
mod freedesktop;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// The system's Trash.
pub(crate) struct SystemTrash;

impl Trash for SystemTrash {
    fn trash(&self, path: &Path) -> Result<TrashReceipt, TrashError> {
        system(path)
    }
}

#[cfg(target_os = "macos")]
fn system(path: &Path) -> Result<TrashReceipt, TrashError> {
    macos::trash(path)
}

#[cfg(windows)]
fn system(path: &Path) -> Result<TrashReceipt, TrashError> {
    windows::trash(path)
}

/// The freedesktop Trash of the user running the app: under
/// `$XDG_DATA_HOME` (by default `~/.local/share`), and at the top of other
/// mounts.
#[cfg(all(unix, not(target_vendor = "apple")))]
fn system(path: &Path) -> Result<TrashReceipt, TrashError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| TrashError::Failed(e.to_string()))?;
    let date = freedesktop::deletion_date(now.as_secs() as i64);
    freedesktop::trash(path, &places(), &date)
}

/// Where the Trashes of the user running the app are.
#[cfg(all(unix, not(target_vendor = "apple")))]
fn places() -> freedesktop::Places<'static> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|d| d.is_absolute())
        .or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .filter(|h| h.is_absolute())
                .map(|h| h.join(".local/share"))
        });
    freedesktop::Places {
        home: data.as_deref().and_then(freedesktop::Home::under),
        uid: rustix::process::getuid().as_raw(),
        top: &freedesktop::mount_top,
    }
}

/// Other systems have no Trash this knows.
#[cfg(not(any(unix, windows)))]
fn system(_path: &Path) -> Result<TrashReceipt, TrashError> {
    Err(TrashError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::SystemTrash;
    use crate::catalog::file_ops::{Trash, TrashError};

    /// The real Trash takes real files, the user's own on a desktop: a
    /// plain `cargo test` would leave one in it per run. These run on CI,
    /// where the Trash is the runner's and thrown away, or when asked with
    /// `RAWMAKASE_TRASH_TESTS=1`; the files are made here, in a temporary
    /// folder, and nothing else is ever trashed.
    fn real_trash() -> bool {
        std::env::var_os("CI").is_some_and(|v| !v.is_empty())
            || std::env::var_os("RAWMAKASE_TRASH_TESTS").is_some_and(|v| v == "1")
    }

    #[test]
    fn a_synthetic_file_goes_to_the_trash_with_a_receipt() {
        if !real_trash() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rawmakase-trash-test.jpg");
        let bytes = b"RAWmakase trash test, synthetic";
        std::fs::write(&path, bytes).unwrap();
        let receipt = SystemTrash.trash(&path).unwrap();
        assert!(!path.exists());
        assert!(receipt.location.exists(), "{receipt:?}");
        assert_eq!(std::fs::read(&receipt.location).unwrap(), bytes);
        // Out of the Trash again, so runs don't pile up there.
        std::fs::remove_file(&receipt.location).unwrap();
    }

    #[test]
    fn a_synthetic_folder_goes_to_the_trash_with_a_receipt() {
        if !real_trash() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("rawmakase-trash-test");
        std::fs::create_dir(&folder).unwrap();
        std::fs::write(folder.join("a.jpg"), b"a").unwrap();
        let receipt = SystemTrash.trash(&folder).unwrap();
        assert!(!folder.exists());
        assert_eq!(std::fs::read(receipt.location.join("a.jpg")).unwrap(), b"a");
        std::fs::remove_dir_all(&receipt.location).unwrap();
    }

    /// Nothing to trash: an error, and no Trash is touched.
    #[test]
    fn a_missing_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let got = SystemTrash.trash(&dir.path().join("missing.jpg"));
        assert!(matches!(got, Err(TrashError::Failed(_))), "{got:?}");
    }
}
