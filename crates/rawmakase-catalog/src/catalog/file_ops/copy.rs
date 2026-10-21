//! Moving a photo's files to another volume, where no rename reaches: each
//! file copied, verified and given its name there, then the catalog, then
//! the originals to the Trash. Every step is journalled before the next.
//!
//! Per member (the photo first, then its companions):
//! 1. its partial copy, hidden in the destination folder by a name unique
//!    to the run, the item and the member, made where nothing has that
//!    name, and its file object journalled (`copying`);
//! 2. the original's bytes copied into it, hashed as they are read
//!    (stopping between chunks when the run is stopped), the hash
//!    journalled, synced, given the original's time and permission bits,
//!    and synced again;
//! 3. read back and compared with the hash, and the original checked to be
//!    the file object and `Identity` journalled;
//! 4. renamed exclusively to its name there, the folder synced, and the
//!    member journalled `copied`.
//!
//! Then, in one transaction, the evidence (every copy at its destination,
//! every original still the file it copied), the rows following the
//! copies, the photo's saved edit and checksum re-keyed to the copy when
//! its volume keeps times more coarsely, and the item `committed`. Last,
//! each original still that file goes to the Trash (`trashed`), or stays,
//! with why (`original_kept`); the item is `done`.
//!
//! Anything that fails before the commit undoes the item: partial copies
//! are removed, complete ones put in the Trash, and nothing else changes.
//! The partial copy is journalled as the disk spells its name (HFS+ keeps
//! names decomposed) and found by any spelling of it with its file object.
//! Recovery does the same for an item a crash left before its commit
//! ([`decide`]), and finishes one left after it.
use super::super::Catalog;
use super::super::db::{Write, sql};
use super::super::sidecar::caseless;
use super::companions::is_legacy_edit;
use super::disk::{Disk, Presence, gone, parent, presence, spelled};
use super::journal::{self, Item, ItemState, Member, MemberState, OperationId, Subject};
use super::plan::{self, Folders, occupied, shared};
use super::recover::NOT_BACK;
use super::{Trash, TrashError};
use crate::storage::Identity;
use crate::storage::fsops::{CopyError, EntryKind, FileId, FsError};
use anyhow::{Context, Result};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// What copying needs of a run or a recovery.
pub(super) struct Cx<'a> {
    pub computer: &'a str,
    pub folders: &'a Folders,
    pub disk: &'a dyn Disk,
    pub trash: &'a dyn Trash,
}

/// Why a move to another volume changed nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Problem {
    /// The run was stopped.
    Stopped,
    /// The destination drive is full.
    NoSpace,
    /// The original changed, or went, while it was copied.
    Changed(PathBuf),
    /// The copy read back otherwise than the original was read.
    Mismatch(PathBuf),
    /// A file of that name came to the destination meanwhile.
    NameTaken(String),
    /// The destination drive didn't keep the file's time.
    TimeNotKept(String),
    /// The photo has an edit file of an earlier release, which names the
    /// photo's time, and the destination drive keeps times more coarsely.
    LegacyEdit(String),
    Disk(String),
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => f.write_str("Stopped."),
            Self::NoSpace => f.write_str("The other drive is full."),
            Self::Changed(p) => write!(f, "{} changed while it was copied.", p.display()),
            Self::Mismatch(p) => write!(
                f,
                "The copy of {} didn't match the original, so nothing was moved.",
                p.display()
            ),
            Self::NameTaken(n) => write!(f, "A file named {n} is there now."),
            Self::TimeNotKept(n) => {
                write!(f, "The other drive didn't keep the date of {n}.")
            }
            Self::LegacyEdit(n) => write!(
                f,
                "{n} was edited by an earlier release, whose edit file the other drive's dates wouldn't match. Open and save its edit first."
            ),
            Self::Disk(e) => f.write_str(e),
        }
    }
}

