//! Which file a name is, on a volume whose file ids change (#573): FAT and
//! exFAT number files anew when they are written, renamed or mounted again,
//! and give a number to another file later, so there a file object proves
//! nothing. A copy there is proven by its exact name, its size and its
//! SHA-256 (the bytes copied, journalled), an original by its exact name,
//! its `Identity` and, before it goes to the Trash, its SHA-256 again. An
//! item journals which proof each side takes ([`Proofs`]), so recovery
//! decides as it was copied.
//!
//! As by file ids, absence is decided only from a whole listing of the
//! folder, and only while the folder is on such a volume (a mount point left
//! behind is another volume's empty folder); a name present but not proven
//! is never taken for gone.
//!
//! macOS's FAT and exFAT list every name decomposed (NFD), but rename (FAT)
//! and remove (exFAT) a file only by the spelling its name is stored in, as
//! it was made (measured on macOS 15); opening, looking up and making work
//! by either. So every file RAWmakase makes there is named composed
//! ([`composed`]) and found by that name; an original, whose stored spelling
//! the listing doesn't tell, is tried composed, then decomposed, where it
//! goes to the Trash. A listed name is a journalled one when they are the
//! same composed, case and all.
use super::disk::{Disk, Presence, parent};
use crate::storage::fsops::{ContentHash, FsError, VolumeKind};
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// How one side of a copy proves which file a name is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::catalog) enum Proof {
    /// By file id, on a volume that keeps them.
    #[default]
    Ids,
    /// By exact name, size and SHA-256, on FAT and exFAT.
    Names,
}

/// How an item's copies are made and proven, journalled with it. Absent
/// from items of volumes that keep ids, as 0.4.2 journalled them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::catalog) struct Proofs {
    /// Its originals'.
    #[serde(default)]
    pub originals: Proof,
    /// Its copies': by names, a copy's file object is journalled as one no
    /// file has (`FileId::NOBODY`), so a release before #573 finds no file
    /// it is and leaves the item for the user.
    #[serde(default)]
    pub copies: Proof,
    /// Its copies are made at their final names: their volume has no
    /// exclusive rename (macOS's exFAT).
    #[serde(default)]
    pub direct: bool,
    /// When it was planned, in seconds since 1970: an empty file left where
    /// the journal says one was being made, before its making was
    /// journalled, is taken for its copy only when made after this.
    #[serde(default)]
    pub since: i64,
}

impl Proofs {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The proof a volume of `facts` takes, if any: by ids where they stay, by
/// names on a local FAT or exFAT volume; none elsewhere (a network share,
/// FUSE, ReFS…), which is refused.
pub(super) fn of(facts: &crate::storage::fsops::VolumeFacts) -> Option<Proof> {
    match facts.kind {
        _ if facts.stable_ids => Some(Proof::Ids),
        VolumeKind::Fat | VolumeKind::Exfat if facts.local => Some(Proof::Names),
        _ => None,
    }
}

/// The names in `path`'s folder that `sidecar::caseless` takes for its
/// name, when the folder lists whole and is on a FAT or exFAT volume.
fn alike(disk: &dyn Disk, path: &Path) -> Option<Vec<OsString>> {
    let (dir, name) = (parent(path), path.file_name()?);
    let names = disk.list(dir).ok()?;
    let fat = disk
        .volume_facts(dir)
        .is_ok_and(|f| matches!(f.kind, VolumeKind::Fat | VolumeKind::Exfat));
    if !fat {
        return None;
    }
    let wanted = super::super::sidecar::caseless(name);
    Some(
        names
            .into_iter()
            .filter(|n| super::super::sidecar::caseless(n) == wanted)
            .collect(),
    )
}

/// `name` composed (NFC), the form a name is used and journalled in on FAT
/// and exFAT.
pub(super) fn composed(name: &std::ffi::OsStr) -> OsString {
    use unicode_normalization::UnicodeNormalization;
    match name.to_str() {
        Some(text) => text.nfc().collect::<String>().into(),
        None => name.to_os_string(),
    }
}

/// Listed name `name` in folder `dir` as it is used: [`composed`] where
/// that spelling finds the file the listed one does (macOS's FAT and exFAT
/// drivers list names decomposed and find a file by either); as listed
/// otherwise (a volume that finds a name only by its bytes, as Linux's do).
pub(super) fn usable(disk: &dyn Disk, dir: &Path, name: &std::ffi::OsStr) -> OsString {
    let nfc = composed(name);
    if nfc == name {
        return nfc;
    }
    match (disk.entry(&dir.join(&nfc)), disk.entry(&dir.join(name))) {
        (Ok(a), Ok(b)) if a.id == b.id => nfc,
        _ => name.to_os_string(),
    }
}

/// `path` with its last name decomposed, where that is another spelling
/// of the same file (macOS's FAT and exFAT, whose drivers rename and remove
/// a file only by the spelling it is stored in); never a file of its own
/// (a volume that tells the spellings apart, as Linux's do).
pub(super) fn other_spelling(disk: &dyn Disk, path: &Path) -> Option<PathBuf> {
    let other = decomposed_path(path);
    let same = other != path
        && match (disk.entry(path), disk.entry(&other)) {
            (Ok(a), Ok(b)) => a.id == b.id,
            _ => false,
        };
    same.then_some(other)
}

/// `path` with its last name [`composed`].
pub(super) fn composed_path(path: &Path) -> PathBuf {
    match path.file_name() {
        Some(name) => parent(path).join(composed(name)),
        None => path.to_path_buf(),
    }
}

/// `path` with its last name decomposed (NFD): the other spelling of a name
/// a file may be stored in on FAT and exFAT, whose drivers rename and remove
/// it only by the spelling it is stored in.
pub(super) fn decomposed_path(path: &Path) -> PathBuf {
    use unicode_normalization::UnicodeNormalization;
    match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => parent(path).join(name.nfd().collect::<String>()),
        None => path.to_path_buf(),
    }
}

