//! Renaming photos (Rename Photo…, and Rename Photos… in capture order,
//! #342): a new stem for each master's file, the extension kept, in its own
//! folder, its companions renamed to match. A rename is all or none when it
//! is planned: a name that can't be given, or is taken (on disk as the
//! folder compares names, by any catalog photo there, offline or not, or
//! twice in the rename), refuses the whole rename, as does a name another
//! photo of the rename has now (no name is freed and taken in one rename,
//! so no rename ever needs a temporary name). A name that differs only in
//! case or accents is given only on a volume whose rename does so in place
//! (APFS). Each photo then runs on its own, journalled like a move within
//! its folder.
//!
//! The order a batch numbers photos in is read once, when its dialog opens
//! ([`rename_order`]): by capture time, the catalog's where it has one,
//! else read from the file and kept nowhere; photos without one follow by
//! filename. [`rename_preview`] names them in that order and plans it.
use super::super::db::{Reads, sql};
use super::super::value::row;
use super::super::{Catalog, FolderId, PhotoId};
use super::companions;
use super::conflicts::Names;
use super::disk::parent;
use super::names::{FileName, Stem};
use super::plan::{Family, Files, Item, PhotoRow, refuse_twice};
use super::{FileOp, FileOpError, Plan, Refusal, Skip, Skipped};
use crate::export_settings::{NameContext, NameFields, NameTemplate};
use crate::storage::fsops::{self, FileId, FsError};
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// The rename items of `families`, each master to its stem in `stems`
/// (`None` when it was given two): resolved, renamed, and checked together.
/// If any photo is left out but for already having its name, every item
/// is, as blocked.
pub(super) fn items(
    files: &mut Files<'_>,
    families: Vec<Family>,
    by_id: &HashMap<PhotoId, &PhotoRow>,
    stems: &HashMap<PhotoId, Option<String>>,
    skipped: &mut Vec<Skipped>,
) -> Result<Vec<Item>, FileOpError> {
    let mut items = Vec::new();
    for f in families {
        let master = f.master;
        let refuse = |r| Skipped {
            photo: master,
            reason: Skip::Refused(r),
        };
        let stem = match stems.get(&master).cloned().flatten() {
            Some(text) => Stem::parse(&text),
            None => {
                skipped.push(refuse(Refusal::Twice));
                continue;
            }
        };
        let stem = match stem {
            Ok(stem) => stem,
            Err(problem) => {
                skipped.push(refuse(Refusal::BadName(problem)));
                continue;
            }
        };
        let item = match files.family(f, by_id, None) {
            Ok(item) => item,
            Err((photo, reason)) => {
                skipped.push(Skipped { photo, reason });
                continue;
            }
        };
        match renamed(files, item, &stem) {
            Ok(item) => items.push(item),
            Err(reason) => skipped.push(Skipped {
                photo: master,
                reason,
            }),
        }
    }
    refuse_twice(&mut items, skipped);
    conflicts(files, &mut items, skipped)?;
    if skipped.iter().any(|s| s.reason != Skip::SameName) {
        skipped.extend(items.drain(..).map(|i| Skipped {
            photo: i.preconditions.master,
            reason: Skip::Blocked,
        }));
    }
    Ok(items)
}

/// `item`, resolved in its folder, given `stem`: each member's destination
/// its new name there.
fn renamed(files: &mut Files<'_>, mut item: Item, stem: &Stem) -> Result<Item, Skip> {
    let refuse = |r| Err(Skip::Refused(r));
    let Some(photo) = item.members.first() else {
        return Err(Skip::Missing);
    };
    let (dir, id) = (parent(&photo.source).to_path_buf(), photo.id);
    let old = photo.source.file_name().unwrap_or_default().to_os_string();
    let Some(text) = old.to_str() else {
        return refuse(Refusal::Unreadable("its name isn't text".into()));
    };
    let new = match FileName::with_stem(text, stem) {
        Ok(new) => OsString::from(new.as_str()),
        Err(problem) => return refuse(Refusal::BadName(problem)),
    };
    if new == old {
        return Err(Skip::SameName);
    }
    let listing = files
        .listing(&dir)
        .map_err(|e| Skip::Refused(Refusal::Unreadable(e)))?;
    let rules = Names(fsops::probe(&dir, &listing));
    let spelling = files.renames_spelling(id.volume(), &dir);
    // A companion already named so (`a.xmp` for `A.NEF` → `a.NEF`) stays
    // as it is, and out of the rename: only what changes is renamed.
    let mut stays = Vec::new();
    for (i, m) in item.members.iter_mut().enumerate() {
        let from = m.source.file_name().unwrap_or_default();
        let to = match i {
            0 => new.clone(),
            _ => match companions::renamed(&old, &new, from) {
                Some(to) => to,
                None => return refuse(Refusal::CompanionName(from.to_string_lossy().into())),
            },
        };
        // Only another spelling of its own name, which few volumes change.
        let alike = rules.key(&to) == rules.key(from)
            || fsops::entry(&dir.join(&to)).is_ok_and(|e| e.id == m.id);
        if i > 0 && to == from {
            stays.push(i);
            continue;
        }
        if to != from && alike && !spelling {
            return refuse(Refusal::SpellingOnly);
        }
        m.destination = Some(dir.join(to));
    }
    for i in stays.into_iter().rev() {
        let kept = item.members.remove(i);
        item.kept.push(kept.source);
    }
    item.preconditions.to = item
        .preconditions
        .folder_path
        .clone()
        .map(|path| (item.preconditions.folder, path));
    item.rules = rules;
    Ok(item)
}

