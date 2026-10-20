//! Executing a confirmed plan, item by item, under the operation lock:
//!
//! a. In one transaction: the item's preconditions, its files and its
//!    destination checked again, and the item journalled with every member
//!    `planned` (or, if something changed, `no_change` with the reason).
//! b. The disk steps, member by member (the photo first, then its
//!    companions), each made durable and journalled before the next. A step
//!    that fails is put back: the item is marked `restoring` first, then its
//!    members already changed are renamed back; it ends `no_change`, or
//!    `needs_recovery` when that fails too.
//! c. In one transaction: the preconditions again, the evidence that the
//!    files are where the rows will say (or gone, with Trash receipts), the
//!    catalog change and `done`. Should that fail, the item is
//!    `needs_recovery` and recovery retries it.
//!
//! Cancelling stops between items only.
use super::super::db::{Write, sql};
use super::super::folder_sync::locate_folders;
use super::super::{Catalog, PhotoId};
use super::companions;
use super::disk::{Disk, Presence, Real, gone, parent, presence, sync_parents};
use super::journal::{self, Item as Journalled, ItemState, Kind, Member, MemberState, OperationId};
use super::plan::{Folders, Item, occupied, shared};
use super::{Confirmed, FileOpError, Outcome, Trash};
use crate::storage::fsops::{EntryKind, FsError};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::sync::atomic::{AtomicBool, Ordering};

/// Executes a confirmed plan in `catalog`, sending its files to `trash`
/// when it trashes them, and stopping between items once `cancel` is set.
/// The outcome is read from the journal at the end, as [`super::outcome`]
/// reads it after anything else happened to the thread.
pub fn execute(
    catalog: &mut Catalog,
    confirmed: Confirmed,
    trash: &dyn Trash,
    cancel: &AtomicBool,
) -> Result<Outcome, FileOpError> {
    execute_with(catalog, confirmed, trash, cancel, &Real)
}

/// [`execute`] on `disk`.
pub(super) fn execute_with(
    catalog: &mut Catalog,
    Confirmed(plan): Confirmed,
    trash: &dyn Trash,
    cancel: &AtomicBool,
    disk: &dyn Disk,
) -> Result<Outcome, FileOpError> {
    if plan.location != catalog.location || plan.computer != catalog.computer.id {
        return Err(FileOpError::WrongCatalog);
    }
    // Photos an unfinished operation involves are left alone. Remove from
    // Catalog touches no file, so it needs neither the lock nor recovery
    // (and works on a catalog that can't take the lock); it leaves alone the
    // photos of every computer's unfinished items. Anything else takes the
    // lock, and decides what a crash left unfinished first.
    let (_lock, busy): (_, HashSet<PhotoId>) = match plan.kind {
        // Checked in each removal's own transaction, which journalling a
        // disk step waits for (see `Run::remove`).
        Kind::RemoveFromCatalog => (None, HashSet::new()),
        Kind::MovePhotos | Kind::TrashPhotos => {
            let lock = super::lock::acquire(catalog)?;
            let recovery = super::recover::run(catalog, disk)?;
            let busy = recovery
                .unfinished
                .iter()
                .flat_map(|u| u.photos.iter().copied())
                .collect();
            (Some(lock), busy)
        }
    };
    let computer = catalog.computer.id.clone();
    let folders = Folders::index(&catalog.db.snapshot(|s| locate_folders(s, &computer))?);
    let run = Run {
        batch: &plan.batch.0,
        computer: &computer,
        kind: plan.kind,
        folders: &folders,
        trash,
        disk,
    };
    let mut stopped = None;
    for item in plan.items {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let result = if item.preconditions.rows.iter().any(|r| busy.contains(r)) {
            run.unchanged(catalog, &item, UNFINISHED)
        } else {
            match plan.kind {
                Kind::RemoveFromCatalog => run.remove(catalog, &item),
                Kind::MovePhotos | Kind::TrashPhotos => run.files(catalog, &item),
            }
        };
        // A catalog that can't be written stops the run; what was
        // journalled stands, and the rest is untouched.
        if let Err(e) = result {
            stopped = Some(format!("{e:#}"));
            break;
        }
    }
    let mut outcome = super::outcome(catalog, &plan.batch)?;
    outcome.stopped = stopped;
    Ok(outcome)
}

