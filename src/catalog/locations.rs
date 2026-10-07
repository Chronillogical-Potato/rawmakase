//! Where each computer finds the catalog's folders, so one catalog can move
//! between computers that mount the same photos at different paths (#187).
//!
//! A folder is its root and a logical path (`folder_paths`): names joined by
//! '/', whatever system added it. Each computer keeps its own locations in
//! `folder_locations`, one for a root ('') or for a folder and everything
//! below it; the most specific one wins, and without one a folder is where
//! its root was added. The legacy `roots.mapped_path` and `folder_mappings`
//! are copied into a computer's rows the first time it opens the catalog
//! with this release; after that they are only written, for older releases.
use super::{Catalog, FolderId, RootId};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A computer that opens catalogs: a random id made once per install, kept
/// in its own data folder, and a name to show.
#[derive(Clone, Debug)]
pub struct Computer {
    pub id: String,
    pub name: String,
}
impl Computer {
    /// This computer.
    pub fn this() -> Computer {
        // Tests never write to the data folder of the computer they run on.
        if cfg!(test) {
            return Computer {
                id: "test".into(),
                name: "Test".into(),
            };
        }
        Self::load_from(&crate::storage::local_data_dir()).unwrap_or_else(|_| {
            // Without a writable data folder, the name is the most stable id left.
            let name = host_name();
            Computer {
                id: format!("host:{name}"),
                name,
            }
        })
    }
    /// The computer whose id is kept in `dir`, made there the first time.
    pub fn load_from(dir: &Path) -> Result<Computer> {
        let file = dir.join(id_file());
        let read = |file: &Path| -> Option<String> {
            let id = std::fs::read_to_string(file).ok()?.trim().to_string();
            (!id.is_empty()).then_some(id)
        };
        let id = match read(&file) {
            Some(id) => id,
            None => {
                std::fs::create_dir_all(dir)?;
                let mut bytes = [0u8; 16];
                getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
                let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                let temp = tempfile::NamedTempFile::new_in(dir)?;
                std::fs::write(temp.path(), &id)?;
                // Another process may have made one meanwhile: theirs stays.
                match temp.persist_noclobber(&file) {
                    Ok(_) => id,
                    Err(_) => read(&file).context("Unreadable computer id")?,
                }
            }
        };
        Ok(Computer {
            id,
            name: host_name(),
        })
    }
}
/// The name of the file holding the computer id. On Linux the data folder
/// can be in a home directory several computers share (over NFS, or a synced
/// XDG_DATA_HOME), so each machine, as /etc/machine-id (else its host
/// name) tells them apart, keeps its own.
fn id_file() -> String {
    let machine = cfg!(target_os = "linux")
        .then(|| std::fs::read_to_string("/etc/machine-id").ok())
        .flatten()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()));
    // Without one, the host name still tells machines sharing a home apart.
    let host = || {
        let name: String = host_name()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        (!name.is_empty()).then_some(name)
    };
    match machine {
        Some(machine) => format!("computer-id-{machine}"),
        None if cfg!(target_os = "linux") => match host() {
            Some(host) => format!("computer-id-host-{host}"),
            None => "computer-id".into(),
        },
        None => "computer-id".into(),
    }
}
/// The computer's name as its system shows it, for labels only.
fn host_name() -> String {
    let run = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| {
            cfg!(target_os = "macos")
                .then(|| run("scutil", &["--get", "ComputerName"]))
                .flatten()
        })
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .or_else(|| run("hostname", &[]))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "This computer".into())
}

/// Whether to keep a root's folder locations when the root moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overrides {
    Keep,
    Clear,
}
/// A folder (and its subfolders) located apart from its root on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Override {
    /// Its logical path in the root.
    pub relative: String,
    pub path: PathBuf,
}
/// A root, where it is on this computer and elsewhere.
#[derive(Clone, Debug)]
pub struct RootLocations {
    pub root: RootId,
    /// Where it was added.
    pub original: String,
    /// This computer's location of the root, if it has one.
    pub location: Option<PathBuf>,
    /// Where the root is on this computer: its location, else where it was added.
    pub path: PathBuf,
    /// Folders below it located separately on this computer.
    pub overrides: Vec<Override>,
    /// Other computers' locations: computer name, logical path, path.
    pub elsewhere: Vec<(String, String, PathBuf)>,
}
/// A place on this computer where a root ('') or folder is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FolderLocation {
    pub root: RootId,
    pub relative: String,
    pub path: PathBuf,
}