/// Whether a file is at `path`, by its exact name: [`Presence::Here`] when
/// its folder's listing has that name, composed (case and all), and no
/// other `sidecar::caseless` takes for it; [`Presence::Absent`] when it has
/// none of them; and [`Presence::Unknown`] otherwise.
pub(super) fn presence(disk: &dyn Disk, path: &Path) -> Presence {
    let Some(alike) = alike(disk, path) else {
        return Presence::Unknown;
    };
    let wanted = path.file_name().map(composed);
    match alike.as_slice() {
        [] => Presence::Absent,
        [one] if Some(composed(one)) == wanted => Presence::Here,
        _ => Presence::Unknown,
    }
}

/// `path` as it is used on its volume: its name [`composed`], once its
/// folder's listing has it ([`presence`]).
pub(super) fn spelled(disk: &dyn Disk, path: &Path) -> Result<PathBuf, FsError> {
    match presence(disk, path) {
        Presence::Here => Ok(composed_path(path)),
        _ => Err(FsError::Io(std::io::Error::other(
            "its name in its folder can't be told",
        ))),
    }
}

/// Whether nothing is at `path` by any spelling: its folder lists whole
/// with no name `sidecar::caseless` takes for it, and looking it up finds
/// nothing.
pub(super) fn gone(disk: &dyn Disk, path: &Path) -> bool {
    presence(disk, path) == Presence::Absent && matches!(disk.entry(path), Err(FsError::NotFound))
}

/// Whether the file at `path` (found by [`presence`]) holds the bytes
/// hashed as `content`.
pub(super) fn holds(disk: &dyn Disk, path: &Path, content: ContentHash) -> bool {
    presence(disk, path) == Presence::Here && disk.hash(path).is_ok_and(|h| h == content)
}

/// Whether the file at `path` holds the first bytes of `original`, as many
/// as it has: a partial copy of it.
pub(super) fn begins(disk: &dyn Disk, path: &Path, original: &Path) -> bool {
    let Ok(copy) = disk.identity(path) else {
        return false;
    };
    presence(disk, path) == Presence::Here
        && match (disk.hash(path), disk.hash_prefix(original, copy.size)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
}

/// Whether the empty file at `path` was made after `since` (seconds since
/// 1970), allowing for FAT's two-second step.
pub(super) fn made_after(disk: &dyn Disk, path: &Path, since: i64) -> bool {
    born_after(disk, path, since) && disk.identity(path).is_ok_and(|i| i.size == 0)
}

/// Whether what is at `path` (a file or a folder) was made after `since`,
/// as [`made_after`] allows.
pub(super) fn born_after(disk: &dyn Disk, path: &Path, since: i64) -> bool {
    let born = disk
        .created(path)
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    born.is_some_and(|b| b + 2 >= since)
}