/// Why a photo an unfinished file operation involves is left alone.
const UNFINISHED: &str = "an unfinished file operation involves it";

/// What every item of a run shares.
struct Run<'a> {
    batch: &'a str,
    computer: &'a str,
    kind: Kind,
    folders: &'a Folders,
    trash: &'a dyn Trash,
    disk: &'a dyn Disk,
}

impl Run<'_> {
    /// Journals `item` as left unchanged, for `reason`.
    fn unchanged(&self, catalog: &mut Catalog, item: &Item, reason: &str) -> Result<()> {
        catalog.db.write(|w| {
            journal::insert(
                w,
                self.batch,
                self.computer,
                self.kind,
                ItemState::NoChange,
                &item.preconditions,
                &item.members,
                Some(reason),
            )
            .map(drop)
        })
    }

    /// Remove from Catalog: the check, the removal and the journal in one
    /// transaction.
    fn remove(&self, catalog: &mut Catalog, item: &Item) -> Result<()> {
        catalog.db.write_immediate(|w| {
            let mut reason = item.preconditions.check(w, self.computer)?;
            // An unfinished file operation of any computer that involves
            // these photos keeps them: it may still need their rows.
            let involved = journal::unresolved_anywhere(w)?.iter().any(|i| {
                i.photos()
                    .iter()
                    .any(|p| item.preconditions.rows.contains(p))
            });
            if reason.is_none() && involved {
                reason = Some(UNFINISHED);
            }
            if reason.is_none() {
                remove_rows(w, &item.preconditions.rows, Removal::Requested)?;
            }
            let state = match reason {
                None => ItemState::Done,
                Some(_) => ItemState::NoChange,
            };
            journal::insert(
                w,
                self.batch,
                self.computer,
                self.kind,
                state,
                &item.preconditions,
                &[],
                reason,
            )
            .map(drop)
        })
    }

    /// An item that moves or trashes files.
    fn files(&self, catalog: &mut Catalog, item: &Item) -> Result<()> {
        // a. Checked again and journalled, in one transaction.
        let started = catalog.db.write_immediate(|w| {
            let reason = match item.preconditions.check(w, self.computer)? {
                Some(reason) => Some(reason.to_string()),
                None => self.changed(w, item)?,
            };
            let state = match reason {
                None => ItemState::Planned,
                Some(_) => ItemState::NoChange,
            };
            let id = journal::insert(
                w,
                self.batch,
                self.computer,
                self.kind,
                state,
                &item.preconditions,
                &item.members,
                reason.as_deref(),
            )?;
            Ok(reason.is_none().then_some(id))
        })?;
        let Some(id) = started else {
            return Ok(());
        };
        // b. Each member's step, durable and journalled before the next. A
        // step that happened counts as changed from then on, whether or not
        // what follows it succeeds.
        let mut changed: Vec<Member> = Vec::new();
        for (ordinal, m) in item.members.iter().enumerate() {
            let (receipt, after) = match (self.kind, &m.destination) {
                (Kind::TrashPhotos, _) => match self.trash.trash(&m.source) {
                    Ok(receipt) => (Some(receipt.location.clone()), receipt.location),
                    Err(e) => return self.put_back(catalog, id, &changed, &e.to_string()),
                },
                (_, Some(to)) => match self.disk.rename(&m.source, to) {
                    Ok(()) => (None, to.clone()),
                    Err(e) => return self.put_back(catalog, id, &changed, &e.to_string()),
                },
                (_, None) => return self.put_back(catalog, id, &changed, "no destination"),
            };
            let state = match receipt {
                Some(_) => MemberState::Trashed,
                None => MemberState::Moved,
            };
            changed.push(Member {
                ordinal: ordinal as i64,
                source: m.source.clone(),
                destination: m.destination.clone(),
                id: Some(m.id),
                identity: Some(m.identity.clone()),
                state,
                receipt: receipt.clone(),
            });
            // The Trash keeps its own folders durable, and may not let them be
            // opened (macOS's needs Full Disk Access).
            let synced = match receipt {
                Some(_) => self.disk.sync_dir(parent(&m.source)),
                None => sync_parents(self.disk, &m.source, &after),
            };
            if let Err(e) = synced {
                return self.put_back(catalog, id, &changed, &e.to_string());
            }
            if let Err(e) = catalog
                .db
                .write(|w| journal::set_member(w, id, ordinal as i64, state, receipt.as_deref()))
            {
                return self.put_back(catalog, id, &changed, &format!("{e:#}"));
            }
        }
        // c. The catalog follows, on the evidence.
        let finished = catalog.db.write_immediate(|w| {
            let journalled = journal::item(w, id)?.context("the journalled item is gone")?;
            complete(w, self.computer, self.folders, self.disk, &journalled)
        });
        if let Err(e) = finished {
            let error = format!("{e:#}");
            catalog
                .db
                .write(|w| journal::set_state(w, id, ItemState::NeedsRecovery, Some(&error)))
                .map_err(|again| again.context(error))?;
        }
        Ok(())
    }

    /// Why `item`'s files are no longer as planned, read just before it is
    /// journalled: a file changed or gone, another photo over the same
    /// file, or the destination taken since.
    fn changed(&self, w: &Write<'_>, item: &Item) -> Result<Option<String>> {
        let pre = &item.preconditions;
        for m in &item.members {
            let same = match self.disk.entry(&m.source) {
                Ok(e) => e.id == m.id && e.kind == EntryKind::File && e.links == 1,
                Err(_) => false,
            };
            if !same || self.disk.identity(&m.source).ok().as_ref() != Some(&m.identity) {
                return Ok(Some(format!(
                    "{} changed since it was planned",
                    m.source.display()
                )));
            }
        }
        let Some((photo, companions)) = item.members.split_first() else {
            return Ok(None);
        };
        // Its folders are still plainly what was planned: each canonical
        // (no link put in on the way since) and on the file's volume (no
        // mount point).
        for m in &item.members {
            let dirs = [
                Some(parent(&m.source)),
                m.destination.as_deref().map(parent),
            ];
            for dir in dirs.into_iter().flatten() {
                let plain = self.disk.canonical(dir).is_ok_and(|c| c == dir)
                    && self
                        .disk
                        .entry(dir)
                        .is_ok_and(|e| e.id.volume() == m.id.volume());
                if !plain {
                    return Ok(Some(format!(
                        "{} changed since it was planned",
                        dir.display()
                    )));
                }
            }
        }
        let filename = photo.source.file_name().unwrap_or_default();
        if shared(
            w,
            self.folders,
            parent(&photo.source),
            filename,
            Some(photo.id),
            &pre.rows,
        )? {
            return Ok(Some(
                "another photo in the catalog is the same file now".into(),
            ));
        }
        // Its companions are still its alone.
        let planned: Vec<OsString> = companions
            .iter()
            .filter_map(|m| m.source.file_name().map(OsString::from))
            .collect();
        let names = self
            .disk
            .list(parent(&photo.source))
            .map_err(anyhow::Error::from)?;
        // Those it carries, as the plan chose them: a companion with another
        // name elsewhere stays.
        let dir = parent(&photo.source);
        let same_file = |a: &OsStr, b: &OsStr| match (
            self.disk.entry(&dir.join(a)),
            self.disk.entry(&dir.join(b)),
        ) {
            (Ok(a), Ok(b)) => a.id == b.id,
            _ => false,
        };
        let carried: Vec<OsString> = companions::of(filename, &names, &same_file)
            .carried
            .into_iter()
            .filter(|n| {
                self.disk
                    .entry(&dir.join(n))
                    .is_ok_and(|e| e.kind == EntryKind::File && e.links == 1)
            })
            .collect();
        if carried != planned {
            return Ok(Some("its XMP sidecars changed since it was planned".into()));
        }
        if let (Some((to, _)), Some(dir)) = (&pre.to, photo.destination.as_deref().map(parent)) {
            let names = self.disk.list(dir).map_err(anyhow::Error::from)?;
            let mut projected = names.clone();
            projected.push(filename.to_os_string());
            projected.extend(planned.iter().cloned());
            if let Some(other) = companions::arrival_conflict(filename, &planned, &projected) {
                return Ok(Some(format!(
                    "{} there would share an XMP sidecar with it now",
                    other.to_string_lossy()
                )));
            }
            for m in &item.members {
                let Some(name) = m.destination.as_deref().and_then(|d| d.file_name()) else {
                    continue;
                };
                let taken = names
                    .iter()
                    .any(|n| n == name || item.rules.key(n) == item.rules.key(name))
                    || !matches!(self.disk.entry(&dir.join(name)), Err(FsError::NotFound));
                if taken {
                    return Ok(Some(format!("{} is there now", name.to_string_lossy())));
                }
            }
            if occupied(w, self.folders, *to, dir, filename, &pre.rows)? {
                return Ok(Some(format!(
                    "a photo of the catalog is named {} there now",
                    filename.to_string_lossy()
                )));
            }
        }
        Ok(None)
    }

    /// After step `error` failed: puts the members already `changed` back
    /// where they were, the decision journalled first.
    fn put_back(
        &self,
        catalog: &mut Catalog,
        id: OperationId,
        changed: &[Member],
        error: &str,
    ) -> Result<()> {
        // Decided before any disk step: from here on it is never completed.
        catalog
            .db
            .write(|w| journal::set_state(w, id, ItemState::Restoring, Some(error)))?;
        // One that can't be put back stays being put back, for recovery.
        if restore(catalog, self.disk, id, changed)? {
            catalog
                .db
                .write(|w| journal::set_state(w, id, ItemState::NoChange, None))
        } else {
            catalog.db.write(|w| {
                journal::set_state(w, id, ItemState::Restoring, Some(super::recover::NOT_BACK))
            })
        }
    }
}

