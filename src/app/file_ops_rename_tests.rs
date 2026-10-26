//! Renaming in the app: Rename Photo (F2), Rename Photos in capture order
//! (#342) and Rename Folder, driven by keys and frames on a catalog of
//! synthetic files in a temporary folder.
use super::move_tests::{at, decoded, editor_with_folders, folder, photo, root, type_text};
use super::tests::{frame, select, settle, tap};
use crate::catalog::file_ops;

use crate::app::Module;
use eframe::egui::{Key, Modifiers};

/// Sets the catalog's capture time of the photo whose file is `name`.
fn captured(d: &tempfile::TempDir, name: &str, when: &str) -> anyhow::Result<()> {
    let db = rusqlite::Connection::open(d.path().join("test.rawmakase"))?;
    db.execute(
        "UPDATE photos SET captured=?1 WHERE filename=?2",
        rusqlite::params![when, name],
    )?;
    Ok(())
}

/// Replaces the focused field's text with `text`.
fn retype(e: &mut crate::app::Editor, text: &str) {
    tap(e, Key::A, Modifiers::COMMAND);
    type_text(e, text);
}

#[test]
fn f2_renames_one_photo_and_its_sidecar_and_the_open_photo_follows() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "a.xmp", "b.RAF"])?;
    let a = photo(&e, &d, "a.RAF");
    select(&mut e, &[a]);
    e.module = Module::Library;
    e.document.catalog_photo = Some(a);
    e.document.path = Some(d.path().join("photos/a.RAF"));
    e.document.set_image(decoded(false));
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    frame(&mut e, Vec::new());
    let shown = e.file_ops.rename_shown().expect("Rename Photo is open");
    assert_eq!(shown.title, "Rename Photo “a.RAF”");
    // The name, not its extension, is what is typed over.
    type_text(&mut e, "Keeper");
    frame(&mut e, Vec::new());
    let shown = e.file_ops.rename_shown().unwrap();
    assert_eq!(shown.rows, [("a.RAF".into(), "Keeper.RAF".into(), None)]);
    assert!(shown.can_rename);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/Keeper.RAF").is_file());
    assert!(d.path().join("photos/Keeper.xmp").is_file());
    assert!(!d.path().join("photos/a.RAF").exists());
    assert_eq!(at(&e, &d, a), "Keeper.RAF");
    assert_eq!(e.status, "Renamed a.RAF to Keeper.RAF");
    let open = e.document.path.clone().unwrap();
    assert!(open.ends_with("photos/Keeper.RAF"), "{}", open.display());
    Ok(())
}

#[test]
fn a_name_taken_there_is_said_at_once_and_renames_nothing() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "b.RAF"])?;
    let a = photo(&e, &d, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    frame(&mut e, Vec::new());
    type_text(&mut e, "B");
    frame(&mut e, Vec::new());
    let shown = e.file_ops.rename_shown().unwrap();
    assert!(!shown.can_rename);
    assert!(shown.rows[0].2.is_some(), "{:?}", shown.rows);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(e.file_ops.rename_shown().is_some(), "still asking");
    assert!(d.path().join("photos/a.RAF").is_file());
    // Escape lets the catalog go.
    tap(&mut e, Key::Escape, Modifiers::NONE);
    frame(&mut e, Vec::new());
    assert!(e.file_ops.rename_shown().is_none());
    assert!(!e.activity.is_busy());
    Ok(())
}

#[test]
fn several_photos_are_renamed_in_the_order_they_were_taken() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["x.RAF", "y.RAF", "z.RAF"])?;
    captured(&d, "x.RAF", "2026-05-04T12:00:03")?;
    captured(&d, "y.RAF", "2026-05-04T12:00:01")?;
    captured(&d, "z.RAF", "2026-05-04T12:00:02")?;
    let ids: Vec<_> = ["x.RAF", "y.RAF", "z.RAF"]
        .iter()
        .map(|n| photo(&e, &d, n))
        .collect();
    select(&mut e, &ids);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    frame(&mut e, Vec::new());
    let shown = e.file_ops.rename_shown().expect("Rename Photos is open");
    assert_eq!(shown.title, "Rename 3 Photos");
    type_text(&mut e, "Trip");
    frame(&mut e, Vec::new());
    let shown = e.file_ops.rename_shown().unwrap();
    let names: Vec<(&str, &str)> = shown
        .rows
        .iter()
        .map(|(old, new, _)| (old.as_str(), new.as_str()))
        .collect();
    assert_eq!(
        names,
        [
            ("y.RAF", "Trip-1.RAF"),
            ("z.RAF", "Trip-2.RAF"),
            ("x.RAF", "Trip-3.RAF")
        ]
    );
    assert!(shown.can_rename);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert_eq!(at(&e, &d, ids[1]), "Trip-1.RAF");
    assert_eq!(at(&e, &d, ids[2]), "Trip-2.RAF");
    assert_eq!(at(&e, &d, ids[0]), "Trip-3.RAF");
    assert_eq!(e.status, "Renamed 3 photos");
    Ok(())
}

#[test]
fn a_batch_with_one_name_taken_renames_none() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "b.RAF", "Trip-2.RAF"])?;
    let (a, b) = (photo(&e, &d, "a.RAF"), photo(&e, &d, "b.RAF"));
    select(&mut e, &[a, b]);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    frame(&mut e, Vec::new());
    type_text(&mut e, "Trip");
    frame(&mut e, Vec::new());
    let shown = e.file_ops.rename_shown().unwrap();
    assert!(!shown.can_rename);
    assert!(shown.rows.iter().all(|r| r.2.is_some()), "{:?}", shown.rows);
    tap(&mut e, Key::Escape, Modifiers::NONE);
    frame(&mut e, Vec::new());
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/a.RAF").is_file());
    Ok(())
}

