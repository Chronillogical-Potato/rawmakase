//! The file system as execution and recovery see it: exactly the
//! [`fsops`] calls they make, behind one trait, so tests inject failures
//! and crashes at any step and record which folders were synced. The real
//! one is [`Real`]; nothing else implements it outside tests.
use crate::storage::Identity;
use crate::storage::fsops::{self, Entry, FsError};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub(super) trait Disk {
    fn entry(&self, path: &Path) -> Result<Entry, FsError>;
    fn list(&self, dir: &Path) -> Result<Vec<OsString>, FsError>;
    fn identity(&self, path: &Path) -> anyhow::Result<Identity>;
    fn rename(&self, from: &Path, to: &Path) -> Result<(), FsError>;
    fn sync_dir(&self, dir: &Path) -> Result<(), FsError>;
    /// `path` with every link on the way resolved.
    fn canonical(&self, path: &Path) -> Result<PathBuf, FsError>;
}

/// The file system itself.
pub(super) struct Real;

impl Disk for Real {
    fn entry(&self, path: &Path) -> Result<Entry, FsError> {
        fsops::entry(path)
    }
    fn list(&self, dir: &Path) -> Result<Vec<OsString>, FsError> {
        fsops::list(dir)
    }
    fn identity(&self, path: &Path) -> anyhow::Result<Identity> {
        Identity::read(path)
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<(), FsError> {
        fsops::rename_exclusive(from, to)
    }
    fn sync_dir(&self, dir: &Path) -> Result<(), FsError> {
        fsops::sync_dir(dir)
    }
    fn canonical(&self, path: &Path) -> Result<PathBuf, FsError> {
        Ok(std::fs::canonicalize(path)?)
    }
}

/// The folder `path` is named in.
pub(super) fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new(""))
}

/// Where a journalled file object is, by its exact name in its folder's
/// listing and its file id: never by looking its name up, which on a
/// case-insensitive or normalising volume finds `A.NEF` as `a.NEF`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Presence {
    /// The name is in the listing, and is that file object.
    Here,
    /// The folder listed whole, is on the file object's volume (so that
    /// volume is there), and has no such name.
    Absent,
    /// Anything else: another file object has the name, the folder can't be
    /// listed or is on another volume (offline, or a mount point left
    /// behind), or reading failed.
    Unknown,
}

/// Whether the file object `id` is at `path`.
pub(super) fn presence(disk: &dyn Disk, path: &Path, id: fsops::FileId) -> Presence {
    let (dir, Some(name)) = (parent(path), path.file_name()) else {
        return Presence::Unknown;
    };
    let Ok(names) = disk.list(dir) else {
        return Presence::Unknown;
    };
    if names.iter().any(|n| n == name) {
        return match disk.entry(path) {
            Ok(e) if e.id == id => Presence::Here,
            _ => Presence::Unknown,
        };
    }
    match disk.entry(dir) {
        Ok(d) if d.id.volume() == id.volume() => Presence::Absent,
        _ => Presence::Unknown,
    }
}

/// Whether the file object `id` is gone from `path`: [`Presence::Absent`],
/// no name in the folder is `path`'s folded, and looking `path` up finds
/// nothing, so it isn't there under another spelling or alias either. What
/// lets a trashed photo's rows go.
pub(super) fn gone(disk: &dyn Disk, path: &Path, id: fsops::FileId) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    let wanted = super::super::sidecar::caseless(name);
    // And looked up, which finds it under any name the volume takes for
    // this one, beyond what folding tells (`ß.NEF` as `SS.NEF`).
    presence(disk, path, id) == Presence::Absent
        && matches!(disk.entry(path), Err(FsError::NotFound))
        && disk.list(parent(path)).is_ok_and(|names| {
            names
                .iter()
                .all(|n| super::super::sidecar::caseless(n) != wanted)
        })
}

/// Syncs the folders of `from` and `to` after a rename between them, so
/// the journal moves past the step only once it is durable.
pub(super) fn sync_parents(disk: &dyn Disk, from: &Path, to: &Path) -> Result<(), FsError> {
    let (a, b) = (parent(from), parent(to));
    disk.sync_dir(a)?;
    if b != a {
        disk.sync_dir(b)?;
    }
    Ok(())
}
