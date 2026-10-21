//! Unix: `lstat` for entries, `renameat2(RENAME_NOREPLACE)` on Linux and
//! `renameatx_np(RENAME_EXCL)` on macOS through rustix, `statvfs` and
//! `statfs` for volumes (`f_bavail * f_frsize` the bytes available).
use super::{Entry, EntryKind, FileId, FsError, VolumeFacts, VolumeId, VolumeKind};
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

/// The file object an open file is.
pub(super) fn handle_id(file: &std::fs::File) -> Result<FileId, FsError> {
    let m = file.metadata()?;
    Ok(FileId {
        volume: VolumeId(m.dev()),
        index: m.ino(),
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
    let local = is_local(path)?;
    let kind = kind(path)?;
    Ok(VolumeFacts {
        kind,
        local,
        read_only: vfs.f_flag.contains(StatVfsMountFlags::RDONLY),
        // APFS renames a name to another spelling of itself in place
        // (tested in `a_case_only_rename_works_on_apfs`); elsewhere a
        // volume that folds names may find the new spelling taken, or
        // rename nothing.
        renames_spelling: cfg!(target_vendor = "apple") && kind == VolumeKind::Apfs,
        stable_ids: local && kind.keeps_ids(),
        exclusive_rename: kind.exclusive_rename(),
        available: vfs.f_bavail.saturating_mul(vfs.f_frsize),
    })
}

/// macOS names the file system.
#[cfg(target_vendor = "apple")]
fn kind(path: &Path) -> Result<VolumeKind, FsError> {
    let fs = rustix::fs::statfs(path).map_err(|e| FsError::from(std::io::Error::from(e)))?;
    let name: Vec<u8> = fs
        .f_fstypename
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    Ok(kind_of(&name))
}

/// The kind of macOS's file system `name` (`f_fstypename`): FSKit's and the
/// kernel's FAT and exFAT have the same names.
#[cfg(target_vendor = "apple")]
pub(super) fn kind_of(name: &[u8]) -> VolumeKind {
    match name {
        b"apfs" => VolumeKind::Apfs,
        b"hfs" => VolumeKind::Hfs,
        b"msdos" => VolumeKind::Fat,
        b"exfat" => VolumeKind::Exfat,
        _ => VolumeKind::Other,
    }
}

/// Linux numbers the file system type.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn kind(path: &Path) -> Result<VolumeKind, FsError> {
    let fs = rustix::fs::statfs(path).map_err(|e| FsError::from(std::io::Error::from(e)))?;
    #[allow(
        clippy::unnecessary_cast,
        reason = "the type differs between architectures"
    )]
    let kind = fs.f_type as u64;
    Ok(kind_of(kind))
}

/// The kind of Linux's file system type `magic` (`f_type`). FUSE (exFAT
/// through exfat-fuse among others) is another kind.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(super) fn kind_of(magic: u64) -> VolumeKind {
    match magic {
        0xef53 => VolumeKind::Ext,
        0x5846_5342 => VolumeKind::Xfs,
        0x9123_683e => VolumeKind::Btrfs,
        0x4d44 => VolumeKind::Fat,
        0x2011_bab0 => VolumeKind::Exfat,
        _ => VolumeKind::Other,
    }
}

/// Elsewhere the file system can't be told.
#[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
fn kind(_path: &Path) -> Result<VolumeKind, FsError> {
    Ok(VolumeKind::Other)
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
