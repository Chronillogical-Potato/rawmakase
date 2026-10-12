//! The filesystem side of a synchronization: lists each catalog folder in
//! scope where it is on this computer, finds the files no catalog photo
//! names, and matches them with the checksums on record. It reads files and
//! the inputs captured from the catalog, and writes nothing.
use super::{
    Addition, Choice, FileState, Missing, Move, NotChecked, SyncInputs, SyncPlan, state_of,
};
use crate::catalog::ingest::{Matched, Place};
use crate::catalog::{FolderId, PhotoId, RootId};
use crate::storage::{ChecksumRead, FileChecksum, Identity};
use anyhow::{Result, bail};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// What listing one directory gave.
enum Listing {
    /// The names of the photo files in it, and its subdirectories.
    Listed {
        files: HashSet<OsString>,
        directories: Vec<PathBuf>,
    },
    /// It isn't there.
    Absent,
    Failed(String),
}

fn list(directory: &Path) -> Listing {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Listing::Absent,
        Err(e) => return Listing::Failed(format!("{e}")),
    };
    let mut files = HashSet::new();
    let mut directories = Vec::new();
    for entry in entries {
        let entry = match entry.and_then(|e| e.file_type().map(|t| (e, t))) {
            Ok(entry) => entry,
            Err(e) => return Listing::Failed(format!("{e}")),
        };
        let (entry, kind) = entry;
        let path = entry.path();
        if kind.is_symlink() || crate::storage::is_hidden(&path) {
            continue;
        }
        if kind.is_dir() {
            directories.push(path);
        } else if is_photo(&path) {
            files.insert(entry.file_name());
        }
    }
    Listing::Listed { files, directories }
}

/// A file Add Folder would add.
fn is_photo(path: &Path) -> bool {
    crate::storage::is_raw(path)
        || path.extension().is_some_and(|x| {
            matches!(
                x.to_string_lossy().to_ascii_lowercase().as_str(),
                "jpg" | "jpeg" | "png" | "tif" | "tiff"
            )
        })
}

/// What one unknown file matched.
enum Found {
    New,
    Copy(Vec<PhotoId>),
    /// Missing masters it may be, each verified or not.
    Moved(Vec<(PhotoId, bool)>),
    /// Missing masters it may be, while it is also a copy of a photo that
    /// is there: always the user's choice.
    Contested(Vec<(PhotoId, bool)>),
}

