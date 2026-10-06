//! Adding a folder of photos to the catalog, with the edits they got from
//! releases that saved them beside the photo.
use super::Catalog;
use super::locations::{FolderLocation, join, logical_from_os, resolve_in};
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// What adding a folder did.
#[derive(Debug, Default)]
pub struct Added {
    /// Photos added.
    pub added: usize,
    /// Sidecars that could not be read.
    pub report: super::SidecarReport,
    /// Folders not added: a location on this computer says their photos are
    /// elsewhere.
    pub conflicts: Vec<Conflict>,
    /// Folders that equally specific locations claim. Nothing is added
    /// until one of the options is chosen for each.
    pub ambiguous: Vec<Ambiguity>,
}
/// A folder on disk not added because its catalog folder is elsewhere on
/// this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Conflict {
    pub directory: PathBuf,
    pub root: i64,
    /// The catalog folder it would have been, by logical path.
    pub relative: String,
    /// Where that folder is on this computer.
    pub located: PathBuf,
}
impl Conflict {
    pub fn message(&self) -> String {
        format!(
            "{} is linked to {} on this computer; {} was not added",
            if self.relative.is_empty() {
                "Its root folder"
            } else {
                &self.relative
            },
            self.located.display(),
            self.directory.display()
        )
    }
}
/// Folders on disk that more than one location on this computer contains
/// equally closely.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ambiguity {
    pub directories: Vec<PathBuf>,
    pub options: Vec<FolderLocation>,
}
/// Where a folder on disk goes in the catalog.
enum Place {
    /// A folder of an existing root, by logical path.
    Folder(i64, String),
    /// A folder of the root the added folder becomes, by logical path.
    New(String),
}