fn problem(e: FsError) -> Problem {
    match e {
        FsError::NoSpace => Problem::NoSpace,
        e => Problem::Disk(e.to_string()),
    }
}

/// Why a member's copy didn't finish: a disk step, or the journal.
pub(super) enum Failed {
    Problem(Problem),
    Journal(anyhow::Error),
}

impl From<Problem> for Failed {
    fn from(p: Problem) -> Self {
        Self::Problem(p)
    }
}

/// Copies journalled item `id`, planned as `item`, then lets the catalog
/// follow and puts its originals in the Trash; undoes it on any failure
/// before the catalog follows. Errs only when the catalog can't be written.
pub(super) fn run(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    item: &plan::Item,
    id: OperationId,
    cancel: &AtomicBool,
) -> Result<()> {
    let temps = item.copy.as_ref().map_or(&[][..], |c| &c.temps[..]);
    for (ordinal, m) in item.members.iter().enumerate() {
        let (Some(to), Some(temp)) = (m.destination.as_deref(), temps.get(ordinal)) else {
            return undo(cx, catalog, id, "it has no destination");
        };
        let copied = copy_one(cx, catalog, id, ordinal as i64, m, temp, to, cancel);
        match copied {
            Ok(()) => {}
            Err(Failed::Problem(p)) => return undo(cx, catalog, id, &p.to_string()),
            Err(Failed::Journal(e)) => return undo(cx, catalog, id, &format!("{e:#}")),
        }
    }
    let committed = catalog.db.write_immediate(|w| {
        let journalled = journal::item(w, id)?.context("the journalled item is gone")?;
        commit(w, cx, &journalled)
    });
    match committed {
        Ok(None) => {}
        Ok(Some(why)) => return undo(cx, catalog, id, &why),
        Err(e) => return undo(cx, catalog, id, &format!("{e:#}")),
    }
    let journalled = journal::item(&catalog.db, id)?.context("the journalled item is gone")?;
    finish(cx, catalog, &journalled, false)
}

/// Steps 1 to 4 for member `ordinal`, planned as `m`, through its partial
/// copy `temp` to `to`.
#[allow(clippy::too_many_arguments, reason = "one member's step")]
fn copy_one(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    id: OperationId,
    ordinal: i64,
    m: &journal::NewMember,
    temp: &Path,
    to: &Path,
    cancel: &AtomicBool,
) -> Result<(), Failed> {
    let disk = cx.disk;
    let (temp, made) = write(
        cx,
        catalog,
        (id, ordinal),
        (&m.source, m.id, &m.identity),
        temp,
        cancel,
    )?;
    // 4. Its name, never replacing one there.
    disk.rename(&temp, to).map_err(|e| match e {
        FsError::AlreadyExists => Problem::NameTaken(name(to)),
        e => problem(e),
    })?;
    disk.sync_dir(parent(to)).map_err(problem)?;
    // As the disk spells it, which a volume that normalises names may not
    // as given.
    let at = spelled(disk, to, made).map_err(problem)?;
    catalog
        .db
        .write(|w| journal::set_copied(w, id, ordinal, &at))
        .map_err(Failed::Journal)?;
    Ok(())
}

