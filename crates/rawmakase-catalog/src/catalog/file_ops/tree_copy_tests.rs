//! Moving a folder to another drive (#572 C): its whole tree copied into a
//! hidden folder there, every file verified, then named in one exclusive
//! rename; the catalog follows; the originals go to the Trash. Anything
//! that fails before the catalog follows is undone, and recovery decides
//! whatever a crash leaves at any step. On one volume, the test disk takes
//! `far/` for another volume's.
use super::recover_tests::{before, check};
use super::tests::{FakeTrash, Fault, Faulty, Fx, NoTrash, Op, bytes, crashes, fixture, results};
use super::*;
use crate::storage::fsops;
use anyhow::Result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// `day/` holds a photo with its sidecar, a hidden file, an empty folder
/// and a folder with a photo; `far/` is a catalog folder on "another drive".
fn files() -> Fx {
    let fx = fixture(&[
        "day/a.NEF",
        "day/a.xmp",
        "day/.hidden",
        "day/sub/b.NEF",
        "far/",
    ]);
    std::fs::create_dir(fx.root.join("day/empty")).unwrap();
    fx
}

/// The least that has every kind of step: a hidden file, and a photo in a
/// folder of the folder.
fn small() -> Fx {
    fixture(&["day/a.NEF", "day/.hidden", "day/sub/b.NEF", "far/"])
}

fn far(fx: &Fx) -> Faulty {
    Faulty::default().elsewhere(&fx.root.join("far"))
}

fn to_far(fx: &Fx) -> FileOp {
    FileOp::MoveFolder {
        root: fx.root_id(),
        folder: "day".into(),
        to_root: fx.root_id(),
        parent: "far".into(),
    }
}

/// Plans `op` on `disk` and runs it, stopping when `cancel` is set.
fn run(fx: &mut Fx, op: FileOp, trash: &dyn Trash, disk: &Faulty, cancel: &AtomicBool) -> Outcome {
    let plan = fx.plan_on(op, disk).unwrap();
    execute::execute_with(&mut fx.cat, plan.confirm(), trash, cancel, disk).unwrap()
}

