//! What the file operation tests share: a catalog over synthetic files in a
//! temporary folder, a Trash that keeps files in a folder of its own, and a
//! disk that fails or "crashes" (panics, as a process ending would leave
//! things) at a chosen call. Never a real catalog or real photos.
use super::disk::{Disk, Real};
use super::*;
use crate::catalog::Catalog;
use crate::storage::Identity;
use crate::storage::fsops::{Entry, FsError};
use anyhow::Result;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Distinct synthetic bytes for a file, from its name.
pub(super) fn bytes(name: &str) -> Vec<u8> {
    let seed = name.bytes().fold(7u8, |a, b| a.wrapping_mul(31) ^ b);
    let mut out = name.as_bytes().to_vec();
    out.extend((0..2000u32).map(|i| (i as u8).wrapping_mul(seed) ^ seed));
    out
}

/// A catalog over one root folder, "trip", of synthetic files.
pub(super) struct Fx {
    pub dir: tempfile::TempDir,
    /// The root, canonical.
    pub root: PathBuf,
    pub catalog: PathBuf,
    pub cat: Catalog,
}

/// A catalog of `trip/` holding `files` (paths below it). A name ending in
/// `/` is a folder, given a photo `keep.dng` so the catalog has it.
pub(super) fn fixture(files: &[&str]) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("trip");
    std::fs::create_dir_all(&root).unwrap();
    for f in files {
        let path = root.join(f);
        if f.ends_with('/') {
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("keep.dng"), bytes(&format!("{f}keep.dng"))).unwrap();
        } else {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes(f)).unwrap();
        }
    }
    let catalog = dir.path().join("Photos.rawmakase");
    let mut cat = Catalog::create(&catalog).unwrap();
    cat.add_folder(&root).unwrap();
    Fx {
        dir,
        root,
        catalog,
        cat,
    }
}

impl Fx {
    /// The master named `name` (its path below the root).
    pub fn photo(&self, name: &str) -> PhotoId {
        let path = self.root.join(name);
        self.cat
            .photos()
            .unwrap()
            .into_iter()
            .find(|p| p.master.is_none() && p.path == path)
            .unwrap_or_else(|| panic!("no photo {name}"))
            .id
    }
    /// The catalog folder at `relative` below the root ("" is the root).
    pub fn folder(&self, relative: &str) -> FolderId {
        let path = if relative.is_empty() {
            self.root.clone()
        } else {
            self.root.join(relative)
        };
        self.cat
            .folders()
            .unwrap()
            .into_iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("no folder {relative}"))
            .id
    }
    /// Every file below the folder the fixture is in, by path relative to
    /// it, with its bytes: where every original is.
    pub fn files(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let base = self.dir.path().canonicalize().unwrap();
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, base, out);
                } else {
                    let name = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                    if !name.starts_with("Photos.rawmakase") {
                        out.insert(name.replace('\\', "/"), std::fs::read(&p).unwrap());
                    }
                }
            }
        }
        walk(&base, &base, &mut out);
        out
    }
    /// Where each photo row says its file is, relative to the root.
    pub fn rows(&self) -> BTreeMap<PhotoId, String> {
        self.cat
            .photos()
            .unwrap()
            .into_iter()
            .map(|p| {
                let rel = p.path.strip_prefix(&self.root).unwrap_or(&p.path);
                (p.id, rel.to_string_lossy().replace('\\', "/"))
            })
            .collect()
    }
    /// A photo row `filename` in folder `logical` of the root added at
    /// `root` (a new root when there is none), written as an older release
    /// or another tool might have: no checks, no file read.
    pub fn add_row(&mut self, root: &Path, logical: &str, filename: &str) -> PhotoId {
        use crate::catalog::db::{Reads, sql};
        let original = root.to_str().unwrap().to_string();
        let path = root.join(logical).join(filename);
        let path = path.to_str().unwrap().to_string();
        self.cat
            .db
            .write(|w| {
                let found: Option<crate::catalog::RootId> = w.read_optional(
                    sql!("SELECT id FROM roots WHERE original_path=?"),
                    &[&original],
                )?;
                let root = match found {
                    Some(root) => root,
                    None => w.insert_returning_id(
                        sql!("INSERT INTO roots(original_path) VALUES (?) RETURNING id"),
                        &[&original],
                    )?,
                };
                let folder = crate::catalog::ingest::folder_id(w, root, logical)?;
                w.insert_returning_id(
                    sql!(
                        "INSERT INTO photos(folder, filename, original_path) VALUES (?, ?, ?)
                         RETURNING id"
                    ),
                    &[&folder, &filename, &path],
                )
            })
            .unwrap()
    }
    /// Opens the catalog again, as a restart does.
    pub fn reopen(&mut self) {
        self.cat = Catalog::open(&self.catalog).unwrap();
    }
    pub fn plan(&self, op: FileOp) -> Plan {
        plan(&self.cat, op).unwrap()
    }
    /// Plans, confirms and executes `op` with `trash` on `disk`.
    pub fn run(&mut self, op: FileOp, trash: &dyn Trash, disk: &dyn Disk) -> Outcome {
        let plan = self.plan(op);
        execute::execute_with(
            &mut self.cat,
            plan.confirm(),
            trash,
            &AtomicBool::new(false),
            disk,
        )
        .unwrap()
    }
    /// Recovers on `disk`.
    pub fn recover(&mut self, disk: &dyn Disk) -> Recovery {
        let _lock = lock::acquire(&self.cat).unwrap();
        recover::run(&mut self.cat, disk).unwrap()
    }
    /// Installs trigger `body` (after its name) for the connection's life.
    pub fn fail_journal_with(&self, body: &str) {
        self.cat
            .db_for_tests()
            .execute_batch(&format!("CREATE TEMP TRIGGER injected {body}"))
            .unwrap();
    }
    /// Installs a trigger making `when` fail, as a full disk would: for
    /// the connection's life, so a reopen clears it.
    pub fn fail_journal(&self, when: &str) {
        self.cat
            .db_for_tests()
            .execute_batch(&format!(
                "CREATE TEMP TRIGGER fail_journal {when} BEGIN SELECT RAISE(FAIL, 'disk full'); END;"
            ))
            .unwrap();
    }
}

