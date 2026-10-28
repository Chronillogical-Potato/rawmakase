//! Folder stacks through file operations: a photo moved to another folder
//! leaves its stack; renamed photos and a renamed folder keep theirs.
use super::tests::*;
use super::*;
use crate::catalog::stacks_tests::group;

#[test]
fn a_photo_moved_to_another_folder_leaves_its_stack() {
    let mut fx = fixture(&["day/a.NEF", "day/b.NEF", "day/c.NEF", "sub/"]);
    let (a, b, c) = (
        fx.photo("day/a.NEF"),
        fx.photo("day/b.NEF"),
        fx.photo("day/c.NEF"),
    );
    group(&mut fx.cat, &[a, b, c]).unwrap();
    let copy = fx.cat.create_virtual_copy(a).unwrap();
    assert_eq!(fx.cat.stack_of(b).unwrap(), [a, copy, b, c]);
    let outcome = fx.run(
        FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_id(),
            to: "sub".into(),
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert!(matches!(results(&outcome)[..], [ItemResult::Moved { .. }]));
    // The master and its copy went; the rest stays a stack, b its top.
    assert_eq!(fx.cat.stack_of(b).unwrap(), [b, c]);
    assert!(fx.cat.stack_of(a).unwrap().is_empty());
    assert!(fx.cat.stack_of(copy).unwrap().is_empty());

    // Moved back, it does not come back into the stack.
    let outcome = fx.run(
        FileOp::MovePhotos {
            photos: vec![a],
            root: fx.root_id(),
            to: "day".into(),
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert!(matches!(results(&outcome)[..], [ItemResult::Moved { .. }]));
    assert_eq!(fx.cat.stack_of(b).unwrap(), [b, c]);
    assert!(fx.cat.stack_of(a).unwrap().is_empty());
}

#[test]
fn renamed_photos_and_a_renamed_folder_keep_their_stacks() {
    let mut fx = fixture(&["day/a.NEF", "day/b.NEF"]);
    let (a, b) = (fx.photo("day/a.NEF"), fx.photo("day/b.NEF"));
    group(&mut fx.cat, &[b, a]).unwrap();
    let outcome = fx.run(
        FileOp::RenamePhotos {
            renames: vec![(a, "first".into())],
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert!(matches!(
        results(&outcome)[..],
        [ItemResult::Renamed { .. }]
    ));
    assert_eq!(fx.cat.stack_of(a).unwrap(), [b, a]);
    let root = fx.root_id();
    let outcome = fx.run(
        FileOp::RenameFolder {
            root,
            folder: "day".into(),
            name: "Day 1".into(),
        },
        &NoTrash,
        &Faulty::default(),
    );
    assert!(matches!(results(&outcome)[..], [ItemResult::FolderRenamed]));
    assert!(fx.root.join("Day 1/first.NEF").is_file());
    assert_eq!(fx.cat.stack_of(a).unwrap(), [b, a]);
}