/// Every name below `dir`, by path relative to it, with a file's bytes
/// (`None` for a folder).
fn tree_of(dir: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        let Ok(names) = std::fs::read_dir(dir) else {
            return;
        };
        for e in names {
            let p = e.unwrap().path();
            let rel = p
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if p.is_dir() {
                out.insert(rel, None);
                walk(&p, base, out);
            } else {
                out.insert(rel, Some(std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// Partial copies or hidden folders a copy made, anywhere but the Trash.
fn partials(fx: &Fx) -> Vec<String> {
    tree_of(fx.dir.path())
        .into_keys()
        .filter(|k| k.contains(".rawmakase-part") && !k.starts_with("trash/"))
        .collect()
}

/// The logical path of catalog folder `folder`.
fn logical(fx: &Fx, folder: FolderId) -> String {
    fx.cat
        .db_for_tests()
        .query_row(
            "SELECT path FROM folder_paths WHERE folder=?1",
            [folder.0],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn a_folder_moves_to_another_drive_by_a_verified_copy_and_its_original_goes_to_the_trash()
-> Result<()> {
    let mut fx = files();
    let (a, b) = (fx.photo("day/a.NEF"), fx.photo("day/sub/b.NEF"));
    let (day, sub) = (fx.folder("day"), fx.folder("day/sub"));
    let tree = tree_of(&fx.root.join("day"));
    let modified = std::fs::metadata(fx.root.join("day/a.NEF"))?.modified()?;
    let disk = far(&fx);
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    let across = plan.summary().across.clone().expect("it copies");
    assert_eq!(across.files, 4);
    let bytes_planned: u64 = ["day/a.NEF", "day/a.xmp", "day/.hidden", "day/sub/b.NEF"]
        .iter()
        .map(|f| bytes(f).len() as u64)
        .sum();
    assert_eq!(across.bytes, bytes_planned);
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &trash,
        &AtomicBool::new(false),
        &disk,
    )?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert!(outcome.items[0].originals_kept.is_empty(), "{outcome:?}");
    assert_eq!(tree_of(&fx.root.join("far/day")), tree);
    assert!(!fx.root.join("day").exists());
    // The original went to the Trash whole, as one folder.
    assert_eq!(tree_of(&trash.dir.join("1-day")), tree);
    let rows = fx.rows();
    assert_eq!(rows[&a], "far/day/a.NEF");
    assert_eq!(rows[&b], "far/day/sub/b.NEF");
    assert_eq!(logical(&fx, day), "far/day");
    assert_eq!(logical(&fx, sub), "far/day/sub");
    assert_eq!(
        std::fs::metadata(fx.root.join("far/day/a.NEF"))?.modified()?,
        modified
    );
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    // Copied, never renamed across: the one rename is the hidden folder's.
    let renames = disk.renames();
    assert_eq!(renames.len(), 1, "{renames:?}");
    assert!(renames[0].0.to_string_lossy().ends_with(".rawmakase-part"));
    Ok(())
}

/// The plan's refusal, as the user reads it.
fn refusal(fx: &Fx, disk: &Faulty) -> String {
    match fx.plan_on(to_far(fx), disk) {
        Err(e @ (FileOpError::Destination(_) | FileOpError::NoSpace { .. })) => e.to_string(),
        other => panic!("not refused: {other:?}"),
    }
}

#[test]
fn a_folder_holding_what_can_t_be_copied_or_proven_is_refused() -> Result<()> {
    let fx = files();
    // Not enough room there.
    let disk = Faulty {
        space: Some(100),
        ..far(&fx)
    };
    assert!(
        refusal(&fx, &disk).contains("free"),
        "{}",
        refusal(&fx, &disk)
    );
    // A drive whose file ids can't be proven, either side.
    for unstable in [fx.root.join("far"), fx.root.clone()] {
        let disk = Faulty {
            unstable_ids: vec![unstable],
            ..far(&fx)
        };
        assert!(
            refusal(&fx, &disk).contains("this kind of drive"),
            "{}",
            refusal(&fx, &disk)
        );
    }
    // A file with another name elsewhere.
    std::fs::hard_link(fx.root.join("day/a.xmp"), fx.root.join("hard.xmp"))?;
    assert!(refusal(&fx, &far(&fx)).contains("another name"));
    std::fs::remove_file(fx.root.join("hard.xmp"))?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(fx.root.join("day/a.NEF"), fx.root.join("day/sub/link"))?;
        assert!(refusal(&fx, &far(&fx)).contains("link"));
    }
    assert!(fx.root.join("day/a.NEF").is_file());
    Ok(())
}

/// Plans the move, runs it on `disk` (which may crash) with `trash`,
/// restarts and recovers on the same volumes.
fn scenario(fx: &mut Fx, trash: &dyn Trash, disk: &Faulty) -> (BatchId, Recovery) {
    let plan = fx.plan_on(to_far(fx), disk).unwrap();
    let batch = plan.batch().clone();
    crashes(|| {
        drop(
            execute::execute_with(
                &mut fx.cat,
                plan.confirm(),
                trash,
                &Default::default(),
                disk,
            )
            .unwrap(),
        );
    });
    fx.reopen();
    let clean = far(fx);
    let recovery = fx.recover_with(&clean, trash);
    (batch, recovery)
}

/// After any fault and recovery: no original lost (`check`), and the
/// folder moved whole (copies there, originals in the Trash or reported) or
/// not at all (no copy left outside the Trash); what is unfinished is put
/// back in the end.
fn proven(
    fx: &mut Fx,
    tree: &BTreeMap<String, Option<Vec<u8>>>,
    recovery: &Recovery,
    batch: &BatchId,
    was: &super::recover_tests::Before,
    what: &str,
) {
    check(fx, was, recovery, batch);
    where_it_says(fx, tree, batch, what);
    let clean = far(fx);
    let trash = FakeTrash::new(fx);
    *trash.calls.borrow_mut() = 100;
    for u in &recovery.unfinished {
        let back = recover::resolve_with(
            &mut fx.cat,
            u.operation,
            Resolution::PutBack,
            &clean,
            &trash,
        )
        .unwrap();
        assert!(
            matches!(back.result, ItemResult::Unchanged(_)),
            "{what}: {back:?}"
        );
    }
    where_it_says(fx, tree, batch, what);
}

/// The folder as the outcome of `batch` says.
fn where_it_says(fx: &Fx, tree: &BTreeMap<String, Option<Vec<u8>>>, batch: &BatchId, what: &str) {
    let outcome = super::outcome(&fx.cat, batch).unwrap();
    let Some(item) = outcome.items.first() else {
        assert_eq!(&tree_of(&fx.root.join("day")), tree, "{what}");
        return;
    };
    match &item.result {
        ItemResult::FolderMoved => {
            assert_eq!(&tree_of(&fx.root.join("far/day")), tree, "{what}");
            if item.originals_kept.is_empty() {
                assert!(!fx.root.join("day").exists(), "{what}");
            }
            assert!(partials(fx).is_empty(), "{what}: {:?}", partials(fx));
        }
        ItemResult::Unchanged(_) => {
            assert_eq!(&tree_of(&fx.root.join("day")), tree, "{what}");
            assert!(!fx.root.join("far/day").exists(), "{what}");
            assert!(partials(fx).is_empty(), "{what}: {:?}", partials(fx));
        }
        ItemResult::NeedsRecovery(_) => {}
        other => panic!("{what}: {other:?}"),
    }
}

#[test]
fn every_fault_at_every_step_of_a_folder_copy_ends_proven() {
    let started = std::time::Instant::now();
    // A run without faults says how many steps there are.
    let mut clean = small();
    let counting = far(&clean);
    let trash = FakeTrash::new(&clean);
    let op = to_far(&clean);
    let outcome = run(&mut clean, op, &trash, &counting, &AtomicBool::new(false));
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(counting.count(Op::Copy), 3);
    // And one undone once two files are whole: a full drive at the last.
    let mut undone = small();
    let failing = Faulty {
        elsewhere: Some(undone.root.join("far")),
        ..Faulty::at(Op::Copy, 2, Fault::Fail)
    };
    let trash = FakeTrash::new(&undone);
    let op = to_far(&undone);
    let _ = run(&mut undone, op, &trash, &failing, &AtomicBool::new(false));
    let mut runs = 0;
    for op in [
        Op::CreateDir,
        Op::CreateFile,
        Op::Copy,
        Op::SyncFile,
        Op::CopyTimes,
        Op::Hash,
        Op::Sync,
        Op::Rename,
        Op::RemoveFile,
        Op::RemoveDir,
    ] {
        // Faults while undoing need the undoing run; the others its steps
        // before the fault are the clean run's.
        let undoing = matches!(op, Op::RemoveFile | Op::RemoveDir | Op::Sync);
        let count = counting.count(op).max(failing.count(op));
        for nth in 0..count {
            let faults: &[Fault] = match op {
                Op::Copy => &[
                    Fault::Fail,
                    Fault::CrashBefore,
                    Fault::CrashMid,
                    Fault::CrashAfter,
                ],
                _ => &[Fault::Fail, Fault::CrashBefore, Fault::CrashAfter],
            };
            for &fault in faults {
                for undo in [false, true] {
                    if undo && !undoing {
                        continue;
                    }
                    let mut fx = small();
                    let was = before(&fx);
                    let tree = tree_of(&fx.root.join("day"));
                    let mut with = vec![(op, nth, fault)];
                    if undo {
                        with.push((Op::Copy, 2, Fault::Fail));
                    }
                    let disk = Faulty {
                        elsewhere: Some(fx.root.join("far")),
                        ..Faulty::with(&with)
                    };
                    let trash = FakeTrash::new(&fx);
                    let (batch, recovery) = scenario(&mut fx, &trash, &disk);
                    let what = format!("{op:?} {nth} {fault:?} undo {undo}");
                    proven(&mut fx, &tree, &recovery, &batch, &was, &what);
                    runs += 1;
                }
            }
        }
    }
    // The Trash failing at, or crashing after, the first and second item.
    for n in 1..=2 {
        for crash in [false, true] {
            let mut fx = small();
            let was = before(&fx);
            let tree = tree_of(&fx.root.join("day"));
            let disk = far(&fx);
            let mut trash = FakeTrash::new(&fx);
            match crash {
                true => trash.crash_after = Some(n),
                false => trash.fail_from = Some(n),
            }
            let (batch, recovery) = scenario(&mut fx, &trash, &disk);
            proven(
                &mut fx,
                &tree,
                &recovery,
                &batch,
                &was,
                &format!("trash {n} {crash}"),
            );
            runs += 1;
        }
    }
    assert!(runs >= 100, "{runs}");
    eprintln!("{runs} runs in {:?}", started.elapsed());
}

#[test]
fn every_failed_journal_write_of_a_folder_copy_ends_proven() -> Result<()> {
    for when in [
        "BEFORE INSERT ON file_operation_copies",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'copying'",
        "BEFORE UPDATE ON file_operation_copies WHEN NEW.content IS NOT NULL",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'copied'",
        "BEFORE UPDATE ON folders",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'committed'",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'trashed'",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'done'",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'discarded'",
    ] {
        let mut fx = files();
        let was = before(&fx);
        let tree = tree_of(&fx.root.join("day"));
        fx.fail_journal(when);
        let disk = far(&fx);
        let trash = FakeTrash::new(&fx);
        let plan = fx.plan_on(to_far(&fx), &disk)?;
        let batch = plan.batch().clone();
        let _ = execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &trash,
            &Default::default(),
            &disk,
        );
        fx.reopen();
        let recovery = fx.recover_with(&far(&fx), &trash);
        assert!(recovery.unfinished.is_empty(), "{when}: {recovery:?}");
        proven(&mut fx, &tree, &recovery, &batch, &was, when);
    }
    Ok(())
}

#[test]
fn stop_stops_a_folder_copy_and_undoes_it() -> Result<()> {
    let mut fx = files();
    let tree = tree_of(&fx.root.join("day"));
    let rows = fx.rows();
    let stop = Arc::new(AtomicBool::new(false));
    let disk = Faulty {
        stop: Some(stop.clone()),
        ..far(&fx)
    };
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &NoTrash, &disk, &stop);
    let [ItemResult::Unchanged(Some(why))] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert_eq!(why, "Stopped.");
    assert_eq!(fx.rows(), rows);
    assert_eq!(tree_of(&fx.root.join("day")), tree);
    assert!(!fx.root.join("far/day").exists());
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    Ok(())
}

#[test]
fn a_name_taken_just_before_the_folder_gets_it_is_never_replaced() -> Result<()> {
    let mut fx = files();
    let tree = tree_of(&fx.root.join("day"));
    let disk = Faulty {
        occupy: Some(fx.root.join("far/day")),
        ..far(&fx)
    };
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &NoTrash, &disk, &AtomicBool::new(false));
    assert!(
        matches!(&results(&outcome)[..], [ItemResult::Unchanged(Some(_))]),
        "{outcome:?}"
    );
    assert_eq!(std::fs::read(fx.root.join("far/day"))?, b"raced");
    assert_eq!(tree_of(&fx.root.join("day")), tree);
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    Ok(())
}

#[test]
fn an_original_changed_while_copied_keeps_its_whole_copy_until_put_back() -> Result<()> {
    let mut fx = files();
    let rows = fx.rows();
    let disk = Faulty {
        change_source: true,
        ..far(&fx)
    };
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, &AtomicBool::new(false));
    let [ItemResult::NeedsRecovery(_)] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert_eq!(fx.rows(), rows);
    // The copy of the original as it was is the one place those bytes are.
    assert!(!partials(&fx).is_empty());
    let id = outcome.items[0].operation;
    let back = {
        let clean = far(&fx);
        recover::resolve_with(&mut fx.cat, id, Resolution::PutBack, &clean, &trash)
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(matches!(back.result, ItemResult::Unchanged(_)), "{back:?}");
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    assert!(!fx.root.join("far/day").exists());
    let first = fx
        .files()
        .into_iter()
        .any(|(k, v)| k.starts_with("trash/") && v == bytes("day/.hidden"));
    assert!(first, "the copy of the original as it was is in the Trash");
    Ok(())
}

#[test]
fn a_file_added_to_the_copy_before_recovery_keeps_it_for_the_user() -> Result<()> {
    let mut fx = files();
    let tree = tree_of(&fx.root.join("day"));
    let disk = Faulty {
        elsewhere: Some(fx.root.join("far")),
        ..Faulty::at(Op::Rename, 0, Fault::CrashAfter)
    };
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    let batch = plan.batch().clone();
    assert!(crashes(|| {
        drop(execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &trash,
            &Default::default(),
            &disk,
        ));
    }));
    std::fs::write(fx.root.join("far/day/theirs.txt"), b"theirs")?;
    fx.reopen();
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert_eq!(recovery.unfinished.len(), 1, "{recovery:?}");
    let u = recovery.unfinished[0].operation;
    let back = {
        let clean = far(&fx);
        recover::resolve_with(&mut fx.cat, u, Resolution::PutBack, &clean, &trash)
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(
        matches!(back.result, ItemResult::NeedsRecovery(_)),
        "{back:?}"
    );
    assert_eq!(
        std::fs::read(fx.root.join("far/day/theirs.txt"))?,
        b"theirs"
    );
    assert_eq!(tree_of(&fx.root.join("day")), tree);
    let kept = {
        let clean = far(&fx);
        recover::resolve_with(&mut fx.cat, u, Resolution::Keep, &clean, &trash)
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert_eq!(kept.result, ItemResult::Kept);
    assert_eq!(
        results(&super::outcome(&fx.cat, &batch)?),
        [ItemResult::Kept]
    );
    Ok(())
}

#[test]
fn originals_changed_or_added_to_after_the_catalog_follows_stay_and_are_reported() -> Result<()> {
    let mut fx = files();
    let a = fx.photo("day/a.NEF");
    let disk = far(&fx);
    let mut trash = FakeTrash::new(&fx);
    trash.crash_before = Some(1);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    let batch = plan.batch().clone();
    assert!(crashes(|| {
        drop(execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &trash,
            &Default::default(),
            &disk,
        ));
    }));
    // Meanwhile one original changes and a file is added beside them.
    std::fs::write(fx.root.join("day/a.xmp"), b"edited since")?;
    std::fs::write(fx.root.join("day/sub/new.txt"), b"new")?;
    fx.reopen();
    trash.crash_before = None;
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    let outcome = super::outcome(&fx.cat, &batch)?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(fx.rows()[&a], "far/day/a.NEF");
    // Proven originals went one by one; the changed one, the new file and
    // the folders holding them stay, reported.
    assert!(!fx.root.join("day/a.NEF").exists());
    assert!(!fx.root.join("day/sub/b.NEF").exists());
    assert_eq!(std::fs::read(fx.root.join("day/a.xmp"))?, b"edited since");
    assert_eq!(std::fs::read(fx.root.join("day/sub/new.txt"))?, b"new");
    let kept: Vec<&Path> = outcome.items[0]
        .originals_kept
        .iter()
        .map(|k| k.path.as_path())
        .collect();
    // A file first, the one that says why.
    assert!(kept[0].ends_with("day/a.xmp"), "{kept:?}");
    for stays in ["day/a.xmp", "day", "day/sub"] {
        assert!(kept.iter().any(|k| k.ends_with(stays)), "{stays}: {kept:?}");
    }
    assert_eq!(
        std::fs::read(fx.root.join("far/day/a.xmp"))?,
        bytes("day/a.xmp")
    );
    Ok(())
}

#[test]
fn a_photo_added_to_the_folder_since_planning_leaves_it_where_it_is() -> Result<()> {
    let mut fx = files();
    let disk = far(&fx);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    std::fs::write(fx.root.join("day/c.NEF"), bytes("day/c.NEF"))?;
    let root = fx.root.clone();
    fx.cat.add_folder(&root)?;
    let c = fx.photo("day/c.NEF");
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        &AtomicBool::new(false),
        &disk,
    )?;
    assert!(
        matches!(&results(&outcome)[..], [ItemResult::Unchanged(Some(_))]),
        "{outcome:?}"
    );
    assert_eq!(fx.rows()[&c], "day/c.NEF");
    assert!(!fx.root.join("far/day").exists());
    assert!(partials(&fx).is_empty());
    Ok(())
}

#[test]
#[ignore = "needs RAWMAKASE_TEST_OTHER_VOLUME: a writable folder on another volume"]
fn a_folder_moves_between_two_real_volumes() -> Result<()> {
    let Some(other) = std::env::var_os("RAWMAKASE_TEST_OTHER_VOLUME") else {
        return Ok(());
    };
    let mut fx = files();
    let there = tempfile::tempdir_in(&other)?;
    let dir: PathBuf = there.path().canonicalize()?;
    assert_ne!(
        fsops::entry(&dir)?.id.volume(),
        fsops::entry(&fx.root)?.id.volume(),
        "{} is on the fixture's volume",
        dir.display()
    );
    std::fs::write(dir.join("keep.dng"), bytes("keep.dng"))?;
    fx.cat.add_folder(&dir)?;
    let other_root = fx
        .cat
        .roots()?
        .into_iter()
        .map(|(id, _, _)| id)
        .find(|id| *id != fx.root_id())
        .unwrap();
    let a = fx.photo("day/a.NEF");
    let tree = tree_of(&fx.root.join("day"));
    let op = FileOp::MoveFolder {
        root: fx.root_id(),
        folder: "day".into(),
        to_root: other_root,
        parent: String::new(),
    };
    let plan = fx.plan(op);
    assert!(plan.summary().across.is_some());
    let trash = FakeTrash::new(&fx);
    let outcome = execute::execute(&mut fx.cat, plan.confirm(), &trash, &AtomicBool::new(false))?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(tree_of(&dir.join("day")), tree);
    assert!(!fx.root.join("day").exists());
    let path = fx
        .cat
        .photos()?
        .into_iter()
        .find(|p| p.id == a)
        .unwrap()
        .path;
    assert_eq!(path, dir.join("day/a.NEF"));
    let leftovers: Vec<_> = tree_of(&dir)
        .into_keys()
        .filter(|k| k.contains(".rawmakase-part"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    drop(fx);
    there.close()?;
    Ok(())
}

#[test]
fn a_file_added_to_the_original_after_the_catalog_follows_stays_with_its_folder() -> Result<()> {
    let mut fx = files();
    let disk = far(&fx);
    let mut trash = FakeTrash::new(&fx);
    trash.crash_before = Some(1);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    let batch = plan.batch().clone();
    assert!(crashes(|| {
        drop(execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &trash,
            &Default::default(),
            &disk,
        ));
    }));
    // Every original is as copied, but one more file is beside them.
    std::fs::write(fx.root.join("day/new.txt"), b"new")?;
    fx.reopen();
    trash.crash_before = None;
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    let outcome = super::outcome(&fx.cat, &batch)?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(std::fs::read(fx.root.join("day/new.txt"))?, b"new");
    assert!(!fx.root.join("day/a.NEF").exists());
    let kept = &outcome.items[0].originals_kept;
    let day = kept
        .iter()
        .find(|k| k.path == fx.root.join("day"))
        .unwrap_or_else(|| panic!("{kept:?}"));
    // What stays because of what wasn't copied says so.
    assert_eq!(day.why, "it holds new.txt, which wasn't copied");
    Ok(())
}

/// Saves an edit and reads a checksum for photo `name` in folder `folder`.
fn edited(fx: &mut Fx, folder: &str, name: &str) -> Result<()> {
    let id = fx.photo(&format!("{folder}/{name}"));
    let source = fx.root.join(folder).join(name);
    fx.cat.save_edit(
        id,
        &source,
        &crate::model::recipe::Recipe::default(),
        &crate::export_settings::ExportOptions::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    let crate::storage::ChecksumRead::Done(checksum) =
        crate::storage::FileChecksum::read_full(&source, |_| true)?
    else {
        panic!("not read")
    };
    fx.cat.save_checksums(&[crate::catalog::ReadChecksum {
        photo: id,
        folder: fx.folder(folder),
        filename: name.into(),
        path: source,
        checksum,
    }])?;
    Ok(())
}

/// The identity photo `id`'s saved edit names.
fn saved_identity(fx: &Fx, id: PhotoId) -> Option<crate::storage::Identity> {
    let text: Option<String> = fx
        .cat
        .db_for_tests()
        .query_row("SELECT identity FROM photos WHERE id=?", [id.0], |r| {
            r.get(0)
        })
        .unwrap();
    text.map(|t| serde_json::from_str(&t).unwrap())
}

#[test]
fn coarser_times_there_rekey_the_folders_saved_edits_and_checksums() -> Result<()> {
    let mut fx = files();
    edited(&mut fx, "day/sub", "b.NEF")?;
    let b = fx.photo("day/sub/b.NEF");
    let copy = fx.cat.create_virtual_copy(b)?;
    let was = crate::storage::Identity::read(&fx.root.join("day/sub/b.NEF"))?;
    let mut disk = far(&fx);
    // FAT's two seconds.
    disk.coarse = Some(2_000_000_000);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, &AtomicBool::new(false));
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    let target = fx.root.join("far/day/sub/b.NEF");
    let now = crate::storage::Identity::read(&target)?;
    assert_ne!(now, was, "the time was cut");
    assert_eq!(saved_identity(&fx, b), Some(now.clone()));
    assert_eq!(saved_identity(&fx, copy), Some(now.clone()));
    assert_eq!(fx.cat.checksums()?[&b].identity, now);
    assert!(fx.cat.load_edit(b, &target)?.is_some(), "the edit opens");
    Ok(())
}

#[test]
fn an_earlier_releases_edit_file_and_coarser_times_leave_the_folder() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "day/a.NEF.rawmakase.json", "far/"]);
    let tree = tree_of(&fx.root.join("day"));
    let mut disk = far(&fx);
    disk.coarse = Some(2_000_000_000);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, &AtomicBool::new(false));
    let [ItemResult::Unchanged(Some(why))] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert!(why.contains("earlier release"), "{why}");
    assert_eq!(fx.rows()[&fx.photo("day/a.NEF")], "day/a.NEF");
    assert_eq!(tree_of(&fx.root.join("day")), tree);
    assert!(!fx.root.join("far/day").exists());
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    Ok(())
}