/// A Trash that keeps files in `<fixture>/trash/`, under a unique name.
pub(super) struct FakeTrash {
    pub dir: PathBuf,
    /// Fails from the `n`-th file on (from 1), if set.
    pub fail_from: Option<usize>,
    /// The process ends right after the `n`-th file went (from 1), if set.
    pub crash_after: Option<usize>,
    pub calls: RefCell<usize>,
}

impl FakeTrash {
    pub fn new(fx: &Fx) -> Self {
        let dir = fx.dir.path().canonicalize().unwrap().join("trash");
        std::fs::create_dir_all(&dir).unwrap();
        Self {
            dir,
            fail_from: None,
            crash_after: None,
            calls: RefCell::new(0),
        }
    }
}

impl Trash for FakeTrash {
    fn trash(&self, path: &Path) -> Result<TrashReceipt, TrashError> {
        let n = {
            let mut calls = self.calls.borrow_mut();
            *calls += 1;
            *calls
        };
        if self.fail_from.is_some_and(|from| n >= from) {
            return Err(TrashError::Unsupported);
        }
        let location = self.dir.join(format!(
            "{n}-{}",
            path.file_name().unwrap().to_string_lossy()
        ));
        crate::storage::fsops::rename_exclusive(path, &location)
            .map_err(|e| TrashError::Failed(e.to_string()))?;
        if self.crash_after == Some(n) {
            panic!("injected crash after trashing {n}");
        }
        Ok(TrashReceipt { location })
    }
}

/// A Trash that is never used.
pub(super) struct NoTrash;
impl Trash for NoTrash {
    fn trash(&self, _: &Path) -> Result<TrashReceipt, TrashError> {
        panic!("nothing should be trashed")
    }
}

/// The calls a test disk wrapping another (as field `$inner`) passes on
/// unchanged, so each needn't spell them out.
macro_rules! passes_on {
    ($inner:tt) => {
        fn volume_facts(
            &self,
            path: &std::path::Path,
        ) -> Result<crate::storage::fsops::VolumeFacts, crate::storage::fsops::FsError> {
            self.$inner.volume_facts(path)
        }
    };
}
pub(super) use passes_on;

/// A disk call, as recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Call {
    Entry(PathBuf),
    List(PathBuf),
    Identity(PathBuf),
    Rename(PathBuf, PathBuf),
    CreateDir(PathBuf),
    Sync(PathBuf),
}

