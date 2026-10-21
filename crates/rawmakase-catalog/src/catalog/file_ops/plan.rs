//! Planning: the user's choice expanded into items, each family resolved on
//! this computer and checked against the refusals and the conflict and
//! companion rules, from one snapshot of the catalog. Every file is
//! captured once here (its path, file object and `Identity`), and the same
//! values are what execution checks, journals and acts on. Nothing is
//! changed.
use super::super::db::{Reads, sql};
use super::super::folder_sync::Located;
use super::super::locations::{join, logical_anchored, logical_location};
use super::super::sidecar::caseless;
use super::super::value::row;
use super::super::{Catalog, FolderId, PhotoId, RootId};
use super::companions::{self, Companions};
use super::conflicts::{Destination, Names};
use super::folder::{FolderState, NewFolder, folder_name_problem};
use super::journal::{Kind, NewMember};
use super::preconditions::Preconditions;
use super::tree::TreeMove;
use super::{BatchId, FileOp, FileOpError, Plan, Refusal, Skip, Skipped, Summary};
use crate::storage::Identity;
use crate::storage::fsops::{self, Containment, EntryKind, FileId, FsError, VolumeId};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// One item of a plan: a family (or a lone virtual copy, for Remove from
/// Catalog) and the files it touches.
#[derive(Clone, Debug)]
pub(super) struct Item {
    pub preconditions: Preconditions,
    /// The photo's file first, then its companions; none when only rows
    /// change.
    pub members: Vec<NewMember>,
    /// Companions another file reads too, which stay.
    pub shared: Vec<PathBuf>,
    /// The photo's companions a rename leaves as they are: they have the
    /// name it gives them already.
    pub kept: Vec<PathBuf>,
    /// How the destination folder compares names.
    pub rules: Names,
}

row! {
    pub(super) struct PhotoRow {
        id: PhotoId,
        folder: FolderId,
        filename: String,
        master: Option<PhotoId>,
        edited: bool,
    }
}

/// A chosen family, before its files are looked at.
pub(super) struct Family {
    pub master: PhotoId,
    pub preconditions: Preconditions,
}