/// Renames each of `members` back from where its step put it to its
/// source, newest first, each journalled `restored`. Only a member proven
/// there (the same file object, by its exact name) is renamed. Returns
/// whether every one is back.
pub(super) fn restore(
    catalog: &mut Catalog,
    disk: &dyn Disk,
    id: OperationId,
    members: &[Member],
) -> Result<bool> {
    for m in members.iter().rev() {
        let (Some(at), Some(file)) = (m.after(), m.id) else {
            return Ok(false);
        };
        if presence(disk, at, file) != Presence::Here || disk.rename(at, &m.source).is_err() {
            return Ok(false);
        }
        let synced = match m.receipt {
            Some(_) => disk.sync_dir(parent(&m.source)),
            None => sync_parents(disk, at, &m.source),
        };
        if synced.is_err() {
            return Ok(false);
        }
        catalog
            .db
            .write(|w| journal::set_member(w, id, m.ordinal, MemberState::Restored, None))?;
    }
    Ok(true)
}

/// Proof that a trashed item's files are gone: each member has a Trash
/// receipt and its source no longer has it. Made only by [`Self::of`], and
/// the only thing that lets [`remove_rows`] remove rows for a Trash.
pub(super) struct DeletionEvidence(());

impl DeletionEvidence {
    pub fn of(disk: &dyn Disk, members: &[Member]) -> Option<Self> {
        let gone = members.iter().all(|m| {
            m.state == MemberState::Trashed
                && m.receipt.is_some()
                && m.id.is_some_and(|id| gone(disk, &m.source, id))
        });
        (gone && !members.is_empty()).then_some(Self(()))
    }
}

