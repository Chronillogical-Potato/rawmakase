use super::*;
use crate::{
    catalog::{Catalog, HistoryUpdate},
    export::{Destination, Format},
};
use std::sync::Mutex;

/// A photo of a batch at `source`, without an edit.
fn photo(source: &Path) -> BatchPhoto {
    BatchPhoto {
        id: 0,
        source: source.to_path_buf(),
        name: source.file_name().unwrap().to_string_lossy().into(),
        edit: Edit::Catalog(Default::default()),
        values: Default::default(),
    }
}
/// Export settings into `folder`, as small TIFFs.
fn settings(folder: &Path) -> ExportSettings {
    ExportSettings {
        destination: Destination::Folder,
        folder: Some(folder.to_path_buf()),
        format: Format::Tiff,
        resize: true,
        long_edge: 96,
        ..Default::default()
    }
}
fn names(plan: &Plan) -> Vec<(String, Write)> {
    plan.entries
        .iter()
        .map(|e| {
            (
                e.target.file_name().unwrap().to_string_lossy().into(),
                e.write,
            )
        })
        .collect()
}
fn s(name: &str, write: Write) -> (String, Write) {
    (name.into(), write)
}

#[test]
fn names_never_collide_within_a_batch() -> Result<()> {
    let d = tempfile::tempdir()?;
    let out = d.path().join("out");
    std::fs::create_dir(&out)?;
    // Already in the folder: a suffixed name, and one in other case.
    std::fs::write(out.join("a-2.tif"), b"")?;
    std::fs::write(out.join("C.TIF"), b"")?;
    let photos = [
        photo(Path::new("/one/a.dng")),
        // The same name from another folder, and a virtual copy of the first.
        photo(Path::new("/two/a.dng")),
        photo(Path::new("/one/a.dng")),
        photo(Path::new("/one/b.dng")),
    ];
    let plan = plan(&photos, &settings(&out), None).unwrap();
    assert_eq!(
        names(&plan),
        [
            s("a.tif", Write::Create),
            // a-2.tif is taken on disk.
            s("a-3.tif", Write::Create),
            s("a-4.tif", Write::Create),
            s("b.tif", Write::Create),
        ]
    );
    // Same Folder: each next to its own original, so no collision.
    let mut same = settings(&out);
    same.destination = Destination::SameFolder;
    let plan = super::plan(&photos[..2], &same, None).unwrap();
    assert_eq!(
        plan.entries
            .iter()
            .map(|e| e.target.clone())
            .collect::<Vec<_>>(),
        [PathBuf::from("/one/a.tif"), PathBuf::from("/two/a.tif")]
    );

    // A name differing only in case is the same name, and the new one keeps the
    // photo's own spelling.
    let mut unique = settings(&out);
    unique.existing = Existing::Unique;
    let plan = super::plan(&[photo(Path::new("/one/c.dng"))], &unique, None).unwrap();
    assert_eq!(names(&plan), [s("c-2.tif", Write::Create)]);
    // Overwrite replaces the file that is there, as it is spelled, and Ask names it
    // so: never a second file beside it in other case.
    let ask = settings(&out);
    assert_eq!(
        super::plan(&[photo(Path::new("/one/c.dng"))], &ask, None).unwrap_err(),
        Unplanned::Conflicts(vec![out.join("C.TIF")])
    );
    let plan = super::plan(
        &[photo(Path::new("/one/c.dng"))],
        &ask,
        Some(Existing::Overwrite),
    )
    .unwrap();
    assert_eq!(names(&plan), [s("C.TIF", Write::Overwrite)]);
    Ok(())
}