/// The folder an item's photo is in.
fn dir_of(item: &Item) -> Option<&Path> {
    item.members.first().map(|m| parent(&m.source))
}

/// The members' names, by `sidecar::caseless`.
fn key(name: &OsStr) -> String {
    super::super::sidecar::caseless(name)
}

/// What one folder of a rename holds, read once.
struct Folder {
    listing: Vec<OsString>,
    /// The catalog's photos there, through any folder over it: id and
    /// filename by `sidecar::caseless`.
    catalog: Vec<(PhotoId, String)>,
}

impl Folder {
    /// Directory `dir`, which catalog folder `own` is over.
    fn read(files: &mut Files<'_>, dir: &Path, own: FolderId) -> Result<Self, FileOpError> {
        row! {
            struct Named {
                id: PhotoId,
                filename: String,
            }
        }
        let listing = files.listing(dir).map_err(FileOpError::Destination)?;
        let mut over = files.folders.over(dir);
        over.push(own);
        let mut catalog = Vec::new();
        for folder in over {
            let named: Vec<Named> = files.catalog.db.read(
                sql!("SELECT id, filename FROM photos WHERE folder=?"),
                &[&folder],
            )?;
            catalog.extend(
                named
                    .into_iter()
                    .map(|p| (p.id, key(OsStr::new(&p.filename)))),
            );
        }
        Ok(Self { listing, catalog })
    }
}

/// Refuses the items whose new names aren't free: taken on disk or in the
/// catalog, given twice, another item's name now, or arriving with a
/// companion another file there would read (or reading one already there).
fn conflicts(
    files: &mut Files<'_>,
    items: &mut Vec<Item>,
    skipped: &mut Vec<Skipped>,
) -> Result<(), FileOpError> {
    // Every name the rename frees and takes, folder by folder.
    let mut sources: HashSet<(PathBuf, String)> = HashSet::new();
    let mut taken: HashMap<(PathBuf, String), usize> = HashMap::new();
    let mut ids: HashSet<FileId> = HashSet::new();
    let mut rows: HashSet<PhotoId> = HashSet::new();
    let mut folders: HashMap<PathBuf, Folder> = HashMap::new();
    for item in items.iter() {
        rows.extend(item.preconditions.rows.iter().copied());
        for m in &item.members {
            let dir = parent(&m.source).to_path_buf();
            let from = key(m.source.file_name().unwrap_or_default());
            sources.insert((dir.clone(), from));
            if let Some(to) = m.destination.as_deref().and_then(Path::file_name) {
                *taken.entry((dir.clone(), key(to))).or_default() += 1;
            }
            ids.insert(m.id);
        }
        let Some(dir) = dir_of(item).map(Path::to_path_buf) else {
            continue;
        };
        if let std::collections::hash_map::Entry::Vacant(e) = folders.entry(dir) {
            let read = Folder::read(files, e.key(), item.preconditions.folder)?;
            e.insert(read);
        }
    }
    let mut refused = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let Some(dir) = dir_of(item) else {
            continue;
        };
        let Some(folder) = folders.get(dir) else {
            continue;
        };
        let arriving: Vec<OsString> = items
            .iter()
            .filter(|other| dir_of(other) == Some(dir))
            .flat_map(|other| &other.members)
            .filter_map(|m| m.destination.as_deref()?.file_name().map(OsString::from))
            .collect();
        if let Some(r) = refusal(item, folder, &sources, &taken, &ids, &rows, &arriving) {
            refused.push((i, r));
        }
    }
    for (i, r) in refused.into_iter().rev() {
        let item = items.remove(i);
        skipped.push(Skipped {
            photo: item.preconditions.master,
            reason: Skip::Refused(r),
        });
    }
    Ok(())
}