#[test]
fn a_crash_once_the_trash_took_the_folder_is_finished_without_keeping_anything() -> Result<()> {
    let mut fx = files();
    let disk = far(&fx);
    let mut trash = FakeTrash::new(&fx);
    trash.crash_after = Some(1);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    let batch = plan.batch().clone();
    assert!(crashes(|| {
        drop(execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &trash,
            &Default::default(),
            &disk,
        ));
    }));
    fx.reopen();
    trash.crash_after = None;
    let recovery = fx.recover_with(&far(&fx), &trash);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    let outcome = super::outcome(&fx.cat, &batch)?;
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert!(outcome.items[0].originals_kept.is_empty(), "{outcome:?}");
    assert!(!fx.root.join("day").exists());
    assert!(trash.dir.join("1-day/sub/b.NEF").is_file());
    Ok(())
}

#[test]
fn a_move_refused_for_one_photo_leaves_every_other_photos_saved_edit_as_it_was() -> Result<()> {
    let mut fx = fixture(&["day/a.NEF", "day/b.NEF", "day/b.NEF.rawmakase.json", "far/"]);
    edited(&mut fx, "day", "a.NEF")?;
    let a = fx.photo("day/a.NEF");
    let saved = saved_identity(&fx, a);
    let checksum = fx.cat.checksums()?[&a].identity.clone();
    let mut disk = far(&fx);
    disk.coarse = Some(2_000_000_000);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, &AtomicBool::new(false));
    let [ItemResult::Unchanged(Some(why))] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    assert!(why.contains("earlier release"), "{why}");
    assert_eq!(saved_identity(&fx, a), saved);
    assert_eq!(fx.cat.checksums()?[&a].identity, checksum);
    assert!(fx.cat.load_edit(a, &fx.root.join("day/a.NEF"))?.is_some());
    Ok(())
}