/// Scans what `inputs` covers. `online` says whether the volume of a
/// location is mounted (the app's volume checks; always true where it has
/// none); `cancel` stops the scan between files and hash chunks, and
/// `progress` is told how many files were looked at.
pub fn scan(
    inputs: &SyncInputs,
    online: &dyn Fn(&Path) -> bool,
    cancel: &AtomicBool,
    progress: &dyn Fn(usize),
) -> Result<SyncPlan> {
    let cancelled = || cancel.load(Ordering::Relaxed);
    let mut plan = SyncPlan::default();
    // Every master's file, so a file a photo names is never unknown.
    let known: HashSet<PathBuf> = inputs
        .masters
        .keys()
        .filter_map(|id| inputs.path_of(*id))
        .collect();
    // Each scope folder's directory, listed once however many folders share it.
    let mut listings: HashMap<PathBuf, Listing> = HashMap::new();
    let mut scope_paths: HashSet<PathBuf> = HashSet::new();
    for id in &inputs.scope {
        let folder = &inputs.folders[id];
        match &folder.path {
            Some(path) => {
                scope_paths.insert(path.clone());
                listings.entry(path.clone()).or_insert_with(|| list(path));
            }
            None => plan.not_checked.push(NotChecked {
                path: PathBuf::from(&folder.logical),
                reason: "not on this computer".into(),
            }),
        }
    }
    let all_paths: HashSet<&PathBuf> = inputs
        .folders
        .values()
        .filter_map(|f| f.path.as_ref())
        .collect();
    // Where each scope photo is.
    let mut states: HashMap<PhotoId, FileState> = HashMap::new();
    let in_scope: HashSet<FolderId> = inputs.scope.iter().copied().collect();
    for master in inputs.masters.values() {
        if !in_scope.contains(&master.folder) {
            continue;
        }
        let folder = &inputs.folders[&master.folder];
        let state = match folder.path.as_ref().and_then(|p| listings.get(p)) {
            None => FileState::Unavailable,
            Some(Listing::Listed { files, .. }) => {
                if files.contains(std::ffi::OsStr::new(&master.filename)) {
                    FileState::Present
                } else if folder.anchor.as_deref().is_some_and(online) {
                    FileState::Missing
                } else {
                    FileState::Unavailable
                }
            }
            Some(Listing::Absent) | Some(Listing::Failed(_)) => state_of(
                inputs.path_of(master.id).as_deref(),
                folder.anchor.as_deref(),
                online,
            ),
        };
        states.insert(master.id, state);
    }
    for (path, listing) in &listings {
        if let Listing::Failed(why) = listing {
            plan.not_checked.push(NotChecked {
                path: path.clone(),
                reason: why.clone(),
            });
        }
    }
    // Files no photo names: in the folders listed, and in directories below
    // them that no catalog folder is.
    let mut unknown: Vec<(PathBuf, (RootId, String))> = Vec::new();
    let mut new_directories: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    // The place of each directory listed: its scope folder's.
    let places: HashMap<&PathBuf, (RootId, String)> = inputs
        .scope
        .iter()
        .filter_map(|id| {
            let f = &inputs.folders[id];
            Some((f.path.as_ref()?, (f.root, f.logical.clone())))
        })
        .collect();
    let mut pending: Vec<PathBuf> = Vec::new();
    for (path, listing) in &listings {
        let Listing::Listed { files, directories } = listing else {
            continue;
        };
        unknown.extend(
            files
                .iter()
                .map(|name| path.join(name))
                .filter(|file| !known.contains(file))
                .map(|file| (file, places[path].clone())),
        );
        pending.extend(
            directories
                .iter()
                .filter(|d| !all_paths.contains(d))
                .cloned(),
        );
    }
    // Locations no scope folder is, walked as new directories are.
    pending.extend(
        inputs
            .walk
            .iter()
            .filter(|path| !listings.contains_key(*path))
            .cloned(),
    );
    while let Some(directory) = pending.pop() {
        if cancelled() {
            bail!("Synchronization stopped");
        }
        match list(&directory) {
            Listing::Listed { files, directories } => {
                let files: Vec<PathBuf> = files
                    .iter()
                    .map(|name| directory.join(name))
                    .filter(|file| !known.contains(file))
                    .collect();
                if !files.is_empty() {
                    new_directories.insert(directory.clone(), files);
                }
                pending.extend(directories.into_iter().filter(|d| !all_paths.contains(d)));
            }
            Listing::Absent => {}
            Listing::Failed(reason) => plan.not_checked.push(NotChecked {
                path: directory,
                reason,
            }),
        }
    }
    for (directory, files) in new_directories {
        match inputs.matcher.place(&directory, &directory, &[]) {
            Matched::Place(Place::Folder(root, logical)) => {
                unknown.extend(files.into_iter().map(|f| (f, (root, logical.clone()))));
            }
            // Below no location: never the case below a catalog folder.
            Matched::Place(Place::New(_)) => plan.not_checked.push(NotChecked {
                path: directory,
                reason: "not in a catalog folder".into(),
            }),
            Matched::Conflict(conflict) => plan.conflicts.push(conflict),
            Matched::Ambiguous(_) => plan.ambiguous.push(directory),
        }
    }
    unknown.sort();
    // Each unknown file matched with the checksums on record.
    let mut matcher = Matching {
        inputs,
        online,
        cancel,
        states,
    };
    let mut found: Vec<(PathBuf, (RootId, String), Identity, Found)> = Vec::new();
    for (n, (file, place)) in unknown.into_iter().enumerate() {
        if cancelled() {
            bail!("Synchronization stopped");
        }
        progress(n + 1);
        match matcher.find(&file) {
            Ok(Some((identity, what))) => found.push((file, place, identity, what)),
            Ok(None) => {}
            Err(e) => plan.not_checked.push(NotChecked {
                path: file,
                reason: format!("{e:#}"),
            }),
        }
    }
    // A missing photo two files claim, or a file two photos claim, is the
    // user's to settle.
    let mut claims: HashMap<PhotoId, usize> = HashMap::new();
    for (_, _, _, what) in &found {
        if let Found::Moved(photos) | Found::Contested(photos) = what {
            for (photo, _) in photos {
                *claims.entry(*photo).or_default() += 1;
            }
        }
    }
    let mut choices: Vec<Vec<Move>> = Vec::new();
    let mut claimed: HashSet<PhotoId> = HashSet::new();
    for (file, place, identity, what) in found {
        let contested = matches!(what, Found::Contested(_));
        match what {
            Found::New => plan.additions.push(Addition {
                file,
                copy_of: Vec::new(),
            }),
            Found::Copy(of) => plan.additions.push(Addition { file, copy_of: of }),
            Found::Moved(photos) | Found::Contested(photos) => {
                let moves: Vec<Move> = photos
                    .iter()
                    .filter_map(|(photo, verified)| {
                        Some(Move {
                            photo: *photo,
                            from: inputs.path_of(*photo)?,
                            to: file.clone(),
                            identity: identity.clone(),
                            verified: *verified,
                            place: place.clone(),
                        })
                    })
                    .collect();
                claimed.extend(moves.iter().map(|m| m.photo));
                if !contested && moves.len() == 1 && claims[&moves[0].photo] == 1 {
                    plan.moves.extend(moves);
                } else {
                    choices.push(moves);
                }
            }
        }
    }
    // Choices sharing a photo are one choice.
    let mut merged: Vec<Vec<Move>> = Vec::new();
    for options in choices {
        let photos: BTreeSet<PhotoId> = options.iter().map(|m| m.photo).collect();
        let (mut joined, rest): (Vec<_>, Vec<_>) = merged
            .into_iter()
            .partition(|c: &Vec<Move>| c.iter().any(|m| photos.contains(&m.photo)));
        let mut group: Vec<Move> = joined.drain(..).flatten().collect();
        group.extend(options);
        merged = rest;
        merged.push(group);
    }
    plan.choices = merged
        .into_iter()
        .map(|options| Choice { options })
        .collect();
    plan.moves.sort_by_key(|m| (!m.verified, m.to.clone()));
    // A mount gone during the scan is no evidence of anything missing.
    for (id, state) in matcher.states.iter_mut() {
        if *state == FileState::Missing {
            let anchor = inputs.folders[&inputs.masters[id].folder].anchor.as_deref();
            if !anchor.is_some_and(online) {
                *state = FileState::Error("its volume went offline during the scan".into());
            }
        }
    }
    let mut missing: Vec<Missing> = Vec::new();
    for (id, state) in &matcher.states {
        let in_scope = in_scope.contains(&inputs.masters[id].folder);
        match state {
            FileState::Missing if in_scope && !claimed.contains(id) => {
                if let Some(path) = inputs.path_of(*id) {
                    missing.push(Missing { photo: *id, path });
                }
            }
            FileState::Error(reason) if in_scope => plan.not_checked.push(NotChecked {
                path: inputs.path_of(*id).unwrap_or_default(),
                reason: reason.clone(),
            }),
            _ => {}
        }
    }
    // A move or choice whose photo turned out not to be missing is dropped.
    let still_missing = |id: &PhotoId| matcher.states.get(id) == Some(&FileState::Missing);
    plan.moves.retain(|m| still_missing(&m.photo));
    for choice in &mut plan.choices {
        choice.options.retain(|m| still_missing(&m.photo));
    }
    plan.choices.retain(|c| !c.options.is_empty());
    missing.sort_by(|a, b| a.path.cmp(&b.path));
    plan.missing = missing;
    // Folders whose directory is gone from a location that is there.
    plan.gone_folders = inputs
        .scope
        .iter()
        .filter(|id| {
            let folder = &inputs.folders[id];
            folder
                .path
                .as_ref()
                .is_some_and(|p| matches!(listings.get(p), Some(Listing::Absent)))
                && folder.anchor.as_deref().is_some_and(|anchor| {
                    folder.path.as_deref() != Some(anchor)
                        && std::fs::read_dir(anchor).is_ok()
                        && online(anchor)
                })
        })
        .copied()
        .collect();
    plan.gone_folders.sort();
    Ok(plan)
}