/// Why rows may leave the catalog.
pub(super) enum Removal {
    /// The user asked for Remove from Catalog.
    Requested,
    /// Their file went to the Trash.
    Trashed(DeletionEvidence),
}

/// Removes photos `rows` with everything of theirs. Takes the reason they
/// may go, so rows of a trashed photo can't go without [`DeletionEvidence`].
pub(super) fn remove_rows(w: &mut Write<'_>, rows: &[PhotoId], _why: Removal) -> Result<()> {
    for id in rows {
        super::super::copies::delete_photo(w, *id)?;
    }
    Ok(())
}

/// Finishes journalled `item` in the catalog, in `w`'s transaction, if the
/// evidence allows: its preconditions still hold, and its files are proven
/// where the rows will say (moved: each at its destination and gone from
/// its source; trashed: [`DeletionEvidence`]). Marks it `done`, or
/// `needs_recovery` with the reason it can't be.
pub(super) fn complete(
    w: &mut Write<'_>,
    computer: &str,
    folders: &Folders,
    disk: &dyn Disk,
    item: &Journalled,
) -> Result<()> {
    let pre = &item.preconditions;
    let shared_now = match item.members.first() {
        Some(photo) => shared(
            w,
            folders,
            parent(&photo.source),
            photo.source.file_name().unwrap_or_default(),
            photo.id,
            &pre.rows,
        )?,
        None => false,
    };
    let reason: Option<String> = match pre.check(w, computer)? {
        Some(reason) => Some(reason.into()),
        None if shared_now => Some("another photo in the catalog is the same file now".into()),
        None => match item.kind {
            Kind::RemoveFromCatalog => {
                remove_rows(w, &pre.rows, Removal::Requested)?;
                None
            }
            Kind::TrashPhotos => match DeletionEvidence::of(disk, &item.members) {
                Some(evidence) => {
                    remove_rows(w, &pre.rows, Removal::Trashed(evidence))?;
                    None
                }
                None => Some("it may be in the Trash".into()),
            },
            Kind::MovePhotos => moved(w, folders, disk, item)?,
        },
    };
    let state = match reason {
        None => ItemState::Done,
        Some(_) => ItemState::NeedsRecovery,
    };
    journal::set_state(w, item.id, state, reason.as_deref())
}

