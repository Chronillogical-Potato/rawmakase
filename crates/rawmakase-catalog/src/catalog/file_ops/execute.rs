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
//! A move to another volume copies instead of renaming in b, and puts the
//! originals in the Trash after c (see [`super::copy`]); anything failing
//! before c undoes it.
//!
//! A plan that makes a folder makes it first, as its own item (see
//! [`super::folder`]); its moves go on only once the catalog has it.
//!
//! Cancelling stops between items, and between the chunks of a copy, whose
//! item is then undone.
use super::super::db::{Write, sql};
use super::super::folder_sync::locate_folders;
use super::super::{Catalog, PhotoId};
use super::companions;
use super::disk::{Disk, Presence, Real, gone, parent, presence, spelled, sync_parents};
use super::folder::{self, Made, NewFolder};
use super::journal::{
    self, Item as Journalled, ItemState, Kind, Member, MemberState, OperationId, Subject,
};
use super::plan::{Folders, Item, occupied, shared};
use super::tree::TreeMove;
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
        Kind::MovePhotos
        | Kind::TrashPhotos
        | Kind::CreateFolder
        | Kind::RenamePhotos
        | Kind::RenameFolder
        | Kind::MoveFolder => {
            let lock = super::lock::acquire(catalog)?;
            super::recover::prune(catalog)?;
            let recovery = super::recover::run(catalog, disk, trash)?;
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
        // A new folder's photos move into it.
        kind: match plan.kind {
            Kind::CreateFolder => Kind::MovePhotos,
            kind => kind,
        },
        folders: &folders,
        trash,
        disk,
    };
    let mut stopped = None;
    let mut items = plan.items;
    if let Some(new) = &plan.create {
        match run.create(catalog, new, cancel) {
            Ok(Some(made)) => rebase(&mut items, &made),
            Ok(None) => items.clear(),
            Err(e) => {
                stopped = Some(format!("{e:#}"));
                items.clear();
            }
        }
    }
    if let Some(tree) = &plan.tree
        && let Err(e) = run.tree(catalog, tree, &busy, cancel)
    {
        stopped = Some(format!("{e:#}"));
    }
    for item in items {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let result = if item.preconditions.rows.iter().any(|r| busy.contains(r)) {
            run.unchanged(catalog, &item, UNFINISHED)
        } else {
            match run.kind {
                Kind::RemoveFromCatalog => run.remove(catalog, &item),
                Kind::MovePhotos | Kind::TrashPhotos | Kind::CreateFolder | Kind::RenamePhotos => {
                    run.files(catalog, &item, cancel)
                }
                // A folder rename or move has no photo items.
                Kind::RenameFolder | Kind::MoveFolder => Ok(()),
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
pub(super) const UNFINISHED: &str = "an unfinished file operation involves it";

/// Points `items`, planned into a folder not yet made, at folder `made`:
/// its id, and the name the disk gave it.
fn rebase(items: &mut [Item], made: &Made) {
    for item in items {
        item.preconditions.to = Some((made.folder, made.resolved.clone()));
        for m in &mut item.members {
            if let Some(name) = m.source.file_name() {
                m.destination = Some(made.dir.join(name));
            }
        }
        if let Some(copy) = &mut item.copy {
            for temp in &mut copy.temps {
                if let Some(name) = temp.file_name() {
                    *temp = made.dir.join(name);
                }
            }
        }
    }
}

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

    /// Makes folder `new`, journalled first, then adds it to the catalog
    /// once it is proven there. Returns it when the catalog has it; when it
    /// doesn't, the item says why, and nothing is to move.
    fn create(
        &self,
        catalog: &mut Catalog,
        new: &NewFolder,
        cancel: &AtomicBool,
    ) -> Result<Option<Made>> {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let dir = parent(&new.path);
        // a. Checked again and journalled, in one transaction: its parent
        // where it was, plainly, and its name free.
        let started = catalog.db.write_immediate(|w| {
            let plain = self.disk.canonical(dir).is_ok_and(|c| c == dir);
            let reason = match new.state.check(w, self.computer)? {
                Some(reason) => Some(reason.to_string()),
                None if !plain => Some(format!("{} changed since it was planned", dir.display())),
                None => folder::taken(w, self.computer, self.disk, new)?,
            };
            let state = match reason {
                None => ItemState::Planned,
                Some(_) => ItemState::NoChange,
            };
            let id = journal::insert_folder(
                w,
                self.batch,
                self.computer,
                state,
                &new.state,
                &new.path,
                reason.as_deref(),
            )?;
            Ok(reason.is_none().then_some(id))
        })?;
        let Some(id) = started else {
            return Ok(None);
        };
        let set = |catalog: &mut Catalog, state: ItemState, error: &str| {
            catalog
                .db
                .write(|w| journal::set_state(w, id, state, Some(error)))
        };
        // b. Made, durable, and which folder it is journalled. A failed
        // `mkdir` made nothing.
        if let Err(e) = self.disk.create_dir(&new.path) {
            set(catalog, ItemState::NoChange, &e.to_string())?;
            return Ok(None);
        }
        let found = self
            .disk
            .sync_dir(dir)
            .and_then(|()| folder::made(self.disk, &new.path));
        let journalled = found.map_err(|e| e.to_string()).and_then(|(file, at)| {
            catalog
                .db
                .write(|w| journal::set_created(w, id, file, &at))
                .map_err(|e| format!("{e:#}"))
        });
        if let Err(e) = journalled {
            set(catalog, ItemState::NeedsRecovery, &e)?;
            return Ok(None);
        }
        // c. The catalog's folder, on the evidence.
        let finished = catalog.db.write_immediate(|w| {
            let item = journal::item(w, id)?.context("the journalled item is gone")?;
            folder::complete(w, self.computer, self.disk, &item)
        });
        if let Err(e) = finished {
            set(catalog, ItemState::NeedsRecovery, &format!("{e:#}"))?;
            return Ok(None);
        }
        let item = journal::item(&catalog.db, id)?.context("the journalled item is gone")?;
        let (Subject::Folder(state), ItemState::Done) = (&item.subject, item.state) else {
            return Ok(None);
        };
        let made = (
            state.folder,
            item.members.first().and_then(|m| m.destination.clone()),
        );
        let (Some(folder), Some(dir)) = made else {
            return Ok(None);
        };
        let name = dir.file_name().and_then(OsStr::to_str).unwrap_or_default();
        Ok(Some(Made {
            folder,
            resolved: state.parent_path.join(name),
            dir,
        }))
    }

    /// Renames folder `tree`'s directory, journalled first, then re-keys
    /// the catalog's folders of its subtree once it is proven there. Left
    /// alone when a photo in it is in `busy`, an unfinished operation.
    fn tree(
        &self,
        catalog: &mut Catalog,
        tree: &TreeMove,
        busy: &HashSet<PhotoId>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let involved = tree.state.photos.iter().any(|p| busy.contains(p));
        // To another volume, its tree is copied.
        if let Some(copy) = &tree.copy {
            return super::tree_copy::run(
                &self.cx(),
                catalog,
                self.batch,
                self.kind,
                tree,
                copy,
                involved,
                cancel,
            );
        }
        // a. Checked again and journalled, in one transaction.
        let started = catalog.db.write_immediate(|w| {
            let reason = match tree.state.check(w, self.computer)? {
                Some(reason) => Some(reason),
                None if involved || super::tree::unfinished(w, tree)? => Some(UNFINISHED.into()),
                None => self.tree_changed(tree)?,
            };
            let state = match reason {
                None => ItemState::Planned,
                Some(_) => ItemState::NoChange,
            };
            let id = journal::insert_tree(
                w,
                self.batch,
                self.computer,
                self.kind,
                state,
                &tree.state,
                (&tree.source, &tree.destination, tree.id),
                reason.as_deref(),
            )?;
            Ok(reason.is_none().then_some(id))
        })?;
        let Some(id) = started else {
            return Ok(());
        };
        // b. The rename, durable, and the name the disk gave it journalled.
        let mut member = Member {
            ordinal: 0,
            source: tree.source.clone(),
            destination: Some(tree.destination.clone()),
            id: Some(tree.id),
            identity: None,
            state: MemberState::Moved,
            receipt: None,
            copy: None,
        };
        if let Err(e) = self.disk.rename(&tree.source, &tree.destination) {
            return self.put_back(catalog, id, &[], &e.to_string());
        }
        let at = sync_parents(self.disk, &tree.source, &tree.destination)
            .and_then(|()| spelled(self.disk, &tree.destination, tree.id));
        let at = match at {
            Ok(at) => at,
            Err(e) => return self.put_back(catalog, id, &[member], &e.to_string()),
        };
        member.destination = Some(at.clone());
        if let Err(e) = catalog.db.write(|w| journal::set_moved(w, id, 0, &at)) {
            return self.put_back(catalog, id, &[member], &format!("{e:#}"));
        }
        // c. The catalog follows, on the evidence.
        let finished = catalog.db.write_immediate(|w| {
            let journalled = journal::item(w, id)?.context("the journalled item is gone")?;
            super::tree::complete(w, self.computer, self.disk, &journalled)
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

    /// Why folder `tree`'s directory is no longer as planned: another file
    /// object, a link or mount point put in on the way, or its destination
    /// taken since.
    fn tree_changed(&self, tree: &TreeMove) -> Result<Option<String>> {
        let same = self
            .disk
            .entry(&tree.source)
            .is_ok_and(|e| e.id == tree.id && e.kind == EntryKind::Directory);
        if !same {
            return Ok(Some(format!(
                "{} changed since it was planned",
                tree.source.display()
            )));
        }
        for dir in [parent(&tree.source), parent(&tree.destination)] {
            let plain = self.disk.canonical(dir).is_ok_and(|c| c == dir)
                && self
                    .disk
                    .entry(dir)
                    .is_ok_and(|e| e.id.volume() == tree.id.volume());
            if !plain {
                return Ok(Some(format!(
                    "{} changed since it was planned",
                    dir.display()
                )));
            }
        }
        super::tree::taken(self.disk, tree)
    }

    /// What copying to another volume needs of the run.
    fn cx(&self) -> super::copy::Cx<'_> {
        super::copy::Cx {
            computer: self.computer,
            folders: self.folders,
            disk: self.disk,
            trash: self.trash,
        }
    }

    /// An item that moves or trashes files.
    fn files(&self, catalog: &mut Catalog, item: &Item, cancel: &AtomicBool) -> Result<()> {
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
            if let Some(copy) = &item.copy {
                journal::insert_copies(w, id, &copy.temps)?;
            }
            Ok(reason.is_none().then_some(id))
        })?;
        let Some(id) = started else {
            return Ok(());
        };
        if item.copy.is_some() {
            return super::copy::run(&self.cx(), catalog, item, id, cancel);
        }
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
                copy: None,
            });
            let mut after = after;
            // The Trash keeps its own folders durable, and may not let them be
            // opened (macOS's needs Full Disk Access).
            let synced = match receipt {
                Some(_) => self.disk.sync_dir(parent(&m.source)),
                None => sync_parents(self.disk, &m.source, &after),
            };
            if let Err(e) = synced {
                return self.put_back(catalog, id, &changed, &e.to_string());
            }
            // A name given by the user is journalled as the disk spells it,
            // which a volume that normalises names may not as given.
            if self.kind == Kind::RenamePhotos {
                match spelled(self.disk, &after, m.id) {
                    Ok(at) => after = at,
                    Err(e) => return self.put_back(catalog, id, &changed, &e.to_string()),
                }
                if let Some(last) = changed.last_mut() {
                    last.destination = Some(after.clone());
                }
            }
            let journalled = match self.kind {
                Kind::RenamePhotos => catalog
                    .db
                    .write(|w| journal::set_moved(w, id, ordinal as i64, &after)),
                _ => catalog.db.write(|w| {
                    journal::set_member(w, id, ordinal as i64, state, receipt.as_deref())
                }),
            };
            if let Err(e) = journalled {
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
        // mount point), or, copied, on the destination's.
        for m in &item.members {
            let there = item.copy.as_ref().map_or(m.id.volume(), |c| c.volume);
            let dirs = [
                Some((parent(&m.source), m.id.volume())),
                m.destination.as_deref().map(|d| (parent(d), there)),
            ];
            for (dir, volume) in dirs.into_iter().flatten() {
                let plain = self.disk.canonical(dir).is_ok_and(|c| c == dir)
                    && self.disk.entry(dir).is_ok_and(|e| e.id.volume() == volume);
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
        // Its companions are still its alone: those it carries, and those
        // a rename keeps as they are.
        let mut planned: Vec<OsString> = companions
            .iter()
            .map(|m| m.source.as_path())
            .chain(item.kept.iter().map(std::path::PathBuf::as_path))
            .filter_map(|p| p.file_name().map(OsString::from))
            .collect();
        planned.sort();
        // As the plan took them where the drive lists them decomposed
        // (#573).
        let usable = |dir: &std::path::Path, names: Vec<OsString>, by: super::proof::Proof| match by
        {
            super::proof::Proof::Names => names
                .iter()
                .map(|n| super::proof::usable(self.disk, dir, n))
                .collect(),
            super::proof::Proof::Ids => names,
        };
        let names = self
            .disk
            .list(parent(&photo.source))
            .map_err(anyhow::Error::from)?;
        let names = usable(parent(&photo.source), names, pre.proofs.originals);
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
        let mut carried: Vec<OsString> = companions::of(filename, &names, &same_file)
            .carried
            .into_iter()
            .filter(|n| {
                self.disk
                    .entry(&dir.join(n))
                    .is_ok_and(|e| e.kind == EntryKind::File && e.links == 1)
            })
            .collect();
        carried.sort();
        if carried != planned {
            return Ok(Some("its XMP sidecars changed since it was planned".into()));
        }
        let goes = pre.to.is_some() || pre.into.is_some();
        if let (true, Some(dir)) = (goes, photo.destination.as_deref().map(parent)) {
            let to = pre.to.as_ref().map(|(to, _)| *to);
            let names = self.disk.list(dir).map_err(anyhow::Error::from)?;
            let names = usable(dir, names, pre.proofs.copies);
            // A rename stays in its folder, where its own names are its own.
            let here = dir == parent(&photo.source);
            let own: Vec<&OsStr> = match here {
                true => item
                    .members
                    .iter()
                    .map(|m| m.source.as_path())
                    .chain(item.kept.iter().map(std::path::PathBuf::as_path))
                    .filter_map(std::path::Path::file_name)
                    .collect(),
                false => Vec::new(),
            };
            let arriving: Vec<OsString> = item
                .members
                .iter()
                .filter_map(|m| m.destination.as_deref()?.file_name().map(OsString::from))
                .collect();
            let Some((new, carried)) = arriving.split_first() else {
                return Ok(None);
            };
            // Companions a rename keeps as they are go with it too.
            let carried: Vec<OsString> = carried
                .iter()
                .cloned()
                .chain(
                    item.kept
                        .iter()
                        .filter_map(|k| k.file_name().map(OsString::from)),
                )
                .collect();
            let mut projected: Vec<OsString> = names
                .iter()
                .filter(|n| !own.contains(&n.as_os_str()))
                .cloned()
                .collect();
            projected.extend(arriving.iter().cloned());
            if let Some(other) = companions::arrival_conflict(new, &carried, &projected) {
                return Ok(Some(format!(
                    "{} there would share an XMP sidecar with it now",
                    other.to_string_lossy()
                )));
            }
            for m in &item.members {
                let Some(name) = m.destination.as_deref().and_then(|d| d.file_name()) else {
                    continue;
                };
                let mine = |n: &OsStr| here && m.source.file_name() == Some(n);
                let listed = names
                    .iter()
                    .any(|n| !mine(n) && (n == name || item.rules.key(n) == item.rules.key(name)));
                let found = match self.disk.entry(&dir.join(name)) {
                    Err(FsError::NotFound) => false,
                    Ok(e) => !(here && e.id == m.id),
                    Err(_) => true,
                };
                if listed || found {
                    return Ok(Some(format!("{} is there now", name.to_string_lossy())));
                }
            }
            if occupied(w, self.folders, to, dir, new, &pre.rows)? {
                return Ok(Some(format!(
                    "a photo of the catalog is named {} there now",
                    new.to_string_lossy()
                )));
            }
            // Its partial copies' names are free.
            for temp in item.copy.iter().flat_map(|c| &c.temps) {
                let listed = temp
                    .file_name()
                    .is_some_and(|t| names.iter().any(|n| n == t));
                if listed || !matches!(self.disk.entry(temp), Err(FsError::NotFound)) {
                    return Ok(Some(format!("{} is there now", temp.display())));
                }
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
    let pre = match &item.subject {
        Subject::Photos(pre) => pre,
        Subject::Folder(_) => return folder::complete(w, computer, disk, item),
        Subject::Tree(_) => return super::tree::complete(w, computer, disk, item),
    };
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
            Kind::MovePhotos | Kind::RenamePhotos => moved(w, computer, folders, disk, pre, item)?,
            Kind::CreateFolder | Kind::RenameFolder | Kind::MoveFolder => {
                Some("not a photo operation".into())
            }
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
    computer: &str,
    folders: &Folders,
    disk: &dyn Disk,
    pre: &super::preconditions::Preconditions,
    item: &Journalled,
) -> Result<Option<String>> {
    if pre.to.is_none() && pre.into.is_none() {
        return Ok(Some("it has no destination".into()));
    }
    let to = pre.to.as_ref().map(|(to, _)| *to);
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
    let to = match arrive(w, computer, item.id, pre, parent(photo))? {
        Ok(to) => to,
        Err(why) => return Ok(Some(why)),
    };
    w.execute(
        sql!("UPDATE photos SET folder=?, filename=? WHERE id=? OR master_id=?"),
        &[&to, &name, &pre.master, &pre.master],
    )?;
    Ok(None)
}

/// The catalog folder move item `id`'s rows go to, in `w`'s transaction:
/// the one planned, or, for a directory the catalog had no folder for, the
/// one it has there now or adds (recorded in the item's journalled
/// preconditions, with whether it added it). Called only once every other
/// check of the change passed, so a folder is never added for rows that
/// don't follow. Why not, otherwise.
pub(super) fn arrive(
    w: &mut Write<'_>,
    computer: &str,
    id: OperationId,
    pre: &super::preconditions::Preconditions,
    dir: &std::path::Path,
) -> Result<std::result::Result<crate::catalog::FolderId, String>> {
    if let Some((to, _)) = &pre.to {
        return Ok(Ok(*to));
    }
    let Some(into) = &pre.into else {
        return Ok(Err("it has no destination".into()));
    };
    let (to, added) = match folder::arrive(w, computer, into, dir)? {
        Ok(found) => found,
        Err(why) => return Ok(Err(why)),
    };
    let mut now = pre.clone();
    now.to = Some((to, into.resolved.clone()));
    if let Some(into) = &mut now.into {
        into.added = added;
    }
    journal::set_preconditions(w, id, &now)?;
    Ok(Ok(to))
}