/// Why `item` of a rename can't take its new names in `folder`, which
/// `arriving` (every new name there, its own too) arrive in; `None` when
/// they are free. A name another member of the rename has now is a chain.
fn refusal(
    item: &Item,
    folder: &Folder,
    sources: &HashSet<(PathBuf, String)>,
    taken: &HashMap<(PathBuf, String), usize>,
    ids: &HashSet<FileId>,
    rows: &HashSet<PhotoId>,
    arriving: &[OsString],
) -> Option<Refusal> {
    let text = |n: &OsStr| n.to_string_lossy().into_owned();
    let family = &item.preconditions.rows;
    for m in &item.members {
        let to = m.destination.as_deref()?;
        let (dir, name) = (parent(to), to.file_name()?);
        let own = m.source.file_name()?;
        if taken
            .get(&(dir.to_path_buf(), key(name)))
            .copied()
            .unwrap_or(0)
            > 1
        {
            return Some(Refusal::NameTwice(text(name)));
        }
        // On disk: as the folder compares names, then as a lookup finds them.
        let alike = folder.listing.iter().find(|n| {
            n.as_os_str() != own
                && (n.as_os_str() == name || item.rules.key(n) == item.rules.key(name))
        });
        if let Some(n) = alike {
            return Some(match sources.contains(&(dir.to_path_buf(), key(n))) {
                true => Refusal::Chain(text(name)),
                false => Refusal::NameTaken(text(name)),
            });
        }
        match fsops::entry(to) {
            Err(FsError::NotFound) => {}
            Ok(e) if e.id == m.id => {}
            Ok(e) if ids.contains(&e.id) => return Some(Refusal::Chain(text(name))),
            _ => return Some(Refusal::NameTaken(text(name))),
        }
        // In the catalog, offline photos too.
        let named = folder
            .catalog
            .iter()
            .filter(|(id, n)| *n == key(name) && !family.contains(id));
        let mut chain = false;
        for (id, _) in named {
            if !rows.contains(id) {
                return Some(Refusal::NameTaken(text(name)));
            }
            chain = true;
        }
        if chain {
            return Some(Refusal::Chain(text(name)));
        }
    }
    let names: Vec<OsString> = item
        .members
        .iter()
        .filter_map(|m| m.destination.as_deref()?.file_name().map(OsString::from))
        .collect();
    let (photo, carried) = names.split_first()?;
    // Companions kept as they are go with the photo too.
    let carried: Vec<OsString> = carried
        .iter()
        .cloned()
        .chain(
            item.kept
                .iter()
                .filter_map(|k| k.file_name().map(OsString::from)),
        )
        .collect();
    let own: Vec<&OsStr> = item
        .members
        .iter()
        .map(|m| m.source.as_path())
        .chain(item.kept.iter().map(PathBuf::as_path))
        .filter_map(Path::file_name)
        .collect();
    let projected: Vec<OsString> = folder
        .listing
        .iter()
        .filter(|n| !own.contains(&n.as_os_str()))
        .chain(arriving)
        .cloned()
        .collect();
    companions::arrival_conflict(photo, &carried, &projected)
        .map(|n| Refusal::CompanionClash(text(&n)))
}

/// A rename item's photo's name and the name it is given.
pub(super) fn names(item: &Item) -> Option<(String, String)> {
    let photo = item.members.first()?;
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().into_owned());
    Some((name(&photo.source)?, name(photo.destination.as_deref()?)?))
}

/// A photo in a batch rename's order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ordered {
    pub photo: PhotoId,
    /// Its file's name, as the catalog has it.
    pub filename: String,
    /// When it was taken, as Lightroom keeps it; `None` when that isn't
    /// known.
    pub captured: Option<String>,
}

/// The photos of a batch rename in the order it numbers them, read once
/// when its dialog opens: nothing reorders it after.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RenameOrder(Vec<Ordered>);

impl RenameOrder {
    pub fn photos(&self) -> &[Ordered] {
        &self.0
    }
}