/// Steps 1 to 3 for member `ordinal` of item `id`: the original `source`
/// (file object and `Identity` as planned) copied into a file made at
/// `temp` where nothing had its name, journalled as it goes, synced, given
/// the original's time, read back, and the original checked to be still
/// what was copied. Returns the copy's path as the disk spells it, and its
/// file object.
pub(super) fn write(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    (id, ordinal): (OperationId, i64),
    (source, source_id, identity): (&Path, FileId, &Identity),
    temp: &Path,
    cancel: &AtomicBool,
) -> Result<(PathBuf, FileId), Failed> {
    let disk = cx.disk;
    let journal = |catalog: &mut Catalog, f: &dyn Fn(&mut Write<'_>) -> Result<()>| {
        catalog.db.write(|w| f(w)).map_err(Failed::Journal)
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(Problem::Stopped.into());
    }
    // 1. Made where nothing is; what a crash leaves before it is journalled
    // is found by its name, empty.
    disk.create_file(temp).map_err(|e| match e {
        FsError::AlreadyExists => Problem::NameTaken(name(temp)),
        e => problem(e),
    })?;
    let made = disk.entry(temp).map_err(problem)?;
    if made.kind != EntryKind::File || made.links != 1 {
        return Err(Problem::Disk(format!("{} isn't the file made", temp.display())).into());
    }
    // As the disk spells it (HFS+ keeps names decomposed), which every
    // later step and recovery find it by.
    let temp = spelled(disk, temp, made.id).map_err(problem)?;
    journal(catalog, &|w| {
        journal::set_copying(w, id, ordinal, Some(made.id), &temp)
    })?;
    // 2. Copied, durable, with the original's time.
    let content = disk
        .copy_into(source, &temp, Some(made.id), &mut |_| {
            !cancel.load(Ordering::Relaxed)
        })
        .map_err(|e| match e {
            CopyError::Stopped => Problem::Stopped,
            CopyError::NotTheFile => Problem::Disk(e.to_string()),
            CopyError::Fs(e) => problem(e),
        })?;
    // Written whole: from here it is never removed while its original
    // can't be proven, whatever fails next.
    journal(catalog, &|w| journal::set_content(w, id, ordinal, content))?;
    disk.sync_file(&temp).map_err(problem)?;
    disk.copy_times(source, &temp).map_err(problem)?;
    disk.sync_file(&temp).map_err(problem)?;
    // 3. Read back, and the original still what was copied.
    if disk.hash(&temp).map_err(problem)? != content {
        return Err(Problem::Mismatch(source.to_path_buf()).into());
    }
    if !unchanged(disk, source, source_id, identity) {
        return Err(Problem::Changed(source.to_path_buf()).into());
    }
    Ok((temp, made.id))
}

/// Whether `path` is file object `id` with `identity`, looked up by its
/// name: what checks the original between its copy's steps.
fn unchanged(disk: &dyn Disk, path: &Path, id: FileId, identity: &Identity) -> bool {
    disk.entry(path)
        .is_ok_and(|e| e.id == id && e.kind == EntryKind::File)
        && disk.identity(path).ok().as_ref() == Some(identity)
}

/// Whether member `m`'s original is proven at its source as it was copied:
/// that file object, by its exact name in its folder's listing, with its
/// journalled `Identity`.
pub(super) fn original_here(disk: &dyn Disk, m: &Member) -> bool {
    match (m.id, &m.identity) {
        (Some(id), Some(identity)) => {
            presence(disk, &m.source, id) == Presence::Here
                && disk.identity(&m.source).ok().as_ref() == Some(identity)
        }
        _ => false,
    }
}

/// Whether `copy`'s time differs from `original`'s only as a volume that
/// keeps times more coarsely makes it (exFAT's 10 ms, FAT's 2 s, HFS+'s
/// 1 s, NTFS's 100 ns, and the like), its size and first bytes the same.
pub(super) fn coarser(original: &Identity, copy: &Identity) -> bool {
    const STEPS: [u128; 6] = [
        100,
        1_000,
        1_000_000,
        10_000_000,
        1_000_000_000,
        2_000_000_000,
    ];
    original.size == copy.size
        && original.prefix_hash == copy.prefix_hash
        && STEPS.iter().any(|step| {
            copy.modified_ns.is_multiple_of(*step)
                && copy.modified_ns.abs_diff(original.modified_ns) < *step
        })
}

