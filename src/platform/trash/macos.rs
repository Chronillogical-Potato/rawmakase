//! macOS: `NSFileManager`'s `trashItemAtURL:resultingItemURL:error:`, which
//! moves a file to the Trash of its volume and says where it went. That
//! resulting item, as a path, is the receipt: success without one is a
//! failure. A volume without a Trash answers `NSFeatureUnsupportedError`
//! ([`TrashError::Unsupported`]); any other error is
//! [`TrashError::Failed`]. Neither deletes the file.
use crate::catalog::file_ops::{TrashError, TrashReceipt};
use objc2::rc::autoreleasepool;
use objc2_foundation::{NSError, NSFeatureUnsupportedError, NSFileManager, NSURL};
use std::path::Path;

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