impl Catalog {
    pub fn add_folder(&mut self, folder: &Path) -> Result<usize> {
        Ok(self.add_folder_with(folder, &Default::default())?.0)
    }
    /// `import_folder` for callers that can't ask which location a folder
    /// belongs to: they get an error instead.
    pub fn add_folder_with(
        &mut self,
        folder: &Path,
        defaults: &super::MetadataDefaults,
    ) -> Result<(usize, super::SidecarReport)> {
        let added = self.import_folder(folder, defaults, &[])?;
        ensure!(
            added.ambiguous.is_empty(),
            "Choose which folder of the catalog {} is",
            added.ambiguous[0].directories[0].display()
        );
        Ok((added.added, added.report))
    }
    /// Adds a folder's new photos with the metadata of their XMP sidecars and
    /// of the XMP inside JPEGs and TIFFs, then the default Creator and
    /// Copyright where neither the file nor its sidecar has one. Photos
    /// already in the catalog are left alone; Read Metadata from Files reads
    /// theirs.
    ///
    /// Each folder found on disk, the one chosen and every one below it, is
    /// matched with this computer's locations (see `locations`): it joins the
    /// catalog folder of the most specific location containing it, if that
    /// folder is found there; a folder no location contains goes under a new
    /// root, the folder chosen. `choices` settle the ambiguous ones of an
    /// earlier call.
    pub fn import_folder(
        &mut self,
        folder: &Path,
        defaults: &super::MetadataDefaults,
        choices: &[FolderLocation],
    ) -> Result<Added> {
        let folder = folder.canonicalize()?;
        let mut files = Vec::new();
        fn walk(p: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
            for entry in std::fs::read_dir(p)? {
                let e = entry?;
                let t = e.file_type()?;
                if t.is_symlink() || crate::storage::is_hidden(&e.path()) {
                    continue;
                }
                if t.is_dir() {
                    walk(&e.path(), files)?
                } else if crate::storage::is_raw(&e.path())
                    || e.path().extension().is_some_and(|x| {
                        matches!(
                            x.to_string_lossy().to_ascii_lowercase().as_str(),
                            "jpg" | "jpeg" | "png" | "tif" | "tiff"
                        )
                    })
                {
                    files.push(e.path());
                }
            }
            Ok(())
        }
        walk(&folder, &mut files)?;
        let mut result = Added::default();
        let mut places = HashMap::new();
        let mut ambiguous: BTreeMap<Vec<FolderLocation>, Vec<PathBuf>> = BTreeMap::new();
        {
            let matcher = Matcher::new(self)?;
            let directories: BTreeSet<_> = files.iter().filter_map(|f| f.parent()).collect();
            for directory in directories {
                match matcher.place(directory, &folder, choices) {
                    Matched::Place(place) => {
                        places.insert(directory.to_path_buf(), place);
                    }
                    Matched::Conflict(conflict) => result.conflicts.push(conflict),
                    Matched::Ambiguous(options) => ambiguous
                        .entry(options)
                        .or_default()
                        .push(directory.to_path_buf()),
                }
            }
        }
        if !ambiguous.is_empty() {
            result.ambiguous = ambiguous
                .into_iter()
                .map(|(options, directories)| Ambiguity {
                    directories,
                    options,
                })
                .collect();
            return Ok(result);
        }
        let tx = self.db.transaction()?;
        let mut new_root = None;
        let mut folders: HashMap<(i64, String), i64> = HashMap::new();
        let mut added = Vec::new();
        for file in files {
            let Some(place) = file.parent().and_then(|d| places.get(d)) else {
                continue;
            };
            let (root, logical) = match place {
                Place::Folder(root, logical) => (*root, logical.clone()),
                Place::New(logical) => {
                    let root = match new_root {
                        Some(root) => root,
                        None => {
                            tx.execute(
                                "INSERT INTO roots(original_path) VALUES(?)",
                                [folder.to_string_lossy()],
                            )?;
                            *new_root.insert(tx.last_insert_rowid())
                        }
                    };
                    (root, logical.clone())
                }
            };
            let filename = file.file_name().unwrap().to_string_lossy();
            // A photo is the same one by root, folder and name; its virtual
            // copies share them and stay as they are.
            if tx
                .query_row(
                    "SELECT 1 FROM photos WHERE filename=? AND folder IN
                     (SELECT f.id FROM folders f JOIN folder_paths p ON p.folder=f.id
                      WHERE f.root=? AND p.path=?)",
                    params![filename, root, logical],
                    |r| r.get::<_, i32>(0),
                )
                .optional()?
                .is_some()
            {
                continue;
            }
            let key = (root, logical);
            let fid = match folders.get(&key) {
                Some(id) => *id,
                None => {
                    let (root, logical) = &key;
                    let existing = tx.query_row(
                        "SELECT min(f.id) FROM folders f JOIN folder_paths p ON p.folder=f.id
                             WHERE f.root=? AND p.path=?",
                        params![root, logical],
                        |r| r.get::<_, Option<i64>>(0),
                    )?;
                    let id = match existing {
                        Some(id) => id,
                        None => {
                            // Older releases read the folder in this system's form.
                            let native: PathBuf = super::locations::names(logical).collect();
                            tx.execute(
                                "INSERT INTO folders(root,relative_path) VALUES(?,?)",
                                params![root, native.to_string_lossy()],
                            )?;
                            let id = tx.last_insert_rowid();
                            tx.execute(
                                "INSERT INTO folder_paths(folder,path) VALUES(?,?)",
                                params![id, logical],
                            )?;
                            id
                        }
                    };
                    folders.insert(key, id);
                    id
                }
            };
            tx.execute(
                "INSERT INTO photos(folder,filename,original_path,format) VALUES(?,?,?,?)",
                params![
                    fid,
                    filename,
                    file.to_string_lossy(),
                    file.extension()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_ascii_uppercase()
                ],
            )?;
            added.push((tx.last_insert_rowid(), file));
        }
        tx.commit()?;
        for (id, file) in &added {
            if crate::storage::is_raw(file) {
                // A sidecar that no longer matches its photo stays unused on disk.
                let _ = self.import_sidecar(*id, file);
            }
        }
        let report = self.import_file_metadata(&added)?;
        // A photo whose metadata couldn't be read may have its own: no
        // default goes in its place.
        let read: Vec<(i64, PathBuf)> = added
            .iter()
            .filter(|(_, file)| {
                let own = super::sidecars(file);
                !report
                    .unreadable
                    .iter()
                    .any(|(path, _)| path == file || own.contains(path))
            })
            .cloned()
            .collect();
        self.apply_defaults(&read, defaults)?;
        result.added = added.len();
        result.report = report;
        Ok(result)
    }
    /// Records capture times read from the photos' files, in one transaction.
    /// Only empty dates are filled, never one Lightroom or the user set, and a
    /// photo's virtual copies get its date too.
    pub fn fill_capture_times(&mut self, times: &[(i64, String)]) -> Result<()> {
        let tx = self.db.transaction()?;
        {
            // Two statements, each on an index, rather than one OR that scans.
            let mut photo =
                tx.prepare("UPDATE photos SET captured=?1 WHERE id=?2 AND captured=''")?;
            let mut copies =
                tx.prepare("UPDATE photos SET captured=?1 WHERE master_id=?2 AND captured=''")?;
            for (id, captured) in times {
                photo.execute(params![captured, id])?;
                copies.execute(params![captured, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Carries the edit a photo got outside any catalog, in its
    /// photo.rawmakase.json sidecar, into the catalog. The sidecar stays on disk.
    fn import_sidecar(&self, id: i64, file: &Path) -> Result<()> {
        let Some((sidecar, bitmaps)) = crate::storage::import(file)? else {
            return Ok(());
        };
        for bitmap in bitmaps {
            self.put_bitmap(&bitmap)?;
        }
        self.save_edit(
            id,
            file,
            &sidecar.recipe,
            &sidecar.export,
            super::HistoryUpdate::Keep,
        )
    }
}

/// This computer's locations, for matching folders found on disk.
struct Matcher {
    /// Every location: each root's own, or where it was added when it has
    /// none here, and every folder's; with its path as the disk has it.
    locations: Vec<(FolderLocation, PathBuf)>,
    roots: HashMap<i64, Root>,
}
/// Where a root was added and its rows on this computer, as stored and as
/// the disk has them.
struct Root {
    original: String,
    rows: Vec<(String, PathBuf)>,
    original_on_disk: String,
    rows_on_disk: Vec<(String, PathBuf)>,
}
/// What a folder found on disk is in the catalog.
enum Matched {
    Place(Place),
    Conflict(Conflict),
    Ambiguous(Vec<FolderLocation>),
}
impl Matcher {
    fn new(catalog: &Catalog) -> Result<Self> {
        let mut rows = catalog.location_rows()?;
        // Paths are compared as the disk has them (no symlinks, no trailing
        // separators); one that isn't there stays as it is.
        let real = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut locations = Vec::new();
        let mut roots = HashMap::new();
        for (root, original, _) in catalog.roots()? {
            let own = rows.remove(&root).unwrap_or_default();
            if !own.iter().any(|(relative, _)| relative.is_empty()) {
                let path = PathBuf::from(&original);
                locations.push((
                    FolderLocation {
                        root,
                        relative: String::new(),
                        path: path.clone(),
                    },
                    real(&path),
                ));
            }
            for (relative, path) in &own {
                locations.push((
                    FolderLocation {
                        root,
                        relative: relative.clone(),
                        path: path.clone(),
                    },
                    real(path),
                ));
            }
            let rows_on_disk = own
                .iter()
                .map(|(relative, path)| (relative.clone(), real(path)))
                .collect();
            let original_on_disk = real(Path::new(&original)).to_string_lossy().into_owned();
            roots.insert(
                root,
                Root {
                    original,
                    rows: own,
                    original_on_disk,
                    rows_on_disk,
                },
            );
        }
        Ok(Self { locations, roots })
    }
    /// Where `directory`, found below the `chosen` folder, goes.
    fn place(&self, directory: &Path, chosen: &Path, choices: &[FolderLocation]) -> Matched {
        // (specificity, location, logical path, whether it resolves back here)
        let mut candidates = Vec::new();
        for (location, on_disk) in &self.locations {
            let Ok(rest) = directory.strip_prefix(on_disk) else {
                continue;
            };
            let logical = join(&location.relative, &logical_from_os(rest));
            let root = &self.roots[&location.root];
            // The folder it would be must be found right here, not hidden by a
            // more specific location elsewhere.
            let found = resolve_in(
                &root.original_on_disk,
                &root.rows_on_disk,
                &logical,
                cfg!(windows),
            )
            .as_deref()
                == Some(directory);
            candidates.push((on_disk.components().count(), location, logical, found));
        }
        if candidates.is_empty() {
            let rest = directory.strip_prefix(chosen).unwrap_or(Path::new(""));
            return Matched::Place(Place::New(logical_from_os(rest)));
        }
        let valid: Vec<_> = candidates.iter().filter(|c| c.3).collect();
        let Some(best) = valid.iter().map(|c| c.0).max() else {
            let (_, location, logical, _) = candidates.iter().max_by_key(|c| c.0).unwrap();
            let root = &self.roots[&location.root];
            return Matched::Conflict(Conflict {
                directory: directory.to_path_buf(),
                root: location.root,
                relative: logical.clone(),
                located: resolve_in(&root.original, &root.rows, logical, cfg!(windows))
                    .unwrap_or_default(),
            });
        };
        let mut closest: Vec<_> = valid.into_iter().filter(|c| c.0 == best).collect();
        closest.dedup_by(|a, b| a.1.root == b.1.root && a.2 == b.2);
        let distinct: BTreeSet<_> = closest.iter().map(|c| (c.1.root, &c.2)).collect();
        if distinct.len() == 1 {
            let (_, location, logical, _) = closest[0];
            return Matched::Place(Place::Folder(location.root, logical.clone()));
        }
        if let Some((_, location, logical, _)) = closest.iter().find(|c| {
            choices
                .iter()
                .any(|choice| choice.root == c.1.root && choice.relative == c.1.relative)
        }) {
            return Matched::Place(Place::Folder(location.root, logical.clone()));
        }
        let mut options: Vec<FolderLocation> = closest.iter().map(|c| c.1.clone()).collect();
        options.sort();
        options.dedup();
        Matched::Ambiguous(options)
    }
}