/// The names of a logical path.
pub(super) fn names(logical: &str) -> impl Iterator<Item = &str> {
    logical.split('/').filter(|n| !n.is_empty())
}
/// Whether `path` was written on Windows: a drive letter or a UNC share.
fn is_windows_path(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with(r"\\")
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'\\' | b'/'))
}
/// The logical path of a folder an older release stored as `relative` in a
/// root added at `original`: '\' separates names only in a Windows root,
/// since elsewhere it can be part of one.
pub(super) fn logical_from_legacy(original: &str, relative: &str) -> String {
    let windows = is_windows_path(original);
    relative
        .split(|c| c == '/' || (windows && c == '\\'))
        .filter(|n| !n.is_empty() && *n != ".")
        .collect::<Vec<_>>()
        .join("/")
}
/// The logical path of a folder at `relative` below a location on this system.
pub(super) fn logical_from_os(relative: &Path) -> String {
    relative
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}
/// Joins two logical paths.
pub(super) fn join(parent: &str, child: &str) -> String {
    names(parent)
        .chain(names(child))
        .collect::<Vec<_>>()
        .join("/")
}
/// Where logical folder `logical` of a root added at `original` is, given
/// that root's `rows` (logical path, location): below the most specific row
/// containing it, else below `original`. `None` when one of its names can't
/// be a file name on this system (a Unix name with a '\' on Windows): it is
/// unavailable here, never read as more folders.
pub fn resolve_in(
    original: &str,
    rows: &[(String, PathBuf)],
    logical: &str,
    windows: bool,
) -> Option<PathBuf> {
    let parts: Vec<&str> = names(logical).collect();
    if windows
        && parts
            .iter()
            .any(|n| n.contains(['\\', '<', '>', ':', '"', '|', '?', '*']))
    {
        return None;
    }
    let mut best: Option<(usize, &PathBuf)> = None;
    for (relative, path) in rows {
        let row: Vec<&str> = names(relative).collect();
        if row.len() <= parts.len()
            && parts[..row.len()] == row[..]
            && best.is_none_or(|(n, _)| row.len() > n)
        {
            best = Some((row.len(), path));
        }
    }
    let (skip, mut path) = match best {
        Some((n, path)) => (n, path.clone()),
        None => (0, PathBuf::from(original)),
    };
    for name in &parts[skip..] {
        path.push(name);
    }
    Some(path)
}

