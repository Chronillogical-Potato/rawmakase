//! Renaming a folder (Rename Folder…) or moving it into another folder of
//! its root or of another root on its drive (Move Folder…, #572): the same
//! change with another destination. The folder is a logical path of its
//! root: a catalog folder, or one the catalog has folders below but none
//! of its own for. One exclusive rename of its directory on its volume,
//! and the catalog's folders of the whole subtree re-keyed to the new
//! logical path (and root), `folder_paths` and the legacy
//! `folders.relative_path` alike. Photos keep their rows: their folders
//! keep their ids, so their paths follow.
//!
//! Kept to the simple case: the subtree is resolved through a location
//! above it, so no `folder_locations` row (on any computer) nor legacy
//! `folder_mappings` row needs to change; a subtree with a location of its
//! own is refused, as is a root itself. Folders are never merged: a
//! destination the disk or the catalog has (a missing catalog folder too)
//! is refused. Nothing else of the catalog may resolve into the subtree,
//! such as another root over a folder in it, nor may the catalog itself or
//! RAWmakase's own folders be in it.
use super::super::db::{Reads, Write, sql};
use super::super::locations::{join, logical_anchored, logical_location, names};
use super::super::sidecar::caseless;
use super::super::value::row;
use super::super::{Catalog, CatalogLocation, FolderId, PhotoId, RootId};
use super::FileOpError;
use super::conflicts::Names;
use super::disk::{Disk, Presence, parent, presence};
use super::folder::{folder_name_problem, in_catalog};
use super::journal::{self, Item, ItemState, Subject};
use super::plan::protected;
use super::preconditions::{CHANGED, RELOCATED};
use crate::storage::fsops::{self, Containment, EntryKind, FileId, FsError};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// A folder item's journalled state: the subtree as planned, where it goes,
/// and what must still hold of the catalog for it to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::catalog) struct TreeState {
    pub root: RootId,
    /// Its logical path, and the one it is given (in [`Self::to_root`]).
    pub from: String,
    pub to: String,
    /// The root it goes into, when another (#572): absent from items of a
    /// move within a root, as 0.4.2 journalled them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub into: Option<RootId>,
    /// The catalog's folders of the subtree, the folder and every one
    /// below it, by logical path.
    pub folders: Vec<(FolderId, String)>,
    /// The photos in them, whose paths change with them.
    pub photos: Vec<PhotoId>,
    /// Where `from` and `to` resolve on this computer.
    pub from_path: PathBuf,
    pub to_path: PathBuf,
}

/// A folder change planned: its state, and its directory as captured.
#[derive(Clone, Debug)]
pub(super) struct TreeMove {
    pub state: TreeState,
    /// Its directory, canonical and as the disk spells it, and where it goes.
    pub source: PathBuf,
    pub destination: PathBuf,
    pub id: FileId,
    /// How the destination's folder compares names.
    pub rules: Names,
    /// Whether its volume renames a name to another spelling of itself.
    pub spelling: bool,
}

/// Why a root can't be renamed or moved here.
const ROOT: &str = "A root folder can't be renamed or moved in RAWmakase. Do it in Finder, then locate the root folder again.";
/// Why a subtree with a location of its own is refused.
const LOCATED: &str = "A folder in it is located on its own (on this or another computer), which RAWmakase can't move with it yet.";
/// Why a folder with something of the catalog in it through another root
/// is refused.
const OVERLAP: &str = "Another folder or root of the catalog is inside it on disk.";
/// Why a folder an unfinished file operation involves is refused.
const UNFINISHED: &str = "An unfinished file operation involves it.";

/// The catalog's folders of `root` at logical path `logical` or below it,
/// exactly.
fn subtree(db: &impl Reads, root: RootId, logical: &str) -> Result<Vec<(FolderId, String)>> {
    let mut found: Vec<(FolderId, String)> = locate_folders_of(db, root)?
        .into_iter()
        .filter(|(_, l)| below(l, logical))
        .collect();
    found.sort();
    Ok(found)
}

/// Every catalog folder of `root`, by logical path.
fn locate_folders_of(db: &impl Reads, root: RootId) -> Result<Vec<(FolderId, String)>> {
    row! {
        struct Folder {
            id: FolderId,
            relative: String,
            logical: Option<String>,
            original: String,
        }
    }
    let rows: Vec<Folder> = db.read(
        sql!(
            "SELECT f.id, f.relative_path, p.path, r.original_path
             FROM folders f JOIN roots r ON r.id=f.root
             LEFT JOIN folder_paths p ON p.folder=f.id WHERE f.root=?"
        ),
        &[&root],
    )?;
    Ok(rows
        .into_iter()
        .map(|f| {
            let logical = f.logical.unwrap_or_else(|| {
                super::super::locations::logical_from_legacy(&f.original, &f.relative)
            });
            (f.id, logical)
        })
        .collect())
}