/// Plans `op` in `catalog` on this computer. Reads the catalog in one
/// snapshot and the files after it; changes nothing.
pub fn plan(catalog: &Catalog, op: FileOp) -> Result<Plan, FileOpError> {
    let mut stems = HashMap::new();
    let (kind, chosen, to, create) = match op {
        FileOp::RemoveFromCatalog { photos } => (Kind::RemoveFromCatalog, photos, None, None),
        FileOp::MovePhotos { photos, to } => (Kind::MovePhotos, photos, Some(to), None),
        FileOp::TrashPhotos { photos } => (Kind::TrashPhotos, photos, None, None),
        FileOp::CreateFolder {
            root,
            parent,
            name,
            then_move,
        } => (
            Kind::CreateFolder,
            then_move,
            None,
            Some((root, parent, name)),
        ),
        FileOp::RenamePhotos { renames } => {
            let mut ids = Vec::new();
            for (photo, stem) in renames {
                ids.push(photo);
                // The same photo given two names has neither.
                let entry = stems.entry(photo).or_insert_with(|| Some(stem.clone()));
                if entry.as_ref() != Some(&stem) {
                    *entry = None;
                }
            }
            (Kind::RenamePhotos, ids, None, None)
        }
        FileOp::RenameFolder { root, folder, name } => {
            let tree = super::tree::rename(catalog, root, &folder, &name)?;
            return tree_plan(catalog, Kind::RenameFolder, tree);
        }
        FileOp::MoveFolder {
            root,
            folder,
            parent,
        } => {
            let tree = super::tree::relocate(catalog, root, &folder, &parent)?;
            return tree_plan(catalog, Kind::MoveFolder, tree);
        }
    };
    let chosen: BTreeSet<PhotoId> = chosen.into_iter().collect();
    let mut skipped = Vec::new();
    let (photos, folders, families, computers) = catalog.db.snapshot(|s| {
        let photos: Vec<PhotoRow> = s.read(
            sql!(
                "SELECT id, folder, filename, master_id, recipe IS NOT NULL FROM photos ORDER BY id"
            ),
            &[],
        )?;
        let folders = catalog.locate_folders(s)?;
        let families = expand(s, kind, &chosen, &photos, &mut skipped)?;
        let computers: i64 = s.read_one(sql!("SELECT count(*) FROM computers"), &[])?;
        Ok((photos, folders, families, computers))
    })?;
    let by_id: HashMap<PhotoId, &PhotoRow> = photos.iter().map(|p| (p.id, p)).collect();
    let mut made = None;
    let items = match kind {
        Kind::RemoveFromCatalog => families
            .into_iter()
            .map(|f| Item {
                preconditions: f.preconditions,
                members: Vec::new(),
                shared: Vec::new(),
                kept: Vec::new(),
                rules: UNKNOWN,
            })
            .collect(),
        Kind::MovePhotos | Kind::TrashPhotos | Kind::CreateFolder => {
            let mut files = Files::new(catalog, &folders);
            let destination = match (to, create) {
                (Some(to), _) => Some(files.destination(to)?),
                (None, Some((root, parent, name))) => {
                    let (new, target) = files.new_folder(root, &parent, &name)?;
                    made = Some(new);
                    Some(target)
                }
                (None, None) => None,
            };
            let mut items = Vec::new();
            for f in families {
                match files.family(f, &by_id, destination.as_ref()) {
                    Ok(item) => items.push(item),
                    Err((photo, reason)) => skipped.push(Skipped { photo, reason }),
                }
            }
            refuse_twice(&mut items, &mut skipped);
            if let Some(destination) = &destination {
                arrivals(destination, &mut items, &mut skipped);
            }
            items
        }
        Kind::RenamePhotos => {
            let mut files = Files::new(catalog, &folders);
            super::rename::items(&mut files, families, &by_id, &stems, &mut skipped)?
        }
        Kind::RenameFolder | Kind::MoveFolder => Vec::new(),
    };
    let mut summary = summarize(&items, &by_id, computers > 1);
    if kind == Kind::RenamePhotos {
        summary.renames = items.iter().filter_map(super::rename::names).collect();
    }
    summary.folder = made.as_ref().map(|m| m.state.name.clone());
    skipped.sort_by_key(|s| s.photo);
    Ok(Plan {
        batch: new_batch()?,
        location: catalog.location.clone(),
        computer: catalog.computer.id.clone(),
        kind,
        create: made,
        tree: None,
        items,
        skipped,
        summary,
    })
}

/// The plan of folder change `tree`, of `kind`.
fn tree_plan(catalog: &Catalog, kind: Kind, tree: TreeMove) -> Result<Plan, FileOpError> {
    let computers: i64 = catalog
        .db
        .read_one(sql!("SELECT count(*) FROM computers"), &[])?;
    let summary = Summary {
        photos: tree.state.photos.len(),
        other_computers: computers > 1,
        folder: tree
            .destination
            .file_name()
            .map(|n| n.to_string_lossy().into_owned()),
        ..Summary::default()
    };
    Ok(Plan {
        batch: new_batch()?,
        location: catalog.location.clone(),
        computer: catalog.computer.id.clone(),
        kind,
        create: None,
        tree: Some(tree),
        items: Vec::new(),
        skipped: Vec::new(),
        summary,
    })
}

/// A run's id: random, so two runs never share one.
fn new_batch() -> anyhow::Result<BatchId> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(BatchId(bytes.iter().map(|b| format!("{b:02x}")).collect()))
}

