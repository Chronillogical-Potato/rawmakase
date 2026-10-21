//! A system RAWmakase has no file system calls for: nothing is moved.
use super::{Entry, FileId, FsError, VolumeFacts};
use std::path::Path;

pub(super) fn entry(_path: &Path) -> Result<Entry, FsError> {
    Err(FsError::Unsupported)
}
pub(super) fn rename_exclusive(_from: &Path, _to: &Path) -> Result<(), FsError> {
    Err(FsError::Unsupported)
}
pub(super) fn sync_dir(_dir: &Path) -> Result<(), FsError> {
    Err(FsError::Unsupported)
}
pub(super) fn volume_facts(_path: &Path) -> Result<VolumeFacts, FsError> {
    Err(FsError::Unsupported)
}
pub(super) fn handle_id(_file: &std::fs::File) -> Result<FileId, FsError> {
    Err(FsError::Unsupported)
}