/// Whether logical path `path` is `top` or below it, exactly.
fn below(path: &str, top: &str) -> bool {
    path == top || path.starts_with(&format!("{top}/"))
}

/// Whether logical path `path` is `top` or below it, by `sidecar::caseless`.
fn below_caseless(path: &str, top: &str) -> bool {
    below(&caseless(OsStr::new(path)), &caseless(OsStr::new(top)))
}

/// Whether `path` is `dir` or inside it, by `sidecar::caseless`.
fn inside(path: &Path, dir: &Path) -> bool {
    let (p, d) = (caseless(path.as_os_str()), caseless(dir.as_os_str()));
    Path::new(&p).starts_with(Path::new(&d))
}

impl TreeState {
    /// The root it goes into: its own, or the other it is moved into.
    pub fn to_root(&self) -> RootId {
        self.into.unwrap_or(self.root)
    }

    /// Why the folder can't be renamed now, read in `db` on `computer`: its
    /// subtree changed in the catalog, it or its destination resolves
    /// elsewhere, a folder in it has a location of its own, or the catalog
    /// has a folder where it goes. `None` when everything holds.
    pub fn check(&self, db: &impl Reads, computer: &str) -> Result<Option<String>> {
        if subtree(db, self.root, &self.from)? != self.folders {
            return Ok(Some(CHANGED.into()));
        }
        let from = logical_location(db, computer, self.root, &self.from)?;
        let to = logical_location(db, computer, self.to_root(), &self.to)?;
        if from.as_ref() != Some(&self.from_path) || to.as_ref() != Some(&self.to_path) {
            return Ok(Some(RELOCATED.into()));
        }
        let ids: Vec<FolderId> = self.folders.iter().map(|(id, _)| *id).collect();
        if located(db, self.root, &self.from, &ids)? {
            return Ok(Some(LOCATED.into()));
        }
        // Checked again before the rename and in recovery: another root may
        // have been added inside since.
        if let Some(why) = overlapped(db, computer, &ids, &[&self.from_path])? {
            return Ok(Some(why.into()));
        }
        if located(db, self.to_root(), &self.to, &[])? {
            return Ok(Some(
                "The catalog locates a folder of that name elsewhere.".into(),
            ));
        }
        let there: Vec<_> = in_catalog(db, computer, self.to_root(), &self.to, &self.to_path)?
            .into_iter()
            .filter(|(id, _)| !ids.contains(id))
            .collect();
        if !there.is_empty() {
            let name = names(&self.to).last().unwrap_or_default();
            return Ok(Some(format!(
                "The catalog has a folder named {name} there, though it may be missing."
            )));
        }
        Ok(None)
    }
}

