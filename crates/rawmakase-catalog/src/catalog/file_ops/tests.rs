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
    /// Plans `op` with the volumes and folders as `disk` sees them.
    pub fn plan_on(&self, op: FileOp, disk: &dyn Disk) -> Result<Plan, FileOpError> {
        plan::plan_with(&self.cat, op, disk)
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
        self.recover_with(disk, &NoTrash)
    }
    /// Recovers on `disk`, with `trash`.
    pub fn recover_with(&mut self, disk: &dyn Disk, trash: &dyn Trash) -> Recovery {
        let _lock = lock::acquire(&self.cat).unwrap();
        recover::run(&mut self.cat, disk, trash).unwrap()
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
        fn create_file(
            &self,
            path: &std::path::Path,
        ) -> Result<(), crate::storage::fsops::FsError> {
            self.$inner.create_file(path)
        }
        fn copy_into(
            &self,
            from: &std::path::Path,
            to: &std::path::Path,
            to_id: crate::storage::fsops::FileId,
            go_on: &mut dyn FnMut(u64) -> bool,
        ) -> Result<crate::storage::fsops::ContentHash, crate::storage::fsops::CopyError> {
            self.$inner.copy_into(from, to, to_id, go_on)
        }
        fn sync_file(&self, path: &std::path::Path) -> Result<(), crate::storage::fsops::FsError> {
            self.$inner.sync_file(path)
        }
        fn copy_times(
            &self,
            from: &std::path::Path,
            to: &std::path::Path,
        ) -> Result<(), crate::storage::fsops::FsError> {
            self.$inner.copy_times(from, to)
        }
        fn hash(
            &self,
            path: &std::path::Path,
        ) -> Result<crate::storage::fsops::ContentHash, crate::storage::fsops::FsError> {
            self.$inner.hash(path)
        }
        fn remove_file(
            &self,
            path: &std::path::Path,
        ) -> Result<(), crate::storage::fsops::FsError> {
            self.$inner.remove_file(path)
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
    CreateFile(PathBuf),
    Copy(PathBuf, PathBuf),
    /// A chunk of a copy written, with the bytes written so far.
    Chunk(u64),
    SyncFile(PathBuf),
    CopyTimes(PathBuf),
    Hash(PathBuf),
    RemoveFile(PathBuf),
}

/// Which kind of call a fault is at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Op {
    Rename,
    CreateDir,
    Sync,
    CreateFile,
    Copy,
    SyncFile,
    CopyTimes,
    Hash,
    RemoveFile,
}

impl Op {
    /// The steps of a copy to another volume.
    pub const COPYING: [Op; 8] = [
        Op::CreateFile,
        Op::Copy,
        Op::SyncFile,
        Op::CopyTimes,
        Op::Hash,
        Op::Rename,
        Op::Sync,
        Op::RemoveFile,
    ];
}

/// What happens at the faulty call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fault {
    /// It fails, changing nothing; a copy fails part way, as on a full
    /// drive.
    Fail,
    /// The process ends before it.
    CrashBefore,
    /// The process ends right after it.
    CrashAfter,
    /// The process ends in the middle of a copy, part of it written; before
    /// any other call.
    CrashMid,
}