/// Moves a moved item's rows to its folder, when every member is proven at
/// its destination and gone from its source, and no other photo of the
/// catalog has taken the name there. Why not, otherwise.
fn moved(
    w: &mut Write<'_>,
    folders: &Folders,
    disk: &dyn Disk,
    item: &Journalled,
) -> Result<Option<String>> {
    let pre = &item.preconditions;
    let Some((to, _)) = pre.to else {
        return Ok(Some("it has no destination".into()));
    };
    let proven = item
        .members
        .iter()
        .all(|m| match (m.id, m.destination.as_deref()) {
            (Some(id), Some(at)) => {
                presence(disk, at, id) == Presence::Here
                    && presence(disk, &m.source, id) == Presence::Absent
            }
            _ => false,
        });
    if !proven {
        return Ok(Some("its files aren't all where they were moved".into()));
    }
    let Some(photo) = item.members.first().and_then(|m| m.destination.as_deref()) else {
        return Ok(Some("it has no files".into()));
    };
    let filename = photo.file_name().unwrap_or_default();
    if occupied(w, folders, to, parent(photo), filename, &pre.rows)? {
        return Ok(Some(format!(
            "a photo of the catalog is named {} there now",
            filename.to_string_lossy()
        )));
    }
    let name = filename.to_str().context("not a name the catalog keeps")?;
    w.execute(
        sql!("UPDATE photos SET folder=?, filename=? WHERE id=? OR master_id=?"),
        &[&to, &name, &pre.master, &pre.master],
    )?;
    Ok(None)
}