/// Matching unknown files, keeping each photo's state once worked out.
struct Matching<'a> {
    inputs: &'a SyncInputs,
    online: &'a dyn Fn(&Path) -> bool,
    cancel: &'a AtomicBool,
    states: HashMap<PhotoId, FileState>,
}

impl Matching<'_> {
    /// Where master `id`'s file stands; one outside the scope is checked
    /// on its own, by the same rules.
    fn state(&mut self, id: PhotoId) -> FileState {
        if let Some(state) = self.states.get(&id) {
            return state.clone();
        }
        let anchor = self
            .inputs
            .masters
            .get(&id)
            .and_then(|m| self.inputs.folders.get(&m.folder))
            .and_then(|f| f.anchor.clone());
        let state = state_of(
            self.inputs.path_of(id).as_deref(),
            anchor.as_deref(),
            self.online,
        );
        self.states.insert(id, state.clone());
        state
    }
    fn go_on(&self) -> bool {
        !self.cancel.load(Ordering::Relaxed)
    }
    /// The full SHA-256 of `path`, if it can be read whole and unchanged.
    fn full(&self, path: &Path) -> Result<Option<String>> {
        Ok(match FileChecksum::read_full(path, |_| self.go_on())? {
            ChecksumRead::Done(c) => c.full,
            ChecksumRead::Changed => None,
            ChecksumRead::Cancelled => bail!("Synchronization stopped"),
        })
    }
    /// What `file` is: new, a copy, or a photo moved. `None` when it changed
    /// while being read.
    fn find(&mut self, file: &Path) -> Result<Option<(Identity, Found)>> {
        let quick = match FileChecksum::read_quick(file, |_| self.go_on())? {
            ChecksumRead::Done(quick) => quick,
            ChecksumRead::Changed => return Ok(None),
            ChecksumRead::Cancelled => bail!("Synchronization stopped"),
        };
        let identity = quick.identity.clone();
        let mut hits: BTreeSet<PhotoId> = BTreeSet::new();
        if let Some(ids) = self.inputs.by_quick.get(&(quick.size, quick.quick.clone())) {
            hits.extend(ids);
        }
        if let Some(ids) = self
            .inputs
            .by_edit
            .get(&(identity.size, identity.prefix_hash.clone()))
        {
            hits.extend(ids);
        }
        let mut full: Option<Option<String>> = None;
        let mut missing: Vec<(PhotoId, bool)> = Vec::new();
        let mut present: Vec<PhotoId> = Vec::new();
        for id in hits {
            let stored = self.inputs.checksums.get(&id);
            // A quick hash on record that differs rules it out.
            if stored.is_some_and(|c| c.size != quick.size || c.quick != quick.quick) {
                continue;
            }
            let stored_full = stored.and_then(|c| c.full.clone());
            match self.state(id) {
                FileState::Missing => match stored_full {
                    Some(theirs) => {
                        let ours = match &full {
                            Some(f) => f.clone(),
                            None => full.insert(self.full(file)?).clone(),
                        };
                        if ours.as_deref() == Some(theirs.as_str()) {
                            missing.push((id, true));
                        }
                    }
                    None => missing.push((id, false)),
                },
                FileState::Present => {
                    let theirs = match stored_full {
                        Some(theirs) => Some(theirs),
                        None => match self.inputs.path_of(id) {
                            Some(path) => self.full(&path)?,
                            None => None,
                        },
                    };
                    let ours = match &full {
                        Some(f) => f.clone(),
                        None => full.insert(self.full(file)?).clone(),
                    };
                    if theirs.is_some() && ours == theirs {
                        present.push(id);
                    }
                }
                // A photo that may still be where it was is never moved.
                FileState::Unavailable | FileState::Error(_) => {}
            }
        }
        let found = match (missing.is_empty(), present.is_empty()) {
            (true, true) => Found::New,
            (true, false) => Found::Copy(present),
            (false, true) => Found::Moved(missing),
            // A present match competes: the user decides, never the scan.
            (false, false) => Found::Contested(missing),
        };
        Ok(Some((identity, found)))
    }
}