/// The real disk, recording every call and faulting at the `nth` (from 0)
/// call of a kind, for each fault it has; and, for copies, numbering what
/// is at or below `elsewhere` as if on another volume, and changing what
/// the hooks ask for.
#[derive(Default)]
pub(super) struct Faulty {
    pub faults: Vec<(Op, usize, Fault)>,
    pub calls: RefCell<Vec<Call>>,
    pub seen: RefCell<BTreeMap<String, usize>>,
    /// A folder taken for another volume's, with everything in it.
    pub elsewhere: Option<PathBuf>,
    /// The bytes every volume has free, instead of its own.
    pub space: Option<u64>,
    /// The step, in nanoseconds, that a copy's time is cut to, as a volume
    /// keeping times more coarsely does.
    pub coarse: Option<u64>,
    /// Each copy reads back with a byte changed.
    pub corrupt: bool,
    /// The original gets another byte while it is copied.
    pub change_source: bool,
    /// The original is taken away to this path while it is copied.
    pub take_source: Option<PathBuf>,
    /// A file there takes this name just before something is renamed to it.
    pub occupy: Option<PathBuf>,
    /// Set once a copy wrote its first chunk, as Stop pressed then.
    pub stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// `elsewhere` keeps the names it is given decomposed (NFD), as HFS+
    /// does, and finds a file by any normalisation form of its name.
    pub decompose: bool,
    /// `elsewhere` gives a file a new number once it is written and closed,
    /// as FAT does.
    pub renumber_on_close: bool,
    /// `elsewhere` gives a file a new number when it is renamed.
    pub renumber_on_rename: bool,
    /// The number `elsewhere` gives each real file object, where it gave
    /// it another than its own.
    pub numbers: RefCell<BTreeMap<crate::storage::fsops::FileId, crate::storage::fsops::FileId>>,
    pub renumbered: RefCell<u64>,
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
    /// This disk with `dir` taken for another volume.
    pub fn elsewhere(mut self, dir: &Path) -> Self {
        self.elsewhere = Some(dir.to_path_buf());
        self
    }
    /// How many calls of `op` it saw.
    pub fn count(&self, op: Op) -> usize {
        self.seen
            .borrow()
            .get(&format!("{op:?}"))
            .copied()
            .unwrap_or(0)
    }
    /// The fault at this call of kind `op`, if any, with its number.
    fn fault(&self, op: Op) -> (usize, Option<Fault>) {
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
        (n, fault)
    }
    /// Runs `step`, a call of kind `op`, with the fault if this is the one.
    fn step<T>(&self, op: Op, step: impl FnOnce() -> Result<T, FsError>) -> Result<T, FsError> {
        let (n, fault) = self.fault(op);
        match fault {
            Some(fault) => match fault {
                Fault::Fail => Err(FsError::Io(std::io::Error::other("injected failure"))),
                Fault::CrashBefore | Fault::CrashMid => panic!("injected crash before {op:?} {n}"),
                Fault::CrashAfter => {
                    let _done = step();
                    panic!("injected crash after {op:?} {n}")
                }
            },
            _ => step(),
        }
    }
    /// Whether `path` is taken for another volume's.
    fn far(&self, path: &Path) -> bool {
        self.elsewhere.as_ref().is_some_and(|d| path.starts_with(d))
    }
    /// `path` as `elsewhere` stores it when it makes a name there.
    fn stored(&self, path: &Path) -> PathBuf {
        use unicode_normalization::UnicodeNormalization;
        match (self.decompose && self.far(path), path.file_name()) {
            (true, Some(name)) => {
                let nfd: String = name.to_string_lossy().nfd().collect();
                path.with_file_name(nfd)
            }
            _ => path.to_path_buf(),
        }
    }
    /// The file `path` names on `elsewhere`, which finds a name in any
    /// normalisation form.
    fn found(&self, path: &Path) -> PathBuf {
        let stored = self.stored(path);
        match std::fs::symlink_metadata(path).is_err() && stored.symlink_metadata().is_ok() {
            true => stored,
            false => path.to_path_buf(),
        }
    }
    /// The number `elsewhere` gives real file object `real`.
    fn number(&self, real: crate::storage::fsops::FileId) -> crate::storage::fsops::FileId {
        self.numbers
            .borrow()
            .get(&real)
            .copied()
            .unwrap_or_else(|| real.elsewhere())
    }
    /// The real file object `elsewhere` numbers `fake`.
    fn real(&self, fake: crate::storage::fsops::FileId) -> crate::storage::fsops::FileId {
        self.numbers
            .borrow()
            .iter()
            .find(|(_, f)| **f == fake)
            .map(|(r, _)| *r)
            .unwrap_or_else(|| fake.elsewhere())
    }
    /// Gives the file at `path`, if `elsewhere`, a number never given yet.
    fn renumber(&self, path: &Path) {
        if !self.far(path) {
            return;
        }
        let Ok(real) = Real.entry(path).map(|e| e.id) else {
            return;
        };
        let n = {
            let mut n = self.renumbered.borrow_mut();
            *n += 1;
            u64::MAX - *n
        };
        let fake = real.elsewhere().numbered(n);
        self.numbers.borrow_mut().insert(real, fake);
    }
    /// A remount of `elsewhere`: each of `files` gets a new number, and the
    /// file at `reuse.1` the number the file at `reuse.0` had before.
    pub fn remount(&self, files: &[&Path], reuse: Option<(&Path, &Path)>) {
        let before = reuse.map(|(a, _)| self.number(Real.entry(a).unwrap().id));
        for f in files {
            self.renumber(f);
        }
        if let (Some((_, b)), Some(old)) = (reuse, before) {
            let real = Real.entry(b).unwrap().id;
            self.numbers.borrow_mut().insert(real, old);
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
    /// How many chunks its copies wrote.
    pub fn chunks(&self) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|c| matches!(c, Call::Chunk(_)))
            .count()
    }
}