/// `photos`, given in the Library's order, in the order a batch rename
/// numbers them: by capture time (the catalog's, else read from the file by
/// `capture_time` and kept nowhere), photos taken at one time in the order
/// given; then those whose time isn't known, by filename. A virtual copy
/// goes with its master when that is given too. Stops between files once
/// `cancel` is set. `capture_time` is the app's reader, which dates the RAW
/// containers the catalog can't read (a CR3, CRW, MRW or X3F) as the
/// capture-time backfill does.
pub fn rename_order(
    catalog: &Catalog,
    photos: &[PhotoId],
    cancel: &AtomicBool,
    capture_time: fn(&Path) -> Option<String>,
) -> Result<RenameOrder, FileOpError> {
    row! {
        struct Dated {
            id: PhotoId,
            folder: super::super::FolderId,
            filename: String,
            master: Option<PhotoId>,
            captured: String,
        }
    }
    let (rows, folders) = catalog.db.snapshot(|s| {
        let rows: Vec<Dated> = s.read(
            sql!("SELECT id, folder, filename, master_id, captured FROM photos"),
            &[],
        )?;
        Ok((rows, catalog.locate_folders(s)?))
    })?;
    let by_id: HashMap<PhotoId, &Dated> = rows.iter().map(|r| (r.id, r)).collect();
    let given: HashSet<PhotoId> = photos.iter().copied().collect();
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for id in photos {
        if !seen.insert(*id) {
            continue;
        }
        let Some(row) = by_id.get(id) else {
            ordered.push(Ordered {
                photo: *id,
                filename: String::new(),
                captured: None,
            });
            continue;
        };
        if row.master.is_some_and(|m| given.contains(&m)) {
            continue;
        }
        let captured = match row.captured.as_str() {
            "" => {
                if cancel.load(Ordering::Relaxed) {
                    return Err(FileOpError::Cancelled);
                }
                folders
                    .get(&row.folder)
                    .and_then(|f| f.path.as_ref())
                    .and_then(|dir| capture_time(&dir.join(&row.filename)))
            }
            text => Some(text.to_string()),
        };
        ordered.push(Ordered {
            photo: *id,
            filename: row.filename.clone(),
            captured,
        });
    }
    // Stable: ties keep the order given.
    ordered.sort_by(|a, b| match (&a.captured, &b.captured) {
        (Some(x), Some(y)) => x.cmp(y),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => key(OsStr::new(&a.filename)).cmp(&key(OsStr::new(&b.filename))),
    });
    Ok(RenameOrder(ordered))
}

/// One photo of a batch rename, as its dialog shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameRow {
    pub photo: PhotoId,
    /// Its file's name now, and the one it would have.
    pub old: String,
    pub new: String,
    /// Why it can't be renamed ([`Skip::Blocked`] when it could, but
    /// another can't); `None` when it can.
    pub problem: Option<Skip>,
    /// Its capture time isn't known: it follows the photos with one.
    pub undated: bool,
    /// What its name lacks of the template (a capture date).
    pub note: Option<String>,
}

/// A batch rename named and planned.
#[derive(Debug)]
pub struct RenamePreview {
    pub rows: Vec<RenameRow>,
    /// The plan of exactly these names, empty when any photo can't be
    /// renamed (but for having its name already).
    pub plan: Plan,
}

/// Names the photos of `order` by `template`, numbered in that order, and
/// plans the rename.
pub fn rename_preview(
    catalog: &Catalog,
    order: &RenameOrder,
    template: &NameTemplate,
) -> Result<RenamePreview, FileOpError> {
    let total = order.0.len();
    let mut rows = Vec::new();
    let mut renames = Vec::new();
    for (index, o) in order.0.iter().enumerate() {
        let fields = NameFields::captured(o.captured.clone());
        let at = NameContext {
            index,
            total,
            fields: &fields,
        };
        let (stem, notes) = template.stem(Path::new(&o.filename), &at);
        let note = (!notes.is_empty()).then(|| notes.join("; "));
        let new = match Path::new(&o.filename).extension() {
            Some(extension) => format!("{stem}.{}", extension.to_string_lossy()),
            None => stem.clone(),
        };
        rows.push(RenameRow {
            photo: o.photo,
            old: o.filename.clone(),
            new,
            problem: None,
            undated: o.captured.is_none(),
            note,
        });
        renames.push((o.photo, stem));
    }
    let plan = super::plan(catalog, FileOp::RenamePhotos { renames })?;
    for row in &mut rows {
        row.problem = plan
            .skipped()
            .iter()
            .find(|s| s.photo == row.photo)
            .map(|s| s.reason.clone());
    }
    Ok(RenamePreview { rows, plan })
}