#[test]
fn ask_returns_the_files_that_exist_and_the_answer_applies_to_all() -> Result<()> {
    let d = tempfile::tempdir()?;
    let out = d.path().join("out");
    std::fs::create_dir(&out)?;
    std::fs::write(out.join("a.tif"), b"")?;
    std::fs::write(out.join("b.tif"), b"")?;
    let photos = [
        photo(Path::new("/p/a.dng")),
        photo(Path::new("/p/b.dng")),
        photo(Path::new("/p/c.dng")),
        // A second a: never over the first one's file.
        photo(Path::new("/q/a.dng")),
    ];
    let ask = settings(&out);
    assert_eq!(ask.existing, Existing::Ask);
    assert_eq!(
        plan(&photos, &ask, None).unwrap_err(),
        Unplanned::Conflicts(vec![out.join("a.tif"), out.join("b.tif")])
    );
    let overwrite = plan(&photos, &ask, Some(Existing::Overwrite)).unwrap();
    assert_eq!(
        names(&overwrite),
        [
            s("a.tif", Write::Overwrite),
            s("b.tif", Write::Overwrite),
            s("c.tif", Write::Create),
            s("a-2.tif", Write::Create),
        ]
    );
    assert_eq!(overwrite.late, Existing::Overwrite);
    let skip = plan(&photos, &ask, Some(Existing::Skip)).unwrap();
    assert_eq!(
        names(&skip),
        [
            s("a.tif", Write::Skip),
            s("b.tif", Write::Skip),
            s("c.tif", Write::Create),
            s("a.tif", Write::Skip),
        ]
    );
    let unique = plan(&photos, &ask, Some(Existing::Unique)).unwrap();
    assert_eq!(
        names(&unique),
        [
            s("a-2.tif", Write::Create),
            s("b-2.tif", Write::Create),
            s("c.tif", Write::Create),
            s("a-3.tif", Write::Create),
        ]
    );
    // Ask with nothing to ask about: a file appearing later is not replaced.
    let fresh = plan(&photos[2..3], &ask, None).unwrap();
    assert_eq!(fresh.late, Existing::Skip);
    // No folder chosen.
    let mut none = ask.clone();
    none.folder = None;
    assert_eq!(plan(&photos, &none, None).unwrap_err(), Unplanned::NoFolder);
    Ok(())
}