/// The families `chosen` stands for, as `db` has them: a master with every
/// copy of it; a copy whose master isn't chosen alone, for Remove from
/// Catalog, and left out (it has no file of its own) otherwise.
fn expand(
    db: &impl Reads,
    kind: Kind,
    chosen: &BTreeSet<PhotoId>,
    photos: &[PhotoRow],
    skipped: &mut Vec<Skipped>,
) -> anyhow::Result<Vec<Family>> {
    let by_id: HashMap<PhotoId, &PhotoRow> = photos.iter().map(|p| (p.id, p)).collect();
    let mut copies: HashMap<PhotoId, Vec<PhotoId>> = HashMap::new();
    for p in photos {
        if let Some(master) = p.master {
            copies.entry(master).or_default().push(p.id);
        }
    }
    // Each master's family, or the copies of it chosen without it.
    let mut groups: Vec<(PhotoId, Vec<PhotoId>)> = Vec::new();
    for id in chosen {
        let Some(photo) = by_id.get(id) else {
            skipped.push(Skipped {
                photo: *id,
                reason: Skip::Unknown,
            });
            continue;
        };
        match photo.master {
            None => {
                let mut rows = vec![*id];
                rows.extend(copies.get(id).into_iter().flatten());
                rows.sort();
                groups.push((*id, rows));
            }
            // Its master's family takes it.
            Some(master) if chosen.contains(&master) => {}
            Some(master) if kind == Kind::RemoveFromCatalog => {
                match groups.iter_mut().find(|(m, _)| *m == master) {
                    Some((_, rows)) => rows.push(*id),
                    None => groups.push((master, vec![*id])),
                }
            }
            Some(_) => skipped.push(Skipped {
                photo: *id,
                reason: Skip::CopyOnly,
            }),
        }
    }
    let mut families = Vec::new();
    for (master, rows) in groups {
        let first = rows.first().copied().unwrap_or(master);
        match Preconditions::read(db, master, rows)? {
            Some(preconditions) => families.push(Family {
                master,
                preconditions,
            }),
            None => skipped.push(Skipped {
                photo: first,
                reason: Skip::Unknown,
            }),
        }
    }
    Ok(families)
}

/// Whether `path` is somewhere no operation touches: in a Trash, or in
/// RAWmakase's own folders.
pub(super) fn protected(path: &Path) -> bool {
    let trash = path.components().any(|c| {
        let name = c.as_os_str().to_string_lossy().to_lowercase();
        name == ".trash"
            || name == ".trashes"
            || name == "$recycle.bin"
            || name.starts_with(".trash-")
    });
    let own = [crate::storage::data_dir(), crate::storage::local_data_dir()]
        .into_iter()
        .filter_map(|d| std::fs::canonicalize(d).ok())
        .any(|d| path.starts_with(d));
    let freedesktop = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .and_then(|d| std::fs::canonicalize(d.join("Trash")).ok())
        .is_some_and(|t| path.starts_with(t));
    trash || own || freedesktop
}

/// Catalog folders by the directory they resolve to on this computer: one
/// index per plan or run, not a scan per photo. Two roots or folders over
/// one directory resolve to the same directory here.
pub(super) struct Folders {
    by_dir: HashMap<FileId, Vec<FolderId>>,
    /// The file object at each path looked up, read once per plan or run
    /// (so before anything of it moves): a volume may take two different
    /// names (`ß.NEF`, `SS.NEF`) for one file, which no folding tells.
    files: std::cell::RefCell<HashMap<PathBuf, Option<FileId>>>,
}

impl Folders {
    pub fn index(folders: &HashMap<FolderId, Located>) -> Self {
        let mut by_dir: HashMap<FileId, Vec<FolderId>> = HashMap::new();
        for (id, f) in folders {
            let found = f
                .path
                .as_deref()
                .and_then(|p| std::fs::canonicalize(p).ok())
                .and_then(|p| fsops::entry(&p).ok());
            if let Some(e) = found {
                by_dir.entry(e.id).or_default().push(*id);
            }
        }
        Self {
            by_dir,
            files: Default::default(),
        }
    }

    /// The file object at `path`, as first read in this plan or run.
    fn file_at(&self, path: &Path) -> Option<FileId> {
        *self
            .files
            .borrow_mut()
            .entry(path.to_path_buf())
            .or_insert_with(|| fsops::entry(path).ok().map(|e| e.id))
    }

    /// The catalog folders over directory `dir`.
    pub fn over(&self, dir: &Path) -> Vec<FolderId> {
        fsops::entry(dir)
            .ok()
            .and_then(|e| self.by_dir.get(&e.id).cloned())
            .unwrap_or_default()
    }
}