/// Lets the catalog follow item `item`'s copies, in `w`'s transaction, if
/// the evidence allows, and marks it `committed`; `Some` with why not
/// otherwise, having changed nothing.
fn commit(w: &mut Write<'_>, cx: &Cx<'_>, item: &Item) -> Result<Option<String>> {
    let Subject::Photos(pre) = &item.subject else {
        return Ok(Some("not a photo operation".into()));
    };
    if let Some(why) = pre.check(w, cx.computer)? {
        return Ok(Some(why.into()));
    }
    let (true, Some(photo)) = (pre.to.is_some() || pre.into.is_some(), item.members.first()) else {
        return Ok(Some("it has no destination".into()));
    };
    let to = pre.to.as_ref().map(|(to, _)| *to);
    let filename = photo.source.file_name().unwrap_or_default();
    if shared(
        w,
        cx.folders,
        parent(&photo.source),
        filename,
        photo.id,
        &pre.rows,
    )? {
        return Ok(Some(
            "another photo in the catalog is the same file now".into(),
        ));
    }
    // Every copy at its destination, every original still what it copied.
    let mut copies = Vec::new();
    for m in &item.members {
        let (Some(at), Some(copy)) = (m.destination.as_deref(), &m.copy) else {
            return Ok(Some("it has no destination".into()));
        };
        let here = copy
            .id
            .is_some_and(|id| presence(cx.disk, at, id) == Presence::Here);
        if m.state != MemberState::Copied || !here {
            return Ok(Some("its copies aren't all where they were made".into()));
        }
        if !original_here(cx.disk, m) {
            return Ok(Some(Problem::Changed(m.source.clone()).to_string()));
        }
        let (Ok(now), Some(was)) = (cx.disk.identity(at), &m.identity) else {
            return Ok(Some(format!("{} can't be read", at.display())));
        };
        if now != *was && !coarser(was, &now) {
            return Ok(Some(Problem::TimeNotKept(name(at)).to_string()));
        }
        copies.push(now);
    }
    let (Some(was), Some(now)) = (&photo.identity, copies.first()) else {
        return Ok(Some("it has no files".into()));
    };
    let rekey = was != now;
    // An earlier release's edit file names the photo's time, which the
    // copy no longer has.
    let legacy = item.members[1..]
        .iter()
        .any(|m| is_legacy_edit(filename, m.source.file_name().unwrap_or_default()));
    if rekey && legacy {
        return Ok(Some(Problem::LegacyEdit(name(&photo.source)).to_string()));
    }
    let Some(arrived) = photo.destination.as_deref() else {
        return Ok(Some("it has no destination".into()));
    };
    let arrived_name = arrived.file_name().unwrap_or_default();
    if occupied(w, cx.folders, to, parent(arrived), arrived_name, &pre.rows)? {
        return Ok(Some(format!(
            "a photo of the catalog is named {} there now",
            arrived_name.to_string_lossy()
        )));
    }
    let text = arrived_name
        .to_str()
        .context("not a name the catalog keeps")?;
    let to = match super::execute::arrive(w, cx.computer, item.id, pre, parent(arrived))? {
        Ok(to) => to,
        Err(why) => return Ok(Some(why)),
    };
    w.execute(
        sql!("UPDATE photos SET folder=?, filename=? WHERE id=? OR master_id=?"),
        &[&to, &text, &pre.master, &pre.master],
    )?;
    // The same bytes, kept with a coarser time: what was saved for the
    // original is the copy's. Only what names exactly the original.
    if rekey {
        for row in &pre.rows {
            super::super::edit_rows::refresh_identity(w, *row, was, now)?;
        }
        super::super::checksums::refresh_identity(w, pre.master, was, now)?;
    }
    for (m, now) in item.members.iter().zip(&copies) {
        journal::set_copy_identity(w, item.id, m.ordinal, now)?;
    }
    journal::set_state(w, item.id, ItemState::Committed, None)?;
    Ok(None)
}