/// A catalog of copies of the synthetic chart DNG, and a file that isn't a photo.
struct Fixture {
    dir: tempfile::TempDir,
    catalog: Catalog,
    photos: Vec<(i64, PathBuf)>,
}
fn fixture(names: &[&str]) -> Result<Fixture> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    let chart = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/synthetic-d65.dng");
    for name in names {
        if name.ends_with(".dng") {
            std::fs::copy(&chart, folder.join(name))?;
        } else {
            std::fs::write(folder.join(name), b"not a photo")?;
        }
    }
    let mut catalog = Catalog::create(&dir.path().join("batch.rawmakase"))?;
    catalog.add_folder(&folder)?;
    let photos = catalog
        .photos()?
        .into_iter()
        .map(|p| (p.id, p.path))
        .collect();
    Ok(Fixture {
        dir,
        catalog,
        photos,
    })
}
impl Fixture {
    fn out(&self) -> PathBuf {
        self.dir.path().join("out")
    }
    /// The batch photos of every photo, as Export reads them.
    fn batch_photos(&self) -> Result<Vec<BatchPhoto>> {
        let ids: Vec<i64> = self.photos.iter().map(|(id, _)| *id).collect();
        Ok(self
            .catalog
            .photo_records(&ids)?
            .into_iter()
            .zip(&self.photos)
            .map(|(record, (_, path))| {
                let name = path.file_name().unwrap().to_string_lossy().into();
                BatchPhoto::from_record(record, path.clone(), name)
            })
            .collect())
    }
    fn batch(&self, photos: Vec<BatchPhoto>, settings: ExportSettings) -> Batch {
        Batch {
            plan: plan(&photos, &settings, Some(Existing::Unique)).unwrap(),
            photos,
            settings,
            defaults: Arc::new(DevelopDefaults::with_presets(Default::default(), |_| None)),
            watermark: None,
        }
    }
}
fn run_all(batch: &Batch) -> Vec<Outcome> {
    run(batch, &AtomicBool::new(false), |_| {})
}
fn exported(outcome: &Outcome) -> &Path {
    match outcome {
        Outcome::Exported { path, .. } => path,
        other => panic!("not exported: {other:?}"),
    }
}
fn pixels(path: &Path) -> Vec<u16> {
    image::open(path).unwrap().into_rgb16().into_raw()
}
/// Files in `dir`, sorted.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn every_photo_is_exported_with_its_own_edit_or_said_why_not() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng", "c.dng", "d.dng", "e.dng"])?;
    let [(a, a_path), (b, b_path), _, (d, d_path), (_, e_path)] = &f.photos[..] else {
        panic!("five photos");
    };
    let m = crate::raw::Raw::open(a_path)?.metadata;
    let (profiles, _) = crate::camera_profiles::installed(&m);
    let mut brighter = Recipe::with_profiles(&m, &profiles);
    brighter.exposure = 1.;
    let save = |id: i64, path: &Path| {
        f.catalog.save_edit(
            id,
            path,
            &brighter,
            &Default::default(),
            HistoryUpdate::Keep,
        )
    };
    save(*a, a_path)?;
    save(*d, d_path)?;
    f.catalog.db_for_tests().execute(
        "UPDATE photos SET lightroom_develop='s = { Exposure2012 = 0.5 }' WHERE id=?",
        [b],
    )?;
    let photos = f.batch_photos()?;
    // Its file changed since the edit was saved: protected, not exported unedited.
    let mut bytes = std::fs::read(d_path)?;
    bytes.extend_from_slice(b"changed");
    std::fs::write(d_path, bytes)?;
    // Offline since Export was pressed.
    std::fs::remove_file(e_path)?;
    let _ = b_path;
    let outcomes = run_all(&f.batch(photos, settings(&f.out())));
    let mean = |o: &Outcome| {
        let p = pixels(exported(o));
        p.iter().map(|&v| v as f64).sum::<f64>() / p.len() as f64
    };
    let (saved, lightroom, unedited) = (mean(&outcomes[0]), mean(&outcomes[1]), mean(&outcomes[2]));
    assert!(
        saved > lightroom && lightroom > unedited,
        "{saved} {lightroom} {unedited}"
    );
    match &outcomes[3] {
        Outcome::Failed(reason) => assert!(reason.contains("protected"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(
        matches!(&outcomes[4], Outcome::Failed(_)),
        "{:?}",
        outcomes[4]
    );
    assert_eq!(listing(&f.out()), ["a.tif", "b.tif", "c.tif"]);
    Ok(())
}

#[test]
fn a_file_that_appears_during_the_export_gets_the_batchs_answer() -> Result<()> {
    let f = fixture(&["a.dng"])?;
    let photos = f.batch_photos()?;
    let late = |existing: Existing, answer: Option<Existing>| -> Result<(Outcome, Vec<String>)> {
        let out = f.out().join(format!("{existing:?}-{}", answer.is_some()));
        let mut settings = settings(&out);
        settings.existing = existing;
        let batch = Batch {
            plan: plan(&photos, &settings, answer).unwrap(),
            ..f.batch(photos.clone(), settings)
        };
        assert_eq!(batch.plan.entries[0].write, Write::Create);
        // Appears after planning, before the photo is written.
        std::fs::create_dir_all(&out)?;
        std::fs::write(out.join("a.tif"), b"late")?;
        let outcome = run_all(&batch).remove(0);
        Ok((outcome, listing(&out)))
    };
    let (outcome, files) = late(Existing::Unique, None)?;
    assert_eq!(exported(&outcome), f.out().join("Unique-false/a-2.tif"));
    assert_eq!(files, ["a-2.tif", "a.tif"]);
    let (outcome, files) = late(Existing::Skip, None)?;
    assert!(matches!(outcome, Outcome::Skipped(_)), "{outcome:?}");
    assert_eq!(files, ["a.tif"]);
    // Overwrite WITHOUT WARNING covers it too, and says so.
    let (outcome, files) = late(Existing::Overwrite, None)?;
    match &outcome {
        Outcome::Exported { notes, .. } => assert!(notes[0].contains("replaced"), "{notes:?}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(files, ["a.tif"]);
    assert_ne!(
        std::fs::read(f.out().join("Overwrite-false/a.tif"))?,
        b"late"
    );
    // Ask had nothing to ask about: no answer, so nothing is replaced.
    let (outcome, files) = late(Existing::Ask, None)?;
    assert!(matches!(outcome, Outcome::Skipped(_)), "{outcome:?}");
    assert_eq!(files, ["a.tif"]);
    Ok(())
}

#[test]
fn cancel_before_a_commit_leaves_nothing_and_after_it_counts_as_exported() -> Result<()> {
    let f = fixture(&["a.dng", "b.dng"])?;
    let photos = f.batch_photos()?;
    // Cancel at `at` of the first photo: 0.95 is staged and not yet committed, 1
    // is committed.
    let cancelled_at = |at: f32| -> Result<(Vec<Outcome>, Vec<String>)> {
        let out = f.out().join(format!("{at}"));
        let batch = f.batch(photos.clone(), settings(&out));
        let cancel = AtomicBool::new(false);
        let outcomes = run(&batch, &cancel, |p| {
            if p.done == 0 && p.fraction >= at {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        Ok((outcomes, listing(&out)))
    };
    let (outcomes, files) = cancelled_at(0.95)?;
    assert_eq!(outcomes, [Outcome::Cancelled, Outcome::NotStarted]);
    // No export and no temporary file.
    assert!(files.is_empty(), "{files:?}");
    let (outcomes, files) = cancelled_at(1.)?;
    assert!(
        matches!(outcomes[0], Outcome::Exported { .. }),
        "{outcomes:?}"
    );
    assert_eq!(outcomes[1], Outcome::NotStarted);
    assert_eq!(files, ["a.tif"]);
    // Before the photo renders.
    let (outcomes, files) = cancelled_at(0.)?;
    assert_eq!(outcomes, [Outcome::Cancelled, Outcome::NotStarted]);
    assert!(files.is_empty(), "{files:?}");
    Ok(())
}

#[test]
fn a_batch_exports_the_edits_as_they_were_when_export_was_pressed() -> Result<()> {
    let f = fixture(&["a.dng"])?;
    let (id, path) = &f.photos[0];
    let m = crate::raw::Raw::open(path)?.metadata;
    let (profiles, _) = crate::camera_profiles::installed(&m);
    let mut edit = Recipe::with_profiles(&m, &profiles);
    edit.exposure = 0.5;
    f.catalog
        .save_edit(*id, path, &edit, &Default::default(), HistoryUpdate::Keep)?;
    let before = f.batch(f.batch_photos()?, settings(&f.out().join("before")));
    let expected = run_all(&before);
    // Edited again after Export was pressed.
    edit.exposure = -1.;
    f.catalog
        .save_edit(*id, path, &edit, &Default::default(), HistoryUpdate::Keep)?;
    let snapshot = Batch {
        settings: settings(&f.out().join("after")),
        plan: plan(&before.photos, &settings(&f.out().join("after")), None).unwrap(),
        ..before
    };
    let outcomes = run_all(&snapshot);
    assert_eq!(
        pixels(exported(&outcomes[0])),
        pixels(exported(&expected[0]))
    );
    Ok(())
}

#[test]
fn the_queue_runs_one_batch_at_a_time_and_a_waiting_one_can_be_removed() -> Result<()> {
    let f = fixture(&["a.dng"])?;
    let photos = f.batch_photos()?;
    let done = Arc::new(Mutex::new(Vec::new()));
    let finished = done.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let changes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let changed = changes.clone();
    let queue = crate::export::queue::Queue::new(
        move |ticket, outcomes| {
            finished.lock().unwrap().push((ticket, outcomes));
            let _ = tx.send(ticket);
        },
        move || {
            changed.fetch_add(1, Ordering::Relaxed);
        },
    );
    let batch = |name: &str| f.batch(photos.clone(), settings(&f.out().join(name)));
    let first = queue.submit(batch("first"));
    let second = queue.submit(batch("second"));
    let third = queue.submit(batch("third"));
    // Nothing runs beside the first; the second waits and is removed, which those
    // showing the queue hear about.
    let heard = changes.load(Ordering::Relaxed);
    assert!(queue.remove(second));
    assert!(changes.load(Ordering::Relaxed) > heard);
    assert!(!queue.cancel(second));
    let mut finished = vec![rx.recv_timeout(std::time::Duration::from_secs(120))?];
    finished.push(rx.recv_timeout(std::time::Duration::from_secs(120))?);
    assert_eq!(finished, [first, third]);
    assert!(!queue.remove(first));
    assert!(!queue.busy());
    assert!(listing(&f.out().join("second")).is_empty());
    assert_eq!(listing(&f.out().join("third")), ["a.tif"]);
    assert!(
        done.lock()
            .unwrap()
            .iter()
            .all(|(_, o)| matches!(o[0], Outcome::Exported { .. }))
    );
    Ok(())
}