#[test]
fn rename_folder_renames_it_on_disk_and_in_the_catalog() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["trip/a.RAF", "trip/day1/b.RAF"])?;
    let (a, b) = (
        photo(&e, &d, "trip/a.RAF"),
        photo(&e, &d, "trip/day1/b.RAF"),
    );
    let trip = folder(&e, "trip").unwrap();
    let r = root(&e);
    e.rename_folder(r, "trip".into());
    frame(&mut e, Vec::new());
    retype(&mut e, "Iceland");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/Iceland/day1/b.RAF").is_file());
    assert!(!d.path().join("photos/trip").exists());
    assert_eq!(at(&e, &d, a), "Iceland/a.RAF");
    assert_eq!(at(&e, &d, b), "Iceland/day1/b.RAF");
    assert_eq!(folder(&e, "Iceland"), Some(trip), "the same folder");
    assert_eq!(e.status, "Renamed folder trip to Iceland");
    Ok(())
}

#[test]
fn a_folder_name_taken_there_renames_nothing_and_says_why() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["trip/a.RAF", "other/b.RAF"])?;
    let r = root(&e);
    e.rename_folder(r, "trip".into());
    frame(&mut e, Vec::new());
    retype(&mut e, "other");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/trip/a.RAF").is_file());
    assert!(e.status.contains("already there"), "{}", e.status);
    Ok(())
}

#[test]
fn a_name_typed_and_returned_in_one_frame_is_the_one_renamed() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF"])?;
    let a = photo(&e, &d, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    frame(&mut e, Vec::new());
    type_text(&mut e, "Keeper");
    frame(&mut e, Vec::new());
    // More typed, and Return, before the next frame plans it.
    frame(
        &mut e,
        vec![
            eframe::egui::Event::Text("2".into()),
            super::tests::key(Key::Enter, true, false, Modifiers::NONE),
            super::tests::key(Key::Enter, false, false, Modifiers::NONE),
        ],
    );
    settle(&mut e);
    assert_eq!(at(&e, &d, a), "Keeper2.RAF");
    Ok(())
}

#[test]
fn reading_the_photos_order_can_be_cancelled_and_its_answer_is_then_dropped() -> anyhow::Result<()>
{
    let (d, mut e, _) = editor_with_folders(&["a.RAF", "b.RAF"])?;
    let ids = vec![photo(&e, &d, "a.RAF"), photo(&e, &d, "b.RAF")];
    let catalog = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog
        .location()
        .clone();
    let order = file_ops::rename_order(
        &e.library.as_ref().unwrap().session.catalog,
        &ids,
        &Default::default(),
        crate::catalog_session::file_capture_time,
    )?;
    e.request_rename(ids);
    assert!(e.activity.is_busy(), "the catalog is held while it reads");
    let cancel = e.file_ops.ordering_cancel().expect("reading");
    e.cancel_ordering();
    assert!(
        cancel.load(std::sync::atomic::Ordering::Relaxed),
        "the worker is told"
    );
    assert!(!e.activity.is_busy());
    // Its answer, coming after, opens nothing.
    e.rename_ordered(catalog, cancel, Ok(order));
    assert!(e.file_ops.rename_shown().is_none());
    assert!(!e.activity.is_busy());
    Ok(())
}

#[test]
fn an_empty_folder_renamed_is_shown_by_its_new_name() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF"])?;
    let r = root(&e);
    e.name_new_folder(r, String::new(), "photos".into(), Vec::new(), false);
    frame(&mut e, Vec::new());
    type_text(&mut e, "empty");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    let empty = folder(&e, "empty").expect("made");
    e.rename_folder(r, "empty".into());
    frame(&mut e, Vec::new());
    retype(&mut e, "Selects");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/Selects").is_dir());
    assert_eq!(folder(&e, "Selects"), Some(empty));
    assert!(folder(&e, "empty").is_none());
    Ok(())
}

#[test]
fn a_name_made_invalid_in_the_frame_it_is_returned_keeps_the_dialog() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["a.RAF"])?;
    let a = photo(&e, &d, "a.RAF");
    select(&mut e, &[a]);
    tap(&mut e, Key::F2, Modifiers::NONE);
    settle(&mut e);
    frame(&mut e, Vec::new());
    type_text(&mut e, "Keeper");
    frame(&mut e, Vec::new());
    frame(
        &mut e,
        vec![
            eframe::egui::Event::Text("/".into()),
            super::tests::key(Key::Enter, true, false, Modifiers::NONE),
            super::tests::key(Key::Enter, false, false, Modifiers::NONE),
        ],
    );
    frame(&mut e, Vec::new());
    let shown = e.file_ops.rename_shown().expect("still asking");
    assert!(!shown.can_rename);
    assert!(shown.rows[0].2.is_some(), "{:?}", shown.rows);
    assert!(d.path().join("photos/a.RAF").is_file());
    tap(&mut e, Key::Escape, Modifiers::NONE);
    frame(&mut e, Vec::new());
    assert!(!e.activity.is_busy(), "the catalog is let go");
    Ok(())
}