fn name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Puts committed item `item`'s originals in the Trash, each only while it
/// is still the file it copied and its copy is where the catalog says;
/// one that can't go stays, with why. Recovery (`recovering`) also reads
/// each copy back first. Marks it `done`.
fn finish(cx: &Cx<'_>, catalog: &mut Catalog, item: &Item, recovering: bool) -> Result<()> {
    let id = item.id;
    let mut unsynced = None;
    for m in &item.members {
        if m.state != MemberState::Copied {
            continue;
        }
        let kept = |catalog: &mut Catalog, why: &str| {
            catalog
                .db
                .write(|w| journal::set_kept(w, id, m.ordinal, why))
        };
        let (Some(at), Some(copy), Some(file)) = (m.destination.as_deref(), &m.copy, m.id) else {
            kept(catalog, "its copy isn't journalled")?;
            continue;
        };
        let copy_here = copy
            .id
            .is_some_and(|c| presence(cx.disk, at, c) == Presence::Here)
            && copy.identity.is_some()
            && cx.disk.identity(at).ok() == copy.identity;
        let read_back = !recovering
            || copy
                .content
                .is_some_and(|c| cx.disk.hash(at).is_ok_and(|now| now == c));
        if !copy_here || !read_back {
            kept(catalog, "its copy isn't as it was made")?;
            continue;
        }
        match presence(cx.disk, &m.source, file) {
            Presence::Here => {}
            // In the Trash, where a crash came before its receipt was
            // journalled, or gone some other way: nothing is left to do.
            Presence::Absent => {
                catalog
                    .db
                    .write(|w| journal::set_member(w, id, m.ordinal, MemberState::Trashed, None))?;
                continue;
            }
            Presence::Unknown => {
                kept(catalog, "whether it is still there can't be told")?;
                continue;
            }
        }
        if !original_here(cx.disk, m) {
            kept(catalog, "it changed since it was copied")?;
            continue;
        }
        let receipt = match cx.trash.trash(&m.source) {
            Ok(receipt) => receipt.location,
            Err(e @ TrashError::Unsupported) => {
                kept(catalog, &e.to_string())?;
                continue;
            }
            Err(TrashError::Failed(e)) => {
                kept(catalog, &e)?;
                continue;
            }
        };
        if let Err(e) = cx.disk.sync_dir(parent(&m.source)) {
            unsynced = Some(format!("{}: {e}", parent(&m.source).display()));
        }
        catalog.db.write(|w| {
            journal::set_member(w, id, m.ordinal, MemberState::Trashed, Some(&receipt))
        })?;
        if !gone(cx.disk, &m.source, file) {
            kept(catalog, "the Trash took it, but it is still there")?;
        }
    }
    let note = unsynced.map(|d| format!("its folder couldn't be synced: {d}"));
    catalog
        .db
        .write(|w| journal::set_state(w, id, ItemState::Done, note.as_deref()))
}

/// What must hold of an item's originals for its copies to be discarded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Originals {
    /// Each the file object and `Identity` it was copied as: whatever is
    /// undone on its own.
    AsCopied,
    /// Each the file object it was, changed or not: what the user's Put
    /// Back asks, the originals being where they were.
    Present,
}

/// Whether every one of `item`'s originals is at its source as `rule` asks.
fn originals(disk: &dyn Disk, item: &Item, rule: Originals) -> bool {
    item.members.iter().all(|m| match rule {
        Originals::AsCopied => original_here(disk, m),
        Originals::Present => {
            m.id.is_some_and(|id| presence(disk, &m.source, id) == Presence::Here)
        }
    })
}

/// Undoes journalled item `id` before the catalog follows it, for `why`:
/// decided first (`restoring`), then each member's copies discarded; it
/// ends `no_change`, or stays being put back when a copy can't be.
pub(super) fn undo(cx: &Cx<'_>, catalog: &mut Catalog, id: OperationId, why: &str) -> Result<()> {
    undo_as(cx, catalog, id, why, Originals::AsCopied)
}