#[test]
fn the_catalog_follows_the_names_the_copies_have_on_disk() -> Result<()> {
    let mut fx = files();
    let (b, sub) = (fx.photo("day/sub/b.NEF"), fx.folder("day/sub"));
    // The catalog spells them otherwise than the disk, as a volume that
    // ignores case lets it.
    fx.cat.db_for_tests().execute_batch(&format!(
        "UPDATE photos SET filename='B.nef' WHERE id={};
         UPDATE folder_paths SET path='day/SUB' WHERE folder={};",
        b.0, sub.0
    ))?;
    let ignores_case = fsops::entry(&fx.root.join("day/SUB/B.nef")).is_ok();
    let disk = far(&fx);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, &AtomicBool::new(false));
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    if ignores_case {
        assert_eq!(fx.rows()[&b], "far/day/sub/b.NEF");
        assert_eq!(logical(&fx, sub), "far/day/sub");
    } else {
        // Missing before, as it was named; missing after.
        assert_eq!(fx.rows()[&b], "far/day/SUB/B.nef");
    }
    Ok(())
}

#[test]
fn names_only_case_tells_apart_are_refused_when_planning() -> Result<()> {
    // A name there that differs only in case, as a volume that compares
    // names exactly lets it have.
    let fx = files();
    std::fs::create_dir(fx.root.join("far/DAY"))?;
    let why = refusal(&fx, &far(&fx));
    assert!(why.contains("already there"), "{why}");
    std::fs::remove_dir(fx.root.join("far/DAY"))?;
    // Two such names in the folder, where its volume can have both.
    std::fs::write(fx.root.join("day/notes.txt"), b"lower")?;
    std::fs::write(fx.root.join("day/NOTES.txt"), b"upper")?;
    if std::fs::read(fx.root.join("day/notes.txt"))? == b"lower" {
        let why = refusal(&fx, &far(&fx));
        assert!(why.contains("would be one name"), "{why}");
    }
    Ok(())
}