/// Whether a master outside `family`, in a catalog folder over directory
/// `dir`, is file object `file` there (or, when it can't be told, is named
/// `filename`, by `sidecar::caseless`): the same file through another photo.
pub(super) fn shared(
    db: &impl Reads,
    folders: &Folders,
    dir: &Path,
    filename: &OsStr,
    file: Option<FileId>,
    family: &[PhotoId],
) -> anyhow::Result<bool> {
    row! {
        struct Named {
            id: PhotoId,
            filename: String,
        }
    }
    let name = caseless(filename);
    for folder in folders.over(dir) {
        let masters: Vec<Named> = db.read(
            sql!("SELECT id, filename FROM photos WHERE folder=? AND master_id IS NULL"),
            &[&folder],
        )?;
        let same = |p: &Named| {
            caseless(OsStr::new(&p.filename)) == name
                || file.is_some_and(|f| folders.file_at(&dir.join(&p.filename)) == Some(f))
        };
        if masters.iter().any(|p| !family.contains(&p.id) && same(p)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether a photo outside `family`, in catalog folder `to` or another over
/// its directory `dir`, is named `filename` (by `sidecar::caseless`), offline or not.
pub(super) fn occupied(
    db: &impl Reads,
    folders: &Folders,
    to: FolderId,
    dir: &Path,
    filename: &OsStr,
    family: &[PhotoId],
) -> anyhow::Result<bool> {
    row! {
        struct Named {
            id: PhotoId,
            filename: String,
        }
    }
    let name = caseless(filename);
    let mut over = folders.over(dir);
    if !over.contains(&to) {
        over.push(to);
    }
    for folder in over {
        let photos: Vec<Named> = db.read(
            sql!("SELECT id, filename FROM photos WHERE folder=?"),
            &[&folder],
        )?;
        if photos
            .iter()
            .any(|p| !family.contains(&p.id) && caseless(OsStr::new(&p.filename)) == name)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Where the files go, checked once.
pub(super) struct Target {
    /// The catalog's folder; `None` for one the plan makes, or one the
    /// catalog has no folder of its own for.
    folder: Option<FolderId>,
    /// Where the folder resolves on this computer, as the catalog places it.
    resolved: PathBuf,
    /// Its canonical path, which members' destinations are in.
    dir: PathBuf,
    volume: VolumeId,
    names: Vec<OsString>,
    rules: Names,
    /// Filenames of the catalog's photos in it, through any folder over the
    /// same directory, by `sidecar::caseless`.
    catalog: HashSet<String>,
}

/// The files side of planning: what is on disk, read once per folder and
/// volume.
pub(super) struct Files<'a> {
    pub catalog: &'a Catalog,
    located: &'a HashMap<FolderId, Located>,
    pub folders: Folders,
    listings: HashMap<PathBuf, Result<Vec<OsString>, String>>,
    volumes: HashMap<VolumeId, Option<fsops::VolumeFacts>>,
}

impl<'a> Files<'a> {
    fn new(catalog: &'a Catalog, located: &'a HashMap<FolderId, Located>) -> Self {
        Self {
            catalog,
            located,
            folders: Folders::index(located),
            listings: HashMap::new(),
            volumes: HashMap::new(),
        }
    }

    pub fn listing(&mut self, dir: &Path) -> Result<Vec<OsString>, String> {
        self.listings
            .entry(dir.to_path_buf())
            .or_insert_with(|| fsops::list(dir).map_err(|e| e.to_string()))
            .clone()
    }

    /// The facts of `volume`, which `path` is on; `None` when they can't be
    /// read.
    fn volume(&mut self, volume: VolumeId, path: &Path) -> Option<fsops::VolumeFacts> {
        *self
            .volumes
            .entry(volume)
            .or_insert_with(|| fsops::volume_facts(path).ok())
    }

    fn read_only(&mut self, volume: VolumeId, path: &Path) -> bool {
        self.volume(volume, path).is_none_or(|v| v.read_only)
    }

    /// Whether a rename on `volume`, which `path` is on, can change only a
    /// name's spelling.
    pub fn renames_spelling(&mut self, volume: VolumeId, path: &Path) -> bool {
        self.volume(volume, path)
            .is_some_and(|v| v.renames_spelling)
    }

    /// Folder `to`, checked to take files.
    fn destination(&mut self, to: FolderId) -> Result<Target, FileOpError> {
        let refuse = |why: &str| FileOpError::Destination(why.to_string());
        let folder = self
            .located
            .get(&to)
            .ok_or_else(|| refuse("The folder is no longer in the catalog."))?;
        let (Some(path), Some(anchor)) = (folder.path.clone(), folder.anchor.clone()) else {
            return Err(refuse("The folder is offline or missing."));
        };
        self.destination_at(Some(to), &path, &anchor)
    }

    /// Folder `name` to make below logical folder `parent` of `root`,
    /// checked, and it as where files go.
    fn new_folder(
        &mut self,
        root: RootId,
        parent: &str,
        name: &str,
    ) -> Result<(NewFolder, Target), FileOpError> {
        let refuse = |why: &str| FileOpError::Destination(why.to_string());
        if let Some(problem) = folder_name_problem(name) {
            return Err(refuse(problem));
        }
        let computer = &self.catalog.computer.id;
        let (anchor, path) = logical_anchored(&self.catalog.db, computer, root, parent)?
            .ok_or_else(|| refuse("The folder is offline or missing."))?;
        // The catalog's own folder there, if it has one.
        let own = self
            .located
            .iter()
            .find(|(_, f)| f.root == root && f.logical == parent)
            .map(|(id, _)| *id);
        let at = self.destination_at(own, &path, &anchor)?;
        let new = NewFolder {
            state: FolderState {
                root,
                parent: parent.to_string(),
                parent_path: path.clone(),
                name: name.to_string(),
                folder: None,
            },
            path: at.dir.join(name),
            rules: at.rules,
        };
        let disk = super::disk::Real;
        let free = super::folder::taken(&self.catalog.db, computer, &disk, &new)
            .map_err(FileOpError::Catalog)?;
        if let Some(why) = free {
            return Err(FileOpError::Destination(why));
        }
        let resolved = path.join(name);
        let placed = logical_location(&self.catalog.db, computer, root, &join(parent, name))?;
        if placed.as_ref() != Some(&resolved) {
            return Err(refuse(
                "The catalog places a folder of that name elsewhere.",
            ));
        }
        let target = Target {
            folder: None,
            resolved,
            dir: new.path.clone(),
            volume: at.volume,
            names: Vec::new(),
            rules: at.rules,
            catalog: HashSet::new(),
        };
        Ok((new, target))
    }

    /// The folder at `path`, resolved below `anchor`, checked to take files.
    fn destination_at(
        &mut self,
        to: Option<FolderId>,
        path: &Path,
        anchor: &Path,
    ) -> Result<Target, FileOpError> {
        let refuse = |why: &str| FileOpError::Destination(why.to_string());
        let dir = match fsops::canonical_below(anchor, path) {
            Ok(dir) => dir,
            Err(Containment::Fs(FsError::NotFound)) => {
                return Err(refuse("The folder is offline or missing."));
            }
            Err(Containment::Symlink(p) | Containment::MountPoint(p)) => {
                return Err(FileOpError::Destination(format!(
                    "{} is a link or another volume inside its root folder.",
                    p.display()
                )));
            }
            Err(Containment::Outside) => {
                return Err(refuse("The folder is not inside its root folder."));
            }
            Err(Containment::Fs(e)) => return Err(FileOpError::Destination(e.to_string())),
        };
        // As the disk spells it, which members' destinations are journalled in.
        let dir =
            std::fs::canonicalize(&dir).map_err(|e| FileOpError::Destination(e.to_string()))?;
        let found = fsops::entry(&dir).map_err(|e| FileOpError::Destination(e.to_string()))?;
        if found.kind != EntryKind::Directory {
            return Err(refuse("The folder is not a folder on disk."));
        }
        if protected(&dir) {
            return Err(refuse(
                "Files can't be moved into the Trash or RAWmakase's own folders.",
            ));
        }
        if self.read_only(found.id.volume(), &dir) {
            return Err(refuse("The folder's volume is read-only."));
        }
        let names = self.listing(&dir).map_err(FileOpError::Destination)?;
        let mut over = self.folders.over(&dir);
        over.extend(to);
        let mut catalog = HashSet::new();
        for folder in over {
            let filenames: Vec<String> = self.catalog.db.read(
                sql!("SELECT filename FROM photos WHERE folder=?"),
                &[&folder],
            )?;
            catalog.extend(filenames.iter().map(|f| caseless(OsStr::new(f))));
        }
        Ok(Target {
            folder: to,
            resolved: path.to_path_buf(),
            rules: Names(fsops::probe(&dir, &names)),
            dir,
            volume: found.id.volume(),
            names,
            catalog,
        })
    }

    /// Family `f`'s item: its file and companions resolved and checked, or
    /// why it is left out.
    pub fn family(
        &mut self,
        f: Family,
        by_id: &HashMap<PhotoId, &PhotoRow>,
        target: Option<&Target>,
    ) -> Result<Item, (PhotoId, Skip)> {
        let skip = |reason| (f.master, reason);
        let refuse = |r| (f.master, Skip::Refused(r));
        let master = by_id.get(&f.master).ok_or_else(|| skip(Skip::Unknown))?;
        let folder = self
            .located
            .get(&master.folder)
            .ok_or_else(|| skip(Skip::Unknown))?;
        let (Some(path), Some(anchor)) = (folder.path.clone(), folder.anchor.clone()) else {
            return Err(skip(Skip::Missing));
        };
        let source = path.join(&master.filename);
        let file = match fsops::canonical_below(&anchor, &source) {
            Ok(file) => file,
            Err(Containment::Fs(FsError::NotFound)) => return Err(skip(Skip::Missing)),
            Err(Containment::Symlink(p) | Containment::MountPoint(p)) => {
                return Err(refuse(Refusal::NotPlain(p)));
            }
            Err(Containment::Outside) => return Err(refuse(Refusal::NotPlain(source))),
            Err(Containment::Fs(e)) => return Err(refuse(Refusal::Unreadable(e.to_string()))),
        };
        let found = fsops::entry(&file).map_err(|e| refuse(Refusal::Unreadable(e.to_string())))?;
        if found.kind != EntryKind::File {
            return Err(refuse(Refusal::Unreadable("it is not a file".into())));
        }
        // Another name of the same file object.
        if found.links != 1 {
            return Err(refuse(Refusal::Shared));
        }
        // The folder and the name as the disk spells them, which the
        // catalog may not (a volume that ignores case finds either): what
        // is journalled is matched byte for byte, so it must be the disk's.
        let dir = std::fs::canonicalize(super::disk::parent(&file))
            .map_err(|e| refuse(Refusal::Unreadable(e.to_string())))?;
        let names = self
            .listing(&dir)
            .map_err(|e| refuse(Refusal::Unreadable(e)))?;
        let wanted = caseless(file.file_name().unwrap_or_default());
        let mut spelled = names.iter().filter(|n| {
            caseless(n) == wanted && fsops::entry(&dir.join(n)).is_ok_and(|e| e.id == found.id)
        });
        let (Some(filename), None) = (spelled.next().cloned(), spelled.next()) else {
            return Err(refuse(Refusal::Unreadable(
                "its name in its folder can't be told".into(),
            )));
        };
        let file = dir.join(&filename);
        let rows = &f.preconditions.rows;
        let shared = shared(
            &self.catalog.db,
            &self.folders,
            &dir,
            &filename,
            Some(found.id),
            rows,
        )
        .map_err(|e| refuse(Refusal::Unreadable(format!("{e:#}"))))?;
        if shared {
            return Err(refuse(Refusal::Shared));
        }
        if protected(&file) {
            return Err(refuse(Refusal::Protected));
        }
        if let Some(target) = target {
            if dir == target.dir {
                return Err(skip(Skip::AlreadyThere));
            }
            if found.id.volume() != target.volume {
                return Err(refuse(Refusal::OtherVolume));
            }
        }
        if self.read_only(found.id.volume(), &dir) {
            return Err(refuse(Refusal::ReadOnly));
        }
        let identity =
            Identity::read(&file).map_err(|e| refuse(Refusal::Unreadable(format!("{e:#}"))))?;
        let same_file =
            |a: &OsStr, b: &OsStr| match (fsops::entry(&dir.join(a)), fsops::entry(&dir.join(b))) {
                (Ok(a), Ok(b)) => a.id == b.id,
                _ => false,
            };
        let Companions { carried, shared } = companions::of(&filename, &names, &same_file);
        let mut members = vec![NewMember {
            destination: target.map(|t| t.dir.join(&filename)),
            source: file,
            id: found.id,
            identity,
        }];
        let mut stays: Vec<PathBuf> = shared.iter().map(|n| dir.join(n)).collect();
        for name in carried {
            let path = dir.join(&name);
            let e = fsops::entry(&path).map_err(|e| refuse(Refusal::Unreadable(e.to_string())))?;
            // A companion with another name elsewhere stays, as one another
            // file reads does.
            if e.kind != EntryKind::File || e.links != 1 {
                stays.push(path);
                continue;
            }
            let identity =
                Identity::read(&path).map_err(|e| refuse(Refusal::Unreadable(format!("{e:#}"))))?;
            members.push(NewMember {
                destination: target.map(|t| t.dir.join(&name)),
                source: path,
                id: e.id,
                identity,
            });
        }
        let mut preconditions = f.preconditions;
        preconditions.folder_path = Some(path);
        // A folder the plan makes has no id yet: execution gives it.
        preconditions.to = target.and_then(|t| Some((t.folder?, t.resolved.clone())));
        Ok(Item {
            preconditions,
            members,
            shared: stays,
            kept: Vec::new(),
            rules: target.map_or(UNKNOWN, |t| t.rules),
        })
    }
}

/// Names compared as loosely as any folder could: for a plan with no
/// destination.
const UNKNOWN: Names = Names(fsops::NameRules {
    case: fsops::Folding::Unknown,
    normalization: fsops::Folding::Unknown,
});

/// Refuses the items whose files can't arrive at `target`: a name taken
/// there, a name two of them would have, or a companion one of them brings
/// that another file there would read (or one there its photo would).
fn arrivals(target: &Target, items: &mut Vec<Item>, skipped: &mut Vec<Skipped>) {
    let dest = Destination {
        dir: &target.dir,
        names: &target.names,
        rules: target.rules,
        catalog: &target.catalog,
    };
    let names_of = |item: &Item| -> Vec<OsString> {
        item.members
            .iter()
            .filter_map(|m| m.source.file_name().map(OsString::from))
            .collect()
    };
    let mut uses: HashMap<String, usize> = HashMap::new();
    for item in items.iter() {
        for name in names_of(item) {
            *uses.entry(target.rules.key(&name)).or_default() += 1;
        }
    }
    let projected: Vec<OsString> = target
        .names
        .iter()
        .cloned()
        .chain(items.iter().flat_map(names_of))
        .collect();
    let disk = super::disk::Real;
    let text = |n: &OsString| n.to_string_lossy().into_owned();
    items.retain(|item| {
        let names = names_of(item);
        let Some((photo, carried)) = names.split_first() else {
            return true;
        };
        let refusal = names
            .iter()
            .find(|n| dest.taken(&disk, n).is_some())
            .map(|n| Refusal::NameTaken(text(n)))
            .or_else(|| {
                names
                    .iter()
                    .find(|n| uses.get(&target.rules.key(n)).copied().unwrap_or(0) > 1)
                    .map(|n| Refusal::NameTwice(text(n)))
            })
            .or_else(|| {
                companions::arrival_conflict(photo, carried, &projected)
                    .map(|n| Refusal::CompanionClash(text(&n)))
            });
        match refusal {
            None => true,
            Some(r) => {
                skipped.push(Skipped {
                    photo: item.preconditions.master,
                    reason: Skip::Refused(r),
                });
                false
            }
        }
    });
}

/// Refuses every item one of whose files another item of the plan also
/// takes: the same file object chosen twice, through two photos.
pub(super) fn refuse_twice(items: &mut Vec<Item>, skipped: &mut Vec<Skipped>) {
    let mut count: HashMap<FileId, usize> = HashMap::new();
    for m in items.iter().flat_map(|i| &i.members) {
        *count.entry(m.id).or_default() += 1;
    }
    items.retain(|item| {
        let twice = item
            .members
            .iter()
            .any(|m| count.get(&m.id).is_some_and(|n| *n > 1));
        if twice {
            skipped.push(Skipped {
                photo: item.preconditions.master,
                reason: Skip::Refused(Refusal::Twice),
            });
        }
        !twice
    });
}

/// What the confirmation counts.
fn summarize(
    items: &[Item],
    by_id: &HashMap<PhotoId, &PhotoRow>,
    other_computers: bool,
) -> Summary {
    let mut s = Summary {
        other_computers,
        ..Summary::default()
    };
    for item in items {
        let rows = &item.preconditions.rows;
        s.photos += rows.len();
        s.copies += rows
            .iter()
            .filter(|r| by_id.get(r).is_some_and(|p| p.master.is_some()))
            .count();
        s.edited += rows
            .iter()
            .filter(|r| by_id.get(r).is_some_and(|p| p.edited))
            .count();
        if let Some(photo) = by_id.get(&item.preconditions.master) {
            s.names.push(photo.filename.clone());
        }
        if !item.members.is_empty() {
            s.files += 1;
            s.companions += item.members.len() - 1;
        }
        s.shared_companions.extend(item.shared.iter().cloned());
    }
    s
}