/// [`undo`], the originals held to `rule`.
fn undo_as(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    id: OperationId,
    why: &str,
    rule: Originals,
) -> Result<()> {
    catalog
        .db
        .write(|w| journal::set_state(w, id, ItemState::Restoring, Some(why)))?;
    let item = journal::item(&catalog.db, id)?.context("the journalled item is gone")?;
    let state = match discard(cx, catalog, &item, rule)? {
        true => (ItemState::NoChange, None),
        false => (ItemState::Restoring, Some(NOT_BACK)),
    };
    catalog
        .db
        .write(|w| journal::set_state(w, id, state.0, state.1))
}

/// Where a member's partial copy is.
enum Partial {
    /// Not there.
    Gone,
    /// There, at this name, and proven this run's.
    Ours(PathBuf),
    Unknown,
}

/// Where member `m`'s partial copy `temp` is, by any name its folder's
/// listing has that `sidecar::caseless` takes for `temp`'s (a volume may
/// spell it otherwise than it was given): by its journalled file object
/// ([`copy_at`]); or, made but not journalled yet, the one such name, only
/// while it is an empty plain file.
fn partial(disk: &dyn Disk, m: &Member, temp: &Path) -> Partial {
    if let Some(id) = m.copy.as_ref().and_then(|c| c.id) {
        return match copy_at(disk, temp, id) {
            Some(Some(at)) => Partial::Ours(at),
            Some(None) => Partial::Gone,
            None => Partial::Unknown,
        };
    }
    let (dir, Some(name)) = (parent(temp), temp.file_name()) else {
        return Partial::Unknown;
    };
    let Ok(names) = disk.list(dir) else {
        return Partial::Unknown;
    };
    let wanted = caseless(name);
    let alike: Vec<&std::ffi::OsString> = names.iter().filter(|n| caseless(n) == wanted).collect();
    let at = match alike.as_slice() {
        [] => return Partial::Gone,
        [one] => dir.join(one),
        _ => return Partial::Unknown,
    };
    let empty = disk
        .entry(&at)
        .is_ok_and(|e| e.kind == EntryKind::File && e.links == 1)
        && disk.identity(&at).is_ok_and(|i| i.size == 0);
    match empty {
        true => Partial::Ours(at),
        false => Partial::Unknown,
    }
}

/// Where the copy that is file object `id`, given the name `at`, is now:
/// `Some(Some(path))` at the one name in its folder's listing that
/// `sidecar::caseless` takes for `at`'s and that is that file object (a
/// volume may spell a name otherwise than it was given); `Some(None)` when
/// no such name is (the folder listed whole, on the copy's volume); `None`
/// when it can't be told, as when a file there has that name but is
/// another file object: on a volume that renumbers its files it may be the
/// copy, so its absence is never concluded from it.
pub(super) fn copy_at(disk: &dyn Disk, at: &Path, id: FileId) -> Option<Option<PathBuf>> {
    let dir = parent(at);
    let wanted = caseless(at.file_name()?);
    let mut ours = Vec::new();
    for name in disk
        .list(dir)
        .ok()?
        .iter()
        .filter(|n| caseless(n) == wanted)
    {
        match disk.entry(&dir.join(name)) {
            Ok(e) if e.id == id => ours.push(dir.join(name)),
            Ok(_) | Err(_) => return None,
        }
    }
    match ours.len() {
        0 => disk
            .entry(dir)
            .ok()
            .filter(|d| d.id.volume() == id.volume())
            .map(|_| None),
        1 => Some(ours.pop()),
        _ => None,
    }
}