/// Whether a folder of `root` at logical path `logical` or below it has a
/// location of its own on any computer, or (one of `folders`) a legacy
/// mapping.
fn located(db: &impl Reads, root: RootId, logical: &str, folders: &[FolderId]) -> Result<bool> {
    row! {
        struct Location {
            relative: String,
        }
    }
    let rows: Vec<Location> = db.read(
        sql!("SELECT relative_path FROM folder_locations WHERE root=?"),
        &[&root],
    )?;
    if rows.iter().any(|l| below_caseless(&l.relative, logical)) {
        return Ok(true);
    }
    for folder in folders {
        let mapped: Option<i64> = db.read_optional(
            sql!("SELECT 1 FROM folder_mappings WHERE folder=?"),
            &[folder],
        )?;
        if mapped.is_some() {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Plans renaming the folder at logical path `folder` of `root` to
/// `name`, on this computer.
pub(super) fn rename(
    catalog: &Catalog,
    root: RootId,
    folder: &str,
    name: &str,
) -> Result<TreeMove, FileOpError> {
    let refuse = |why: &str| FileOpError::Destination(why.to_string());
    if let Some(problem) = folder_name_problem(name) {
        return Err(refuse(problem));
    }
    known(catalog, root, folder)?;
    let up: Vec<&str> = names(folder).collect();
    let parent = up
        .split_last()
        .map(|(_, up)| up.join("/"))
        .unwrap_or_default();
    let to = join(&parent, name);
    if to == folder {
        return Err(refuse("It has that name already."));
    }
    plan_to(catalog, root, folder, root, &to)
}

/// Refuses the folder at logical path `folder` of `root` unless the
/// catalog has a folder there or below it (`2024` above `2024/day1` is
/// one, though it has no folder of its own), and it isn't a root.
fn known(catalog: &Catalog, root: RootId, folder: &str) -> Result<(), FileOpError> {
    let refuse = |why: &str| FileOpError::Destination(why.to_string());
    if folder.is_empty() {
        return Err(refuse(ROOT));
    }
    let located = catalog.db.snapshot(|s| catalog.locate_folders(s))?;
    if !located
        .values()
        .any(|f| f.root == root && below(&f.logical, folder))
    {
        return Err(refuse("The folder is no longer in the catalog."));
    }
    Ok(())
}

/// Plans moving the folder at logical path `folder` of `root` into
/// logical folder `parent` of `to_root` (the same root or another), on
/// this computer. The destination may be a folder the catalog has no
/// folder of its own for, as for New Folder Inside.
pub(super) fn relocate(
    catalog: &Catalog,
    root: RootId,
    folder: &str,
    to_root: RootId,
    parent: &str,
) -> Result<TreeMove, FileOpError> {
    let refuse = |why: &str| FileOpError::Destination(why.to_string());
    known(catalog, root, folder)?;
    // Within a root by its logical paths; into another, on disk below.
    let same = to_root == root;
    if same && below_caseless(parent, folder) {
        return Err(refuse(
            "A folder can't be moved into itself or a folder inside it.",
        ));
    }
    let name = names(folder).last().unwrap_or_default();
    let to = join(parent, name);
    if same && caseless(OsStr::new(&to)) == caseless(OsStr::new(folder)) {
        return Err(refuse("It is in that folder already."));
    }
    plan_to(catalog, root, folder, to_root, &to)
}

/// Plans moving the folder at logical path `from` of `root` to logical
/// path `to` of `to_root`: the folder's own root, or another, which must
/// resolve and be online here.
fn plan_to(
    catalog: &Catalog,
    root: RootId,
    from: &str,
    to_root: RootId,
    to: &str,
) -> Result<TreeMove, FileOpError> {
    let refuse = |why: &str| FileOpError::Destination(why.to_string());
    let computer = &catalog.computer.id;
    let db = &catalog.db;
    let from = from.to_string();
    let (anchor, from_path) = logical_anchored(db, computer, root, &from)?
        .ok_or_else(|| refuse("The folder is offline or missing."))?;
    // Its directory, as the disk spells it.
    let source = plain(&anchor, &from_path)?;
    let found = fsops::entry(&source).map_err(|e| refuse(&e.to_string()))?;
    if found.kind != EntryKind::Directory {
        return Err(refuse("The folder is not a folder on disk."));
    }
    let source = super::disk::spelled(&super::disk::Real, &source, found.id)
        .map_err(|e| refuse(&e.to_string()))?;
    // Where it goes: below its new parent, as the catalog places it.
    let mut up: Vec<&str> = names(to).collect();
    let name = up.pop().context("no folder name")?.to_string();
    let to_parent = up.join("/");
    let (parent_anchor, parent_path) = logical_anchored(db, computer, to_root, &to_parent)?
        .ok_or_else(|| refuse("The destination is offline or missing."))?;
    let dir = plain(&parent_anchor, &parent_path)?;
    let at = fsops::entry(&dir).map_err(|e| refuse(&e.to_string()))?;
    if at.kind != EntryKind::Directory {
        return Err(refuse("The destination is not a folder on disk."));
    }
    if dir.starts_with(&source) {
        return Err(refuse(
            "A folder can't be moved into itself or a folder inside it.",
        ));
    }
    if at.id.volume() != found.id.volume() {
        return Err(refuse(
            "Moving between drives isn't available yet. Move it in Finder, then locate the folder again.",
        ));
    }
    if protected(&source) || protected(&dir) || holds_own(catalog, &source) {
        return Err(refuse(
            "Folders in the Trash, or holding the catalog or RAWmakase's own folders, can't be renamed or moved.",
        ));
    }
    let facts = fsops::volume_facts(&dir).map_err(|e| refuse(&e.to_string()))?;
    if facts.read_only {
        return Err(refuse("The folder's volume is read-only."));
    }
    let to_path = parent_path.join(&name);
    if logical_location(db, computer, to_root, to)?.as_ref() != Some(&to_path) {
        return Err(refuse(
            "The catalog places a folder of that name elsewhere.",
        ));
    }
    let listing = fsops::list(&dir).map_err(|e| refuse(&e.to_string()))?;
    let planned = TreeMove {
        state: TreeState {
            root,
            from: from.clone(),
            to: to.to_string(),
            into: (to_root != root).then_some(to_root),
            folders: subtree(db, root, &from)?,
            photos: Vec::new(),
            from_path,
            to_path,
        },
        destination: dir.join(&name),
        source,
        id: found.id,
        rules: Names(fsops::probe(&dir, &listing)),
        spelling: facts.renames_spelling,
    };
    // Another root over its own directory, or one nested in its root, can
    // name where it is already by another logical path.
    if to_root != root
        && caseless(planned.destination.as_os_str()) == caseless(planned.source.as_os_str())
    {
        return Err(refuse("It is in that folder already."));
    }
    if let Some(why) = taken(&super::disk::Real, &planned)? {
        return Err(FileOpError::Destination(why));
    }
    if let Some(why) = planned.state.check(db, computer)? {
        return Err(FileOpError::Destination(why));
    }
    let mut planned = planned;
    planned.state.photos = photos_in(db, &planned.state.folders)?;
    if unfinished(db, &planned)? {
        return Err(refuse(UNFINISHED));
    }
    Ok(planned)
}

/// `path`, below `anchor` with no link or mount point between, canonical.
fn plain(anchor: &Path, path: &Path) -> Result<PathBuf, FileOpError> {
    let refuse = |why: String| FileOpError::Destination(why);
    let dir = match fsops::canonical_below(anchor, path) {
        Ok(dir) => dir,
        Err(Containment::Fs(FsError::NotFound)) => {
            return Err(refuse("The folder is offline or missing.".into()));
        }
        Err(Containment::Symlink(p) | Containment::MountPoint(p)) => {
            return Err(refuse(format!(
                "{} is a link or another volume inside its root folder.",
                p.display()
            )));
        }
        Err(Containment::Outside) => {
            return Err(refuse("The folder is not inside its root folder.".into()));
        }
        Err(Containment::Fs(e)) => return Err(refuse(e.to_string())),
    };
    std::fs::canonicalize(&dir).map_err(|e| refuse(e.to_string()))
}

/// Whether the catalog's own files are in `dir`.
fn holds_own(catalog: &Catalog, dir: &Path) -> bool {
    let CatalogLocation::File(file) = catalog.location();
    std::fs::canonicalize(file).is_ok_and(|f| f.starts_with(dir))
        || [crate::storage::data_dir(), crate::storage::local_data_dir()]
            .into_iter()
            .filter_map(|d| std::fs::canonicalize(d).ok())
            .any(|d| d.starts_with(dir))
}

/// Why `planned`'s destination is taken on `disk`: a file or folder there
/// has its name, as the folder compares names or as a lookup finds it,
/// other than the folder itself (a new spelling of its own name, which
/// only some volumes give). `None` when it is free.
pub(super) fn taken(disk: &dyn Disk, planned: &TreeMove) -> Result<Option<String>> {
    let (dir, name) = (
        parent(&planned.destination),
        planned.destination.file_name().unwrap_or_default(),
    );
    let own = |n: &OsStr| parent(&planned.source) == dir && planned.source.file_name() == Some(n);
    let key = planned.rules.key(name);
    let listed = disk
        .list(dir)?
        .into_iter()
        .any(|n| !own(&n) && (n == name || planned.rules.key(&n) == key));
    let found = match disk.entry(&planned.destination) {
        Err(FsError::NotFound) => false,
        Ok(e) => !(e.id == planned.id && own(planned.source.file_name().unwrap_or_default())),
        Err(_) => true,
    };
    let text = name.to_string_lossy();
    if listed || found {
        return Ok(Some(format!(
            "A file or folder named {text} is already there."
        )));
    }
    let alike = own(planned.source.file_name().unwrap_or_default())
        && (planned
            .rules
            .key(planned.source.file_name().unwrap_or_default())
            == key
            || matches!(disk.entry(&planned.destination), Ok(e) if e.id == planned.id));
    if alike && !planned.spelling {
        return Ok(Some(
            "Its volume can't change only the case or accents of a name. Give it another name first."
                .into(),
        ));
    }
    Ok(None)
}

/// Why another root or folder of the catalog, outside the subtree
/// (`ours`), being inside the folder on disk refuses it: inside any of
/// `dirs`, as resolved or as the disk has them. (Where it goes is taken
/// by any folder there, which refuses it anyway.)
fn overlapped(
    db: &impl Reads,
    computer: &str,
    ours: &[FolderId],
    dirs: &[&Path],
) -> Result<Option<&'static str>> {
    row! {
        struct RootRow {
            id: RootId,
        }
    }
    let canonical: Vec<PathBuf> = dirs
        .iter()
        .filter_map(|d| std::fs::canonicalize(d).ok())
        .collect();
    let into = |p: &Path| {
        dirs.iter().any(|d| inside(p, d))
            || canonical.iter().any(|c| inside(p, c))
            || std::fs::canonicalize(p).is_ok_and(|cp| canonical.iter().any(|c| cp.starts_with(c)))
    };
    for (id, f) in super::super::folder_sync::locate_folders(db, computer)? {
        if !ours.contains(&id) && f.path.as_deref().is_some_and(into) {
            return Ok(Some(OVERLAP));
        }
    }
    let roots: Vec<RootRow> = db.read(sql!("SELECT id FROM roots"), &[])?;
    for root in roots {
        if let Some(at) = logical_location(db, computer, root.id, "")?
            && into(&at)
        {
            return Ok(Some(OVERLAP));
        }
    }
    Ok(None)
}

/// The photos in `folders`.
fn photos_in(db: &impl Reads, folders: &[(FolderId, String)]) -> Result<Vec<PhotoId>> {
    let mut photos = Vec::new();
    for (folder, _) in folders {
        let ids: Vec<PhotoId> = db.read(sql!("SELECT id FROM photos WHERE folder=?"), &[folder])?;
        photos.extend(ids);
    }
    photos.sort();
    Ok(photos)
}

/// Whether an unfinished file operation of any computer involves the
/// subtree: one of its photos, or a file or folder in it.
pub(super) fn unfinished(db: &impl Reads, planned: &TreeMove) -> Result<bool> {
    let items = journal::unresolved_anywhere(db)?;
    Ok(items.iter().any(|i| {
        i.photos().iter().any(|p| planned.state.photos.contains(p))
            || i.members.iter().any(|m| {
                [
                    Some(m.source.as_path()),
                    m.destination.as_deref(),
                    m.receipt.as_deref(),
                ]
                .into_iter()
                .flatten()
                .any(|p| inside(p, &planned.source) || inside(p, &planned.destination))
            })
    }))
}

/// Re-keys the subtree of `state` to logical path `to` of its destination
/// root in `w`'s transaction: each folder's root and logical path, and the
/// form releases before logical paths read. Each folder keeps its id.
fn rekey(w: &mut Write<'_>, state: &TreeState, to: &str) -> Result<()> {
    for (folder, logical) in &state.folders {
        let rest = logical
            .strip_prefix(state.from.as_str())
            .context("a folder outside the subtree")?;
        let new = format!("{to}{rest}");
        let native: PathBuf = names(&new).collect();
        w.execute(
            sql!(
                "INSERT INTO folder_paths(folder, path) VALUES (?, ?)
                 ON CONFLICT(folder) DO UPDATE SET path=excluded.path"
            ),
            &[folder, &new],
        )?;
        w.execute(
            sql!("UPDATE folders SET root=?, relative_path=? WHERE id=?"),
            &[&state.to_root(), &native.to_string_lossy(), folder],
        )?;
    }
    Ok(())
}

/// Re-keys the subtree of journalled folder `item` in `w`'s transaction, if
/// the evidence allows: its catalog state still holds, and its directory
/// is proven at its destination (the same file object, by the name the
/// disk gave it) and gone from its source. Marks it `done`, or
/// `needs_recovery` with the reason it can't be.
pub(super) fn complete(
    w: &mut Write<'_>,
    computer: &str,
    disk: &dyn Disk,
    item: &Item,
) -> Result<()> {
    let Subject::Tree(state) = &item.subject else {
        anyhow::bail!("not a folder rename");
    };
    let member = item
        .members
        .first()
        .context("the folder operation has no folder")?;
    let reason = match state.check(w, computer)? {
        Some(reason) => Some(reason),
        None => match (member.id, member.destination.as_deref()) {
            (Some(id), Some(at))
                if presence(disk, at, id) == Presence::Here
                    && presence(disk, &member.source, id) == Presence::Absent =>
            {
                // The new name as the disk spells it.
                let name = at
                    .file_name()
                    .and_then(OsStr::to_str)
                    .context("not a name the catalog keeps")?;
                let mut up: Vec<&str> = names(&state.to).collect();
                up.pop();
                rekey(w, state, &join(&up.join("/"), name))?;
                None
            }
            _ => Some("the folder isn't where it was moved".into()),
        },
    };
    let state = match reason {
        None => ItemState::Done,
        Some(_) => ItemState::NeedsRecovery,
    };
    journal::set_state(w, item.id, state, reason.as_deref())
}