#[test]
fn a_file_another_app_saves_anew_keeps_only_its_own_copy_which_put_back_trashes() -> Result<()> {
    let mut fx = files();
    let tree_before = tree_of(&fx.root.join("day"));
    let disk = far(&fx);
    let trash = FakeTrash::new(&fx);
    let plan = fx.plan_on(to_far(&fx), &disk)?;
    // Saved anew the usual way: a new file renamed over the old one.
    std::fs::write(fx.root.join("day/.a.xmp.tmp"), b"saved anew")?;
    std::fs::rename(fx.root.join("day/.a.xmp.tmp"), fx.root.join("day/a.xmp"))?;
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &trash,
        &AtomicBool::new(false),
        &disk,
    )?;
    let [ItemResult::NeedsRecovery(_)] = &results(&outcome)[..] else {
        panic!("{outcome:?}")
    };
    // The copies of what is proven unchanged are gone; the one of what
    // changed stays, in the hidden folder.
    let left = partials(&fx);
    assert!(left.iter().any(|p| p.ends_with("a.xmp")), "{left:?}");
    assert!(
        !left
            .iter()
            .any(|p| p.ends_with("a.NEF") || p.ends_with(".hidden")),
        "{left:?}"
    );
    // The unfinished operation says where its copies are.
    fx.reopen();
    let recovery = fx.recover_with(&far(&fx), &trash);
    let u = &recovery.unfinished[0];
    for m in &u.members {
        if let Some(after) = &m.after {
            assert!(
                after.to_string_lossy().contains(".rawmakase-part"),
                "{after:?}"
            );
        }
    }
    let back = {
        let clean = far(&fx);
        recover::resolve_with(
            &mut fx.cat,
            u.operation,
            Resolution::PutBack,
            &clean,
            &trash,
        )
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(matches!(back.result, ItemResult::Unchanged(_)), "{back:?}");
    assert!(partials(&fx).is_empty(), "{:?}", partials(&fx));
    let mut now = tree_before.clone();
    now.insert("a.xmp".into(), Some(b"saved anew".to_vec()));
    assert_eq!(tree_of(&fx.root.join("day")), now);
    assert!(
        fx.files()
            .iter()
            .any(|(k, v)| k.starts_with("trash/") && v == b"saved anew")
    );
    Ok(())
}