/// Discards journalled `item`'s copies, newest first, each journalled
/// `discarded`: a partial copy is removed (one written whole only while
/// every original is proven at its source as it was copied, and to the
/// Trash instead on the user's Put Back); a complete one goes to the
/// Trash, only while it is proven the copy (its file object, its bytes the
/// original's), no photo of the catalog is named so there, and every
/// original is proven at its source. Returns whether every one is gone.
fn discard(cx: &Cx<'_>, catalog: &mut Catalog, item: &Item, rule: Originals) -> Result<bool> {
    let disk = cx.disk;
    let originals = originals(disk, item, rule);
    let rows: &[_] = item.photos();
    let to = match &item.subject {
        Subject::Photos(pre) => pre.to.as_ref().map(|(to, _)| *to),
        _ => None,
    };
    for m in item.members.iter().rev() {
        let Some(copy) = &m.copy else {
            return Ok(false);
        };
        match m.state {
            MemberState::Discarded => continue,
            MemberState::Planned | MemberState::Copying | MemberState::Copied => {}
            _ => return Ok(false),
        }
        let mut receipt = None;
        match partial(disk, m, &copy.temp) {
            // Never renamed, so nothing of it is at its destination. One
            // written whole stays while its original can't be proven: it
            // may be the one copy of those bytes left.
            Partial::Ours(_) if copy.content.is_some() && !originals => return Ok(false),
            // Put Back with an original changed since: written whole, it has
            // the bytes the original had, perhaps the only ones left, so it
            // goes to the Trash, never unlinked.
            Partial::Ours(at) if copy.content.is_some() && rule == Originals::Present => {
                let Ok(r) = cx.trash.trash(&at) else {
                    return Ok(false);
                };
                if disk.sync_dir(parent(&at)).is_err() {
                    return Ok(false);
                }
                receipt = Some(r.location);
            }
            Partial::Ours(at) => {
                let removed = disk
                    .remove_file(&at)
                    .and_then(|()| disk.sync_dir(parent(&at)));
                if removed.is_err() {
                    return Ok(false);
                }
            }
            Partial::Unknown => return Ok(false),
            Partial::Gone => {
                if let (Some(asked), Some(file)) = (m.destination.as_deref(), copy.id) {
                    match copy_at(disk, asked, file) {
                        Some(None) => {}
                        None => return Ok(false),
                        Some(Some(at)) => {
                            let at = at.as_path();
                            let proven = originals
                                && copy
                                    .content
                                    .is_some_and(|c| disk.hash(at).is_ok_and(|now| now == c));
                            let filename = at.file_name().unwrap_or_default();
                            let named =
                                occupied(&catalog.db, cx.folders, to, parent(at), filename, rows)?;
                            if !proven || named {
                                return Ok(false);
                            }
                            let Ok(r) = cx.trash.trash(at) else {
                                return Ok(false);
                            };
                            if disk.sync_dir(parent(at)).is_err() {
                                return Ok(false);
                            }
                            receipt = Some(r.location);
                        }
                    }
                }
            }
        }
        catalog
            .db
            .write(|w| journal::set_discarded(w, item.id, m.ordinal, receipt.as_deref()))?;
    }
    Ok(true)
}

/// Recovery of copy item `item`: one the catalog follows is finished;
/// otherwise its originals are what counts, and it is undone once every
/// one is proven at its source, or left for the user.
/// Put Back (`put_back`, the user's choice) asks only that each original is
/// still the file object it was, changed or not.
pub(super) fn decide(
    cx: &Cx<'_>,
    catalog: &mut Catalog,
    item: &Item,
    put_back: bool,
) -> Result<()> {
    let id = item.id;
    if item.state == ItemState::Committed {
        return finish(cx, catalog, item, true);
    }
    let rule = match put_back {
        true => Originals::Present,
        false => Originals::AsCopied,
    };
    if !originals(cx.disk, item, rule) {
        let left = match item.state {
            ItemState::Restoring => ItemState::Restoring,
            _ => ItemState::NeedsRecovery,
        };
        let why = match put_back {
            true => "It can't be put back: an original isn't where it was.",
            false => "an original isn't where it was copied from",
        };
        return catalog
            .db
            .write(|w| journal::set_state(w, id, left, Some(why)));
    }
    let why = item.error.as_deref().unwrap_or("it was interrupted");
    undo_as(cx, catalog, id, why, rule)
}
