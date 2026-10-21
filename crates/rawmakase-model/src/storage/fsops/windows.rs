//! Windows: `GetFileInformationByHandle` for entries, `MoveFileExW` without
//! `MOVEFILE_REPLACE_EXISTING` or `MOVEFILE_COPY_ALLOWED` for the exclusive
//! rename, and the volume's drive type and flags.
use super::{Entry, EntryKind, FileId, FsError, VolumeFacts, VolumeId};
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::AsRawHandle as _;
use std::path::Path;
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_FILE_NOT_FOUND,
    ERROR_INVALID_FUNCTION, ERROR_NOT_SAME_DEVICE, ERROR_NOT_SUPPORTED, ERROR_PATH_NOT_FOUND,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    GetDriveTypeW, GetFileInformationByHandle, GetVolumeInformationW, GetVolumePathNameW,
    MOVEFILE_WRITE_THROUGH, MoveFileExW,
};
use windows_sys::Win32::System::SystemServices::FILE_READ_ONLY_VOLUME;
use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOTE;

/// `path` as a NUL-terminated wide string.
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain([0]).collect()
}

/// The error of the last call, as callers branch on it.
fn last_error() -> FsError {
    let e = std::io::Error::last_os_error();
    match e.raw_os_error().map(|c| c as u32) {
        Some(ERROR_ALREADY_EXISTS | ERROR_FILE_EXISTS) => FsError::AlreadyExists,
        Some(ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND) => FsError::NotFound,
        Some(ERROR_ACCESS_DENIED) => FsError::PermissionDenied,
        Some(ERROR_NOT_SAME_DEVICE) => FsError::CrossDevice,
        Some(ERROR_INVALID_FUNCTION | ERROR_NOT_SUPPORTED) => FsError::Unsupported,
        _ => FsError::from(e),
    }
}

pub(super) fn entry(path: &Path) -> Result<Entry, FsError> {
    let t = std::fs::symlink_metadata(path)?.file_type();
    // Opened for no access, so a photo another program has open can still
    // be identified, and as the reparse point itself rather than its target.
    let file = std::fs::OpenOptions::new()
        .access_mode(0)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    // SAFETY: an all-zero BY_HANDLE_FILE_INFORMATION is a valid value of
    // the plain C struct, which the call then fills.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: the handle is open for the duration of the call, and `info`
    // is a valid, writable struct of the type the call expects.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(last_error());
    }
    let kind = if t.is_symlink() {
        EntryKind::Symlink
    } else if t.is_dir() {
        EntryKind::Directory
    } else if t.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    Ok(Entry {
        id: FileId {
            volume: VolumeId(u64::from(info.dwVolumeSerialNumber)),
            index: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
        },
        kind,
        links: u64::from(info.nNumberOfLinks),
    })
}

pub(super) fn rename_exclusive(from: &Path, to: &Path) -> Result<(), FsError> {
    let (from, to) = (wide(from), wide(to));
    // SAFETY: both are NUL-terminated wide strings that outlive the call.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(last_error());
    }
    Ok(())
}

/// NTFS journals renames, and a folder can't be opened to sync it.
pub(super) fn sync_dir(dir: &Path) -> Result<(), FsError> {
    if !std::fs::metadata(dir)?.is_dir() {
        return Err(FsError::NotFound);
    }
    Ok(())
}

pub(super) fn volume_facts(path: &Path) -> Result<VolumeFacts, FsError> {
    let path = wide(path);
    let mut root = vec![0u16; 1024];
    // SAFETY: `path` is NUL-terminated and `root` holds the length given.
    if unsafe { GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) } == 0 {
        return Err(last_error());
    }
    let mut flags = 0u32;
    // SAFETY: `root` is the NUL-terminated volume path the call above
    // wrote; the name buffers are null with length 0, which the call
    // allows, and `flags` is a valid place for the flags.
    let read = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut flags,
            std::ptr::null_mut(),
            0,
        )
    };
    if read == 0 {
        return Err(last_error());
    }
    // SAFETY: `root` is NUL-terminated.
    let remote = unsafe { GetDriveTypeW(root.as_ptr()) } == DRIVE_REMOTE;
    Ok(VolumeFacts {
        local: !remote,
        read_only: flags & FILE_READ_ONLY_VOLUME != 0,
        // Not tried on Windows' volumes yet.
        renames_spelling: false,
    })
}
