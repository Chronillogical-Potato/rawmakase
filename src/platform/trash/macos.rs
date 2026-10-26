//! macOS: `NSFileManager`'s `trashItemAtURL:resultingItemURL:error:`, which
//! moves a file to the Trash of its volume and says where it went. That
//! resulting item, as a path, is the receipt: success without one is a
//! failure. A volume without a Trash answers `NSFeatureUnsupportedError`
//! ([`TrashError::Unsupported`]); any other error is
//! [`TrashError::Failed`]. Neither deletes the file.
//!
//! Whether a file's volume has a Trash is asked of the same manager's
//! `URLForDirectory:inDomain:appropriateForURL:create:error:`, which names
//! the user's Trash for it without making one: a sure yes or don't know.
use crate::catalog::file_ops::{TrashError, TrashReceipt, TrashSupport};
use objc2::rc::autoreleasepool;
use objc2_foundation::{
    NSError, NSFeatureUnsupportedError, NSFileManager, NSSearchPathDirectory,
    NSSearchPathDomainMask, NSURL,
};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

pub(super) fn trash(path: &Path) -> Result<TrashReceipt, TrashError> {
    // Foundation autoreleases what it returns, and a worker thread has no
    // pool of its own to drain.
    autoreleasepool(|_| {
        let url = NSURL::from_file_path(path).ok_or_else(|| {
            TrashError::Failed(format!("{} is not a path macOS can name", path.display()))
        })?;
        let mut resulting = None;
        NSFileManager::defaultManager()
            .trashItemAtURL_resultingItemURL_error(&url, Some(&mut resulting))
            .map_err(|e| refused(&e))?;
        let location = resulting
            .and_then(|url| url.to_file_path())
            .ok_or_else(|| TrashError::Failed("the Trash didn't say where the file went".into()))?;
        Ok(TrashReceipt { location })
    })
}

fn refused(e: &NSError) -> TrashError {
    let cocoa = e.domain().to_string() == "NSCocoaErrorDomain";
    if cocoa && e.code() == NSFeatureUnsupportedError {
        TrashError::Unsupported
    } else {
        TrashError::Failed(e.localizedDescription().to_string())
    }
}

/// Whether `path` can go to the Trash: `Yes` when the Trash macOS names
/// for it is a folder on `path`'s own volume, else `Unknown`, never `No`.
/// Nothing is made (`create:NO`), so a volume whose `.Trashes/<uid>` isn't
/// made yet answers `NSFeatureUnsupportedError` as a read-only one does
/// (measured: APFS, HFS+, FAT and exFAT disk images before their first
/// Trash), though the Trash then takes the file.
pub(super) fn supports(path: &Path) -> TrashSupport {
    let Ok(device) = std::fs::symlink_metadata(path).map(|m| m.dev()) else {
        return TrashSupport::Unknown;
    };
    autoreleasepool(|_| {
        let Some(url) = NSURL::from_file_path(path) else {
            return TrashSupport::Unknown;
        };
        let named = NSFileManager::defaultManager()
            .URLForDirectory_inDomain_appropriateForURL_create_error(
                NSSearchPathDirectory::TrashDirectory,
                NSSearchPathDomainMask::UserDomainMask,
                Some(&url),
                false,
            );
        answer(named.ok().and_then(|dir| dir.to_file_path()), device)
    })
}

/// What the Trash folder named for a file on `device` says: one on that
/// device takes it; one elsewhere (Apple documents the URL as used only for
/// other directories, so it may be the home Trash whatever the volume), or
/// none, can't tell.
fn answer(named: Option<PathBuf>, device: u64) -> TrashSupport {
    match named {
        Some(dir) if std::fs::metadata(&dir).is_ok_and(|m| m.is_dir() && m.dev() == device) => {
            TrashSupport::Yes
        }
        _ => TrashSupport::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::{answer, supports};
    use crate::catalog::file_ops::TrashSupport;
    use std::os::unix::fs::MetadataExt as _;

    #[test]
    fn a_trash_folder_on_the_files_volume_says_yes_and_any_other_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let device = std::fs::metadata(dir.path()).unwrap().dev();
        let at = |p: &std::path::Path| Some(p.to_path_buf());
        assert_eq!(answer(at(dir.path()), device), TrashSupport::Yes);
        assert_eq!(answer(at(dir.path()), device ^ 1), TrashSupport::Unknown);
        let file = dir.path().join("a.jpg");
        std::fs::write(&file, b"a").unwrap();
        assert_eq!(answer(at(&file), device), TrashSupport::Unknown);
        assert_eq!(
            answer(at(&dir.path().join("gone")), device),
            TrashSupport::Unknown
        );
        // No Trash named: unsupported, or not made yet.
        assert_eq!(answer(None, device), TrashSupport::Unknown);
    }

    #[test]
    fn a_missing_path_is_unknown() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(supports(&dir.path().join("gone")), TrashSupport::Unknown);
    }
}