/// Readies a catalog for `computer`, the only writing an open does, in one
/// transaction: registers the computer, gives folders an older release added
/// their logical path, and once per computer adopts the legacy mappings.
pub(super) fn prepare(db: &mut Connection, computer: &Computer) -> Result<()> {
    // Most opens have nothing to write; a write would wait for other computers.
    let adopted = |db: &Connection| -> rusqlite::Result<bool> {
        Ok(db
            .query_row(
                "SELECT adopted_at IS NOT NULL FROM computers WHERE id=?",
                [&computer.id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    };
    const UNMAPPED: &str = "FROM folders f JOIN roots r ON r.id=f.root
        LEFT JOIN folder_paths p ON p.folder=f.id WHERE p.folder IS NULL";
    let unmapped = |db: &Connection| -> rusqlite::Result<bool> {
        db.query_row(&format!("SELECT EXISTS(SELECT 1 {UNMAPPED})"), [], |r| {
            r.get(0)
        })
    };
    if adopted(db)? && !unmapped(db)? {
        return Ok(());
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute(
        "INSERT OR IGNORE INTO computers(id, name) VALUES (?, ?)",
        params![computer.id, computer.name],
    )?;
    let legacy: Vec<(FolderId, String, String)> = tx
        .prepare(&format!(
            "SELECT f.id, r.original_path, f.relative_path {UNMAPPED}"
        ))?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (folder, original, relative) in legacy {
        tx.execute(
            "INSERT INTO folder_paths(folder, path) VALUES (?, ?)",
            params![folder, logical_from_legacy(&original, &relative)],
        )?;
    }
    if !adopted(&tx)? {
        // Everything the legacy mapping says, the root and every folder, so
        // a later change of one folder leaves the others where they were.
        tx.execute(
            "INSERT OR IGNORE INTO folder_locations(root, relative_path, computer, path)
             SELECT id, '', ?, mapped_path FROM roots WHERE mapped_path IS NOT NULL",
            [&computer.id],
        )?;
        // A folder's mapping wins over its root's, as in older releases.
        tx.execute(
            "INSERT OR REPLACE INTO folder_locations(root, relative_path, computer, path)
             SELECT f.root, p.path, ?, m.path FROM folder_mappings m
             JOIN folders f ON f.id=m.folder JOIN folder_paths p ON p.folder=f.id
             ORDER BY f.id",
            [&computer.id],
        )?;
        tx.execute(
            "UPDATE computers SET adopted_at=CURRENT_TIMESTAMP WHERE id=?",
            [&computer.id],
        )?;
    }
    tx.commit()?;
    Ok(())
}

impl Catalog {
    /// The computer this catalog was opened on.
    pub fn computer(&self) -> &Computer {
        &self.computer
    }
    /// This computer's locations, by root: (logical path, location).
    pub(super) fn location_rows(&self) -> Result<HashMap<RootId, Vec<(String, PathBuf)>>> {
        let mut rows: HashMap<RootId, Vec<(String, PathBuf)>> = HashMap::new();
        let mut q = self.db.prepare(
            "SELECT root, relative_path, path FROM folder_locations WHERE computer=?
             ORDER BY root, relative_path",
        )?;
        for row in q.query_map([&self.computer.id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, String>(2)?))
        })? {
            let (root, relative, path) = row?;
            rows.entry(root)
                .or_default()
                .push((relative, PathBuf::from(path)));
        }
        Ok(rows)
    }
    /// A folder's root and logical path.
    fn logical_path(&self, folder: FolderId) -> Result<(RootId, String)> {
        let (root, original, relative, logical): (RootId, String, String, Option<String>) = self
            .db
            .query_row(
                "SELECT f.root, r.original_path, f.relative_path, p.path
                 FROM folders f JOIN roots r ON r.id=f.root
                 LEFT JOIN folder_paths p ON p.folder=f.id WHERE f.id=?",
                [folder],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?
            .context("Unknown folder")?;
        Ok((
            root,
            logical.unwrap_or_else(|| logical_from_legacy(&original, &relative)),
        ))
    }
    /// Finds root `id` at `path` on this computer, keeping its folders'
    /// own locations.
    pub fn relink_root(&self, id: RootId, path: &Path) -> Result<()> {
        self.set_root_location(id, path, Overrides::Keep)
    }
    /// Finds root `id` at `path` on this computer, keeping or clearing the
    /// locations of folders below it.
    pub fn relink_root_with(
        &mut self,
        id: RootId,
        path: &Path,
        overrides: Overrides,
    ) -> Result<()> {
        self.set_root_location(id, path, overrides)
    }
    fn set_root_location(&self, id: RootId, path: &Path, overrides: Overrides) -> Result<()> {
        ensure!(path.is_dir(), "Choose an existing folder");
        let path = path.to_string_lossy();
        let tx = self.db.unchecked_transaction()?;
        // Older releases read the last change made anywhere.
        ensure!(
            tx.execute(
                "UPDATE roots SET mapped_path=? WHERE id=?",
                params![path, id]
            )? == 1,
            "Unknown root"
        );
        tx.execute(
            "INSERT INTO folder_locations(root, relative_path, computer, path) VALUES (?, '', ?, ?)
             ON CONFLICT DO UPDATE SET path=excluded.path",
            params![id, self.computer.id, path],
        )?;
        if overrides == Overrides::Clear {
            let below: Vec<String> = tx
                .prepare(
                    "SELECT relative_path FROM folder_locations
                     WHERE root=? AND computer=? AND relative_path<>''",
                )?
                .query_map(params![id, self.computer.id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for relative in below {
                clear(&tx, &self.computer.id, id, &relative)?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Finds folder `id` and its subfolders at `path` on this computer.
    pub fn relink_folder(&self, id: FolderId, path: &Path) -> Result<()> {
        ensure!(path.is_dir(), "Choose an existing folder");
        let (root, logical) = self.logical_path(id)?;
        let path = path.to_string_lossy();
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO folder_mappings(folder, path) VALUES (?, ?)
             ON CONFLICT(folder) DO UPDATE SET path=excluded.path",
            params![id, path],
        )?;
        tx.execute(
            "INSERT INTO folder_locations(root, relative_path, computer, path) VALUES (?, ?, ?, ?)
             ON CONFLICT DO UPDATE SET path=excluded.path",
            params![root, logical, self.computer.id, path],
        )?;
        tx.commit()?;
        Ok(())
    }
    /// Forgets this computer's location of root `root` ('') or of its folder
    /// `relative`: it is then found through the nearest location above it,
    /// else where its root was added. Folders below keep their own.
    pub fn clear_folder_location(&mut self, root: RootId, relative: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        clear(&tx, &self.computer.id, root, relative)?;
        tx.commit()?;
        Ok(())
    }
    /// The folders below root `root` located separately on this computer.
    pub fn root_overrides(&self, root: RootId) -> Result<Vec<Override>> {
        Ok(self
            .location_rows()?
            .remove(&root)
            .unwrap_or_default()
            .into_iter()
            .filter(|(relative, _)| !relative.is_empty())
            .map(|(relative, path)| Override { relative, path })
            .collect())
    }
    /// Every root with its locations, for Folder locations.
    pub fn folder_locations(&self) -> Result<Vec<RootLocations>> {
        let mut rows = self.location_rows()?;
        let mut elsewhere: HashMap<RootId, Vec<(String, String, PathBuf)>> = HashMap::new();
        let mut q = self.db.prepare(
            "SELECT l.root, c.name, l.relative_path, l.path FROM folder_locations l
             JOIN computers c ON c.id=l.computer WHERE l.computer<>?
             ORDER BY c.name, l.relative_path",
        )?;
        for row in q.query_map([&self.computer.id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, String>(3)?))
        })? {
            let (root, name, relative, path) = row?;
            elsewhere
                .entry(root)
                .or_default()
                .push((name, relative, PathBuf::from(path)));
        }
        Ok(self
            .roots()?
            .into_iter()
            .map(|(root, original, location)| {
                let own = rows.remove(&root).unwrap_or_default();
                let location = location.map(PathBuf::from);
                RootLocations {
                    root,
                    path: location.clone().unwrap_or_else(|| PathBuf::from(&original)),
                    original,
                    location,
                    overrides: own
                        .into_iter()
                        .filter(|(relative, _)| !relative.is_empty())
                        .map(|(relative, path)| Override { relative, path })
                        .collect(),
                    elsewhere: elsewhere.remove(&root).unwrap_or_default(),
                }
            })
            .collect())
    }
    /// Renames this computer; its locations stay.
    pub fn rename_computer(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        ensure!(!name.is_empty(), "Name this computer");
        self.db.execute(
            "UPDATE computers SET name=? WHERE id=?",
            params![name, self.computer.id],
        )?;
        self.computer.name = name.into();
        Ok(())
    }
    /// This computer's name as the catalog has it.
    pub fn computer_name(&self) -> Result<String> {
        Ok(self
            .db
            .query_row(
                "SELECT name FROM computers WHERE id=?",
                [&self.computer.id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_else(|| self.computer.name.clone()))
    }
}
/// Clears `computer`'s location of `relative` in `root` and, in the same
/// transaction, the legacy mapping older releases read for it.
fn clear(db: &Connection, computer: &str, root: RootId, relative: &str) -> Result<()> {
    db.execute(
        "DELETE FROM folder_locations WHERE root=? AND relative_path=? AND computer=?",
        params![root, relative, computer],
    )?;
    if relative.is_empty() {
        db.execute("UPDATE roots SET mapped_path=NULL WHERE id=?", [root])?;
    } else {
        db.execute(
            "DELETE FROM folder_mappings WHERE folder IN
             (SELECT f.id FROM folders f JOIN folder_paths p ON p.folder=f.id
              WHERE f.root=? AND p.path=?)",
            params![root, relative],
        )?;
    }
    Ok(())
}
