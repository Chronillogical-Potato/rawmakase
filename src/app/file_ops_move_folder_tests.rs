//! Moving a folder in the app: into another folder of its root, or to the
//! root itself, confirmed with Cancel focused; driven by keys and frames on
//! a catalog of synthetic files in a temporary folder.
use super::move_tests::{at, editor_with_folders, folder, photo, root};
use super::tests::{settle, tap};
use eframe::egui::{Key, Modifiers};

#[test]
fn a_folder_moves_into_another_with_its_photos_once_confirmed() -> anyhow::Result<()> {
    let (d, mut e, _) =
        editor_with_folders(&["2024/day1/a.RAF", "2024/day1/sub/b.RAF", "2025/c.RAF"])?;
    let (a, b) = (
        photo(&e, &d, "2024/day1/a.RAF"),
        photo(&e, &d, "2024/day1/sub/b.RAF"),
    );
    let day1 = folder(&e, "2024/day1").unwrap();
    let r = root(&e);
    e.request_folder_move(r, "2024/day1".into(), "2025".into());
    settle(&mut e);
    let (title, lines, actions) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Move folder “day1” into “2025”?");
    assert_eq!(actions, ["Move"]);
    assert!(
        lines
            .iter()
            .any(|l| l == "The folder moves on disk with the 2 photos and the folders in it."),
        "{lines:?}"
    );
    // Return alone cancels.
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/2024/day1/a.RAF").is_file());
    e.request_folder_move(r, "2024/day1".into(), "2025".into());
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(d.path().join("photos/2025/day1/sub/b.RAF").is_file());
    assert!(!d.path().join("photos/2024/day1").exists());
    assert_eq!(at(&e, &d, a), "2025/day1/a.RAF");
    assert_eq!(at(&e, &d, b), "2025/day1/sub/b.RAF");
    assert_eq!(folder(&e, "2025/day1"), Some(day1), "the same folder");
    assert_eq!(e.status, "Moved folder day1 into 2025");
    Ok(())
}

#[test]
fn a_folder_moves_to_its_root() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["2024/day1/a.RAF"])?;
    let day1 = folder(&e, "2024/day1").unwrap();
    let r = root(&e);
    e.request_folder_move(r, "2024/day1".into(), String::new());
    settle(&mut e);
    let (title, ..) = e.file_ops.question().expect("it asks");
    assert_eq!(title, "Move folder “day1” into “photos”?");
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/day1/a.RAF").is_file());
    assert_eq!(folder(&e, "day1"), Some(day1));
    Ok(())
}

#[test]
fn a_folder_into_itself_is_refused_and_says_why() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["2024/day1/a.RAF", "2024/day1/sub/b.RAF"])?;
    let r = root(&e);
    e.request_folder_move(r, "2024/day1".into(), "2024/day1/sub".into());
    settle(&mut e);
    assert!(!e.file_ops.confirming());
    assert!(!e.activity.is_busy());
    assert!(e.status.contains("into itself"), "{}", e.status);
    assert!(d.path().join("photos/2024/day1/sub/b.RAF").is_file());
    Ok(())
}

#[test]
fn an_empty_folder_moved_is_shown_where_it_went() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_folders(&["2025/c.RAF"])?;
    let r = root(&e);
    e.name_new_folder(r, String::new(), "photos".into(), Vec::new(), false);
    super::tests::frame(&mut e, Vec::new());
    super::move_tests::type_text(&mut e, "empty");
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    let empty = folder(&e, "empty").expect("made");
    e.request_folder_move(r, "empty".into(), "2025".into());
    settle(&mut e);
    tap(&mut e, Key::Tab, Modifiers::NONE);
    tap(&mut e, Key::Enter, Modifiers::NONE);
    settle(&mut e);
    assert!(d.path().join("photos/2025/empty").is_dir());
    assert_eq!(folder(&e, "2025/empty"), Some(empty));
    assert!(folder(&e, "empty").is_none());
    Ok(())
}

#[test]
fn move_folder_offers_folders_with_no_catalog_row_of_their_own() -> anyhow::Result<()> {
    let (_d, mut e, _) = editor_with_folders(&["2024/day1/a.RAF", "2025/day2/b.RAF"])?;
    // 2025 has no folder row: only 2025/day2 has photos.
    assert!(folder(&e, "2025").is_none());
    let r = root(&e);
    e.choose_parent_for(r, "2024/day1".into());
    let offered = e.parent_choices();
    assert!(offered.contains(&"2025".to_string()), "{offered:?}");
    assert!(offered.contains(&"2025/day2".to_string()));
    assert!(offered.contains(&String::new()), "its root");
    // Not where it is, nor itself.
    assert!(!offered.contains(&"2024".to_string()));
    assert!(!offered.contains(&"2024/day1".to_string()));
    Ok(())
}