#[test]
fn a_missing_folder_below_one_spelled_otherwise_stays_below_it() -> Result<()> {
    let mut fx = fixture(&["day/sub/b.NEF", "day/sub/gone/c.NEF", "far/"]);
    let (sub, gone) = (fx.folder("day/sub"), fx.folder("day/sub/gone"));
    std::fs::remove_file(fx.root.join("day/sub/gone/c.NEF"))?;
    std::fs::remove_dir(fx.root.join("day/sub/gone"))?;
    fx.cat.db_for_tests().execute_batch(&format!(
        "UPDATE folder_paths SET path='day/SUB' WHERE folder={};
         UPDATE folder_paths SET path='day/SUB/gone' WHERE folder={};",
        sub.0, gone.0
    ))?;
    let ignores_case = fsops::entry(&fx.root.join("day/SUB")).is_ok();
    let disk = far(&fx);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, &AtomicBool::new(false));
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    // Below its folder, however the disk spells that.
    let (sub, gone) = (logical(&fx, sub), logical(&fx, gone));
    assert_eq!(gone, format!("{sub}/gone"));
    if ignores_case {
        assert_eq!(sub, "far/day/sub");
    }
    Ok(())
}

#[test]
fn a_missing_folder_never_takes_the_place_of_another_named_alike() -> Result<()> {
    // Two folders only case tells apart, where the volume can have both.
    let mut fx = fixture(&["day/sub/b.NEF", "day/SUB/c.NEF", "far/"]);
    if std::fs::read_dir(fx.root.join("day"))?.count() != 2 {
        return Ok(());
    }
    let (sub, upper) = (fx.folder("day/sub"), fx.folder("day/SUB"));
    std::fs::remove_file(fx.root.join("day/SUB/c.NEF"))?;
    std::fs::remove_dir(fx.root.join("day/SUB"))?;
    let disk = far(&fx);
    let trash = FakeTrash::new(&fx);
    let op = to_far(&fx);
    let outcome = run(&mut fx, op, &trash, &disk, &AtomicBool::new(false));
    assert_eq!(results(&outcome), [ItemResult::FolderMoved]);
    assert_eq!(logical(&fx, sub), "far/day/sub");
    assert_eq!(logical(&fx, upper), "far/day/SUB");
    Ok(())
}