impl Disk for Faulty {
    fn entry(&self, path: &Path) -> Result<Entry, FsError> {
        self.calls.borrow_mut().push(Call::Entry(path.into()));
        let mut found = Real.entry(&self.found(path))?;
        if self.far(path) {
            found.id = self.number(found.id);
        }
        Ok(found)
    }
    fn list(&self, dir: &Path) -> Result<Vec<OsString>, FsError> {
        self.calls.borrow_mut().push(Call::List(dir.into()));
        Real.list(dir)
    }
    fn identity(&self, path: &Path) -> anyhow::Result<Identity> {
        self.calls.borrow_mut().push(Call::Identity(path.into()));
        Real.identity(&self.found(path))
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<(), FsError> {
        self.calls
            .borrow_mut()
            .push(Call::Rename(from.into(), to.into()));
        if self.occupy.as_deref() == Some(to) {
            std::fs::write(to, b"raced").unwrap();
        }
        let (from, to) = (&self.found(from), &self.stored(to));
        let renamed = self.step(Op::Rename, || Real.rename(from, to));
        if renamed.is_ok() && self.renumber_on_rename {
            self.renumber(to);
        }
        renamed
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
        let mut facts = Real.volume_facts(path)?;
        if let Some(space) = self.space {
            facts.available = space;
        }
        Ok(facts)
    }
    fn create_file(&self, path: &Path) -> Result<(), FsError> {
        self.calls.borrow_mut().push(Call::CreateFile(path.into()));
        self.step(Op::CreateFile, || Real.create_file(&self.stored(path)))
    }
    fn copy_into(
        &self,
        from: &Path,
        to: &Path,
        to_id: crate::storage::fsops::FileId,
        go_on: &mut dyn FnMut(u64) -> bool,
    ) -> Result<crate::storage::fsops::ContentHash, crate::storage::fsops::CopyError> {
        use crate::storage::fsops::CopyError;
        self.calls
            .borrow_mut()
            .push(Call::Copy(from.into(), to.into()));
        let to = &self.found(to);
        // The file object as the real disk numbers it.
        let to_id = match self.far(to) {
            true => self.real(to_id),
            false => to_id,
        };
        let (n, fault) = self.fault(Op::Copy);
        // Part of it: the first chunk.
        let part = || Real.copy_into(from, to, to_id, &mut |_| false);
        match fault {
            Some(Fault::CrashBefore) => panic!("injected crash before copy {n}"),
            Some(Fault::Fail) => {
                let _part = part();
                return Err(CopyError::Fs(FsError::NoSpace));
            }
            Some(Fault::CrashMid) => {
                let _part = part();
                panic!("injected crash in copy {n}")
            }
            Some(Fault::CrashAfter) | None => {}
        }
        let copied = Real.copy_into(from, to, to_id, &mut |written| {
            self.calls.borrow_mut().push(Call::Chunk(written));
            if let Some(stop) = &self.stop {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            go_on(written)
        });
        if copied.is_ok() && self.corrupt {
            let mut bytes = std::fs::read(to).unwrap();
            bytes[0] ^= 0xff;
            std::fs::write(to, bytes).unwrap();
        }
        if let (true, Some(away)) = (copied.is_ok(), &self.take_source) {
            std::fs::rename(from, away).unwrap();
        }
        if copied.is_ok() && self.change_source {
            let mut bytes = std::fs::read(from).unwrap();
            bytes.push(b'!');
            std::fs::write(from, bytes).unwrap();
        }
        // Closed after writing.
        if self.renumber_on_close {
            self.renumber(to);
        }
        if fault == Some(Fault::CrashAfter) {
            panic!("injected crash after copy {n}");
        }
        copied
    }
    fn sync_file(&self, path: &Path) -> Result<(), FsError> {
        self.calls.borrow_mut().push(Call::SyncFile(path.into()));
        self.step(Op::SyncFile, || Real.sync_file(&self.found(path)))
    }
    fn copy_times(&self, from: &Path, to: &Path) -> Result<(), FsError> {
        self.calls.borrow_mut().push(Call::CopyTimes(to.into()));
        let to = &self.found(to);
        self.step(Op::CopyTimes, || {
            Real.copy_times(from, to)?;
            if let Some(step) = self.coarse {
                let at = std::fs::metadata(to)?.modified()?;
                let ns = at.duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
                let cut = ns - ns % u128::from(step);
                let cut = std::time::UNIX_EPOCH
                    + std::time::Duration::new(
                        (cut / 1_000_000_000) as u64,
                        (cut % 1_000_000_000) as u32,
                    );
                std::fs::File::options()
                    .write(true)
                    .open(to)?
                    .set_modified(cut)?;
            }
            Ok(())
        })
    }
    fn hash(&self, path: &Path) -> Result<crate::storage::fsops::ContentHash, FsError> {
        self.calls.borrow_mut().push(Call::Hash(path.into()));
        self.step(Op::Hash, || Real.hash(&self.found(path)))
    }
    fn remove_file(&self, path: &Path) -> Result<(), FsError> {
        self.calls.borrow_mut().push(Call::RemoveFile(path.into()));
        self.step(Op::RemoveFile, || Real.remove_file(&self.found(path)))
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