/// Which kind of call a fault is at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Op {
    Rename,
    CreateDir,
    Sync,
}

/// What happens at the faulty call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fault {
    /// It fails, changing nothing.
    Fail,
    /// The process ends before it.
    CrashBefore,
    /// The process ends right after it.
    CrashAfter,
}

/// The real disk, recording every call and faulting at the `nth` (from 0)
/// call of a kind, for each fault it has.
#[derive(Default)]
pub(super) struct Faulty {
    pub faults: Vec<(Op, usize, Fault)>,
    pub calls: RefCell<Vec<Call>>,
    seen: RefCell<BTreeMap<String, usize>>,
}

impl Faulty {
    pub fn at(op: Op, nth: usize, fault: Fault) -> Self {
        Self::with(&[(op, nth, fault)])
    }
    pub fn with(faults: &[(Op, usize, Fault)]) -> Self {
        Self {
            faults: faults.to_vec(),
            ..Self::default()
        }
    }
    /// How many calls of `op` it saw.
    pub fn count(&self, op: Op) -> usize {
        self.seen
            .borrow()
            .get(&format!("{op:?}"))
            .copied()
            .unwrap_or(0)
    }
    /// Runs `step`, a call of kind `op`, with the fault if this is the one.
    fn step<T>(&self, op: Op, step: impl FnOnce() -> Result<T, FsError>) -> Result<T, FsError> {
        let n = {
            let mut seen = self.seen.borrow_mut();
            let n = seen.entry(format!("{op:?}")).or_default();
            *n += 1;
            *n - 1
        };
        let fault = self
            .faults
            .iter()
            .find(|(at, nth, _)| *at == op && *nth == n)
            .map(|f| f.2);
        match fault {
            Some(fault) => match fault {
                Fault::Fail => Err(FsError::Io(std::io::Error::other("injected failure"))),
                Fault::CrashBefore => panic!("injected crash before {op:?} {n}"),
                Fault::CrashAfter => {
                    let _done = step();
                    panic!("injected crash after {op:?} {n}")
                }
            },
            _ => step(),
        }
    }
    pub fn renames(&self) -> Vec<(PathBuf, PathBuf)> {
        self.calls
            .borrow()
            .iter()
            .filter_map(|c| match c {
                Call::Rename(a, b) => Some((a.clone(), b.clone())),
                _ => None,
            })
            .collect()
    }
}

impl Disk for Faulty {
    fn entry(&self, path: &Path) -> Result<Entry, FsError> {
        self.calls.borrow_mut().push(Call::Entry(path.into()));
        Real.entry(path)
    }
    fn list(&self, dir: &Path) -> Result<Vec<OsString>, FsError> {
        self.calls.borrow_mut().push(Call::List(dir.into()));
        Real.list(dir)
    }
    fn identity(&self, path: &Path) -> anyhow::Result<Identity> {
        self.calls.borrow_mut().push(Call::Identity(path.into()));
        Real.identity(path)
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<(), FsError> {
        self.calls
            .borrow_mut()
            .push(Call::Rename(from.into(), to.into()));
        self.step(Op::Rename, || Real.rename(from, to))
    }
    fn create_dir(&self, path: &Path) -> Result<(), FsError> {
        self.calls.borrow_mut().push(Call::CreateDir(path.into()));
        self.step(Op::CreateDir, || Real.create_dir(path))
    }
    fn sync_dir(&self, dir: &Path) -> Result<(), FsError> {
        self.calls.borrow_mut().push(Call::Sync(dir.into()));
        self.step(Op::Sync, || Real.sync_dir(dir))
    }
    fn canonical(&self, path: &Path) -> Result<PathBuf, FsError> {
        Real.canonical(path)
    }
    fn volume_facts(&self, path: &Path) -> Result<crate::storage::fsops::VolumeFacts, FsError> {
        Real.volume_facts(path)
    }
}

/// Runs `f`, which may "crash", and says whether it did.
pub(super) fn crashes(f: impl FnOnce()) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err()
}

/// The results of `outcome`'s items, in order.
pub(super) fn results(outcome: &Outcome) -> Vec<ItemResult> {
    outcome.items.iter().map(|i| i.result.clone()).collect()
}

#[test]
fn a_crash_is_a_panic_the_tests_survive() -> Result<()> {
    assert!(crashes(|| panic!("injected")));
    assert!(!crashes(|| {}));
    Ok(())
}
