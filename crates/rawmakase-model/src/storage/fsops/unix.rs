//! Unix: `lstat` for entries, `renameat2(RENAME_NOREPLACE)` on Linux and
//! `renameatx_np(RENAME_EXCL)` on macOS through rustix, `statvfs` and
//! `statfs` for volumes.
use super::{Entry, EntryKind, FileId, FsError, VolumeFacts, VolumeId};
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;

pub(super) fn entry(path: &Path) -> Result<Entry, FsError> {
    let m = std::fs::symlink_metadata(path)?;
    let t = m.file_type();
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
            volume: VolumeId(m.dev()),
            index: m.ino(),
        },
        kind,
        links: m.nlink(),
    })
}

#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
pub(super) fn rename_exclusive(from: &Path, to: &Path) -> Result<(), FsError> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(errno)
}

/// Other Unix systems have no exclusive rename rustix reaches.
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
pub(super) fn rename_exclusive(_from: &Path, _to: &Path) -> Result<(), FsError> {
    Err(FsError::Unsupported)
}

/// What a failed rename's error means here. A file system without the
/// exclusive flag answers `EINVAL` on Linux and `ENOTSUP` on macOS (exFAT,
/// some network shares).
#[cfg(any(target_vendor = "apple", target_os = "linux", target_os = "android"))]
fn errno(e: rustix::io::Errno) -> FsError {
    use rustix::io::Errno;
    match e {
        Errno::EXIST | Errno::NOTEMPTY => FsError::AlreadyExists,
        Errno::NOENT => FsError::NotFound,
        Errno::ACCESS | Errno::PERM => FsError::PermissionDenied,
        Errno::XDEV => FsError::CrossDevice,
        Errno::INVAL | Errno::NOTSUP | Errno::NOSYS => FsError::Unsupported,
        e => FsError::Io(e.into()),
    }
}

pub(super) fn sync_dir(dir: &Path) -> Result<(), FsError> {
    Ok(std::fs::File::open(dir)?.sync_all()?)
}

pub(super) fn volume_facts(path: &Path) -> Result<VolumeFacts, FsError> {
    use rustix::fs::{StatVfsMountFlags, statvfs};
    let vfs = statvfs(path).map_err(|e| FsError::from(std::io::Error::from(e)))?;
    Ok(VolumeFacts {
        local: is_local(path)?,
        read_only: vfs.f_flag.contains(StatVfsMountFlags::RDONLY),
        renames_spelling: renames_spelling(path)?,
    })
}

/// APFS renames a name to another spelling of itself in place (tested in
/// `a_case_only_rename_works_on_apfs`).
#[cfg(target_vendor = "apple")]
fn renames_spelling(path: &Path) -> Result<bool, FsError> {
    let fs = rustix::fs::statfs(path).map_err(|e| FsError::from(std::io::Error::from(e)))?;
    let kind: Vec<u8> = fs
        .f_fstypename
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    Ok(kind == b"apfs")
}

/// Elsewhere a volume that folds names may find the new spelling taken, or
/// rename nothing, so none is taken to.
#[cfg(not(target_vendor = "apple"))]
fn renames_spelling(_path: &Path) -> Result<bool, FsError> {
    Ok(false)
}

/// macOS says so in the mount's flags.
#[cfg(target_vendor = "apple")]
fn is_local(path: &Path) -> Result<bool, FsError> {
    let fs = rustix::fs::statfs(path).map_err(|e| FsError::from(std::io::Error::from(e)))?;
    Ok(fs.f_flags & libc::MNT_LOCAL as u32 != 0)
}

/// Linux names the file system type: network ones, and FUSE, which GVFS
/// shares and sshfs are, are not local.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn is_local(path: &Path) -> Result<bool, FsError> {
    const REMOTE: [u64; 11] = [
        0x6969,      // NFS
        0x517b,      // SMB
        0xff53_4d42, // CIFS
        0xfe53_4d42, // SMB2
        0x6573_5546, // FUSE
        0x5346_414f, // AFS
        0x00c3_6400, // Ceph
        0x7375_7245, // Coda
        0x0102_1997, // 9P
        0x0bd0_0bd0, // Lustre
        0x0000_564c, // NCP
    ];
    let fs = rustix::fs::statfs(path).map_err(|e| FsError::from(std::io::Error::from(e)))?;
    #[allow(
        clippy::unnecessary_cast,
        reason = "the type differs between architectures"
    )]
    let kind = fs.f_type as u64;
    Ok(!REMOTE.contains(&kind))
}

/// Elsewhere it can't be told, so no volume is taken for local.
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
fn is_local(_path: &Path) -> Result<bool, FsError> {
    Ok(false)
}
