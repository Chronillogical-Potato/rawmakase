//! Folder stacks (see `stacks`): what each command leaves, and how rows a
//! move, a removal or another release changed are read.
use super::*;
use rusqlite::params;
use std::path::PathBuf;

/// Group into Stack: `photos` one stack, the first its top; the stack.
pub(super) fn group(cat: &mut Catalog, photos: &[PhotoId]) -> Result<StackId> {
    cat.change_stacks(&StackChange::Group(photos.to_vec()))?;
    Ok(cat.stack_places()?[&photos[0]].id)
}

/// A catalog of `names` in folder "photos", their ids by name, and the
/// folder.
fn catalog(names: &[&str]) -> Result<(tempfile::TempDir, Catalog, Vec<PhotoId>, PathBuf)> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in names {
        std::fs::write(folder.join(name), format!("synthetic {name}"))?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let photos = cat.photos()?;
    let ids = names
        .iter()
        .map(|n| photos.iter().find(|p| p.filename == *n).unwrap().id)
        .collect();
    Ok((dir, cat, ids, folder))
}

/// Removes photo `id` from the catalog, as Remove from Catalog does.
fn remove(cat: &mut Catalog, id: PhotoId) -> Result<()> {
    cat.db.write(|w| super::copies::delete_photo(w, id))
}

/// Each stacked photo's (stack, index, count, collapsed), by id.
fn places(cat: &Catalog) -> Result<Vec<(PhotoId, usize, usize, bool)>> {
    let mut places: Vec<_> = cat
        .stack_places()?
        .into_iter()
        .map(|(id, p)| (id, p.index, p.count, p.collapsed))
        .collect();
    places.sort();
    Ok(places)
}

#[test]
fn grouping_makes_a_collapsed_stack_topped_by_the_first_photo() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW", "d.ARW"])?;
    let [a, b, c, d] = ids[..] else { panic!() };
    let id = group(&mut cat, &[c, a, b])?;
    assert_eq!(cat.stack_of(a)?, [c, a, b]);
    assert!(cat.stack_of(d)?.is_empty());
    let photo = |id: PhotoId| cat.photos().unwrap().into_iter().find(|p| p.id == id);
    let top = photo(c).unwrap().stack.unwrap();
    assert_eq!(
        (top.id, top.index, top.count, top.collapsed),
        (id, 0, 3, true)
    );
    assert!(top.is_top());
    assert_eq!(photo(d).unwrap().stack, None);

    // Fewer than two photos, a photo twice or an unknown one: no stack.
    assert!(group(&mut cat, &[d]).is_err());
    assert!(group(&mut cat, &[d, d]).is_err());
    assert!(group(&mut cat, &[d, PhotoId(9999)]).is_err());
    assert!(cat.stack_of(d)?.is_empty());
    Ok(())
}

#[test]
fn photos_of_different_folders_are_not_stacked() -> Result<()> {
    let dir = tempfile::tempdir()?;
    for sub in ["x", "y"] {
        std::fs::create_dir_all(dir.path().join("photos").join(sub))?;
        std::fs::write(dir.path().join("photos").join(sub).join("a.ARW"), sub)?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&dir.path().join("photos"))?;
    let ids: Vec<_> = cat.photos()?.iter().map(|p| p.id).collect();
    let error = group(&mut cat, &ids).unwrap_err();
    assert!(error.to_string().contains("same folder"), "{error}");
    assert!(cat.stack_places()?.is_empty());
    Ok(())
}

#[test]
fn regrouping_takes_photos_out_of_their_stacks_and_a_single_photo_is_no_stack() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW", "d.ARW"])?;
    let [a, b, c, d] = ids[..] else { panic!() };
    group(&mut cat, &[a, b, c])?;
    group(&mut cat, &[d, c])?;
    assert_eq!(cat.stack_of(a)?, [a, b]);
    assert_eq!(cat.stack_of(c)?, [d, c]);
    group(&mut cat, &[b, d])?;
    // a is alone in its stack: no stack any more; so is c.
    assert!(cat.stack_of(a)?.is_empty());
    assert!(cat.stack_of(c)?.is_empty());
    assert_eq!(cat.stack_of(b)?, [b, d]);
    let stacks: i64 = cat
        .db_for_tests()
        .query_row("SELECT count(*) FROM stacks", [], |r| r.get(0))?;
    // Two stacks still hold one photo's row each, until it leaves them.
    assert!(stacks <= 3, "{stacks}");
    Ok(())
}

#[test]
fn unstack_remove_and_move_to_top_keep_the_rest_in_order() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW", "d.ARW"])?;
    let [a, b, c, d] = ids[..] else { panic!() };
    group(&mut cat, &[a, b, c, d])?;
    cat.change_stacks(&StackChange::MoveToTop(c))?;
    assert_eq!(cat.stack_of(a)?, [c, a, b, d]);
    cat.change_stacks(&StackChange::Move(a, 1))?;
    assert_eq!(cat.stack_of(a)?, [c, b, a, d]);
    cat.change_stacks(&StackChange::Move(d, 1))?;
    assert_eq!(cat.stack_of(a)?, [c, b, a, d]);
    // Removing the top makes the next photo the top.
    cat.change_stacks(&StackChange::Remove(vec![c]))?;
    assert_eq!(cat.stack_of(a)?, [b, a, d]);
    assert!(cat.stack_of(c)?.is_empty());
    cat.change_stacks(&StackChange::Remove(vec![a, d]))?;
    assert!(cat.stack_places()?.is_empty());
    assert!(cat.change_stacks(&StackChange::MoveToTop(a)).is_err());

    group(&mut cat, &[a, b])?;
    group(&mut cat, &[c, d])?;
    cat.change_stacks(&StackChange::Unstack(vec![b]))?;
    assert!(cat.stack_of(a)?.is_empty());
    assert_eq!(cat.stack_of(d)?, [c, d]);
    Ok(())
}

#[test]
fn collapsing_is_saved_with_the_catalog() -> Result<()> {
    let (dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW"])?;
    let id = group(&mut cat, &ids)?;
    cat.set_stacks_collapsed(&[id], false)?;
    drop(cat);
    let cat = Catalog::open(dir.path().join("Photos.rawmakase"))?;
    assert!(places(&cat)?.iter().all(|p| !p.3));
    Ok(())
}

#[test]
fn a_virtual_copy_is_stacked_right_after_its_photo_and_shown() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW"])?;
    let [a, b, c] = ids[..] else { panic!() };
    let copy = cat.create_virtual_copy(a)?;
    assert_eq!(cat.stack_of(a)?, [a, copy]);
    assert!(
        places(&cat)?.iter().all(|p| !p.3),
        "open, so the copy shows"
    );

    let id = group(&mut cat, &[b, c])?;
    cat.set_stacks_collapsed(&[id], true)?;
    let second = cat.create_virtual_copy(b)?;
    assert_eq!(cat.stack_of(c)?, [b, second, c]);
    // A copy of a copy follows the copy it was made from.
    let third = cat.create_virtual_copy(copy)?;
    assert_eq!(cat.stack_of(a)?, [a, copy, third]);
    // Set Copy as Master changes no stack.
    cat.set_copy_as_master(copy)?;
    assert_eq!(cat.stack_of(a)?, [a, copy, third]);
    // Removing a copy takes it out of its stack.
    cat.remove_virtual_copy(third)?;
    assert_eq!(cat.stack_of(a)?, [a, copy]);
    Ok(())
}

#[test]
fn undo_puts_back_the_stacks_as_they_were() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW", "d.ARW", "e.ARW"])?;
    let [a, b, c, d, e] = ids[..] else { panic!() };
    let first = group(&mut cat, &[a, b, c])?;
    cat.set_stacks_collapsed(&[first], false)?;
    group(&mut cat, &[d, e])?;
    let selection = [b, d];
    let before = cat.stack_state(&selection)?;
    assert_eq!(before.photos.len(), 5);
    // The change reads both states in its own transaction, so Undo and
    // Redo get them whenever it is saved.
    let changed = cat.change_stacks(&StackChange::Group(vec![d, b]))?;
    let after = cat.stack_state(&before.photos)?;
    assert_eq!(changed.before.photos.len(), before.photos.len());
    assert!(
        before
            .stacks
            .iter()
            .all(|s| changed.before.stacks.contains(s))
    );
    assert_eq!(changed.after.stacks, after.stacks);
    assert_eq!(cat.stack_of(a)?, [a, c]);

    cat.change_stacks(&StackChange::Restore(before.clone()))?;
    assert_eq!(cat.stack_of(a)?, [a, b, c]);
    assert_eq!(cat.stack_of(e)?, [d, e]);
    assert!(!cat.stack_places()?[&a].collapsed);
    cat.change_stacks(&StackChange::Restore(after.clone()))?;
    assert_eq!(cat.stack_of(b)?, [d, b]);
    assert_eq!(cat.stack_of(c)?, [a, c]);
    assert!(cat.stack_of(e)?.is_empty());

    // A photo removed since is left out; a stack it leaves single is none.
    let mut state = cat.stack_state(&[d])?;
    cat.change_stacks(&StackChange::Unstack(vec![d]))?;
    remove(&mut cat, b)?;
    state.forget_photo(b);
    cat.change_stacks(&StackChange::Restore(state.clone()))?;
    assert!(cat.stack_of(d)?.is_empty());
    assert_eq!(cat.stack_of(c)?, [a, c]);
    Ok(())
}

#[test]
fn rows_another_release_changed_are_read_as_stacks_of_one_folder() -> Result<()> {
    let dir = tempfile::tempdir()?;
    for sub in ["x", "y"] {
        std::fs::create_dir_all(dir.path().join("photos").join(sub))?;
    }
    for name in ["x/a.ARW", "x/b.ARW", "x/c.ARW", "y/d.ARW"] {
        std::fs::write(dir.path().join("photos").join(name), name)?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&dir.path().join("photos"))?;
    let id = |name: &str| {
        cat.photos()
            .unwrap()
            .into_iter()
            .find(|p| p.filename == name)
            .unwrap()
            .id
    };
    let (a, b, c, d) = (id("a.ARW"), id("b.ARW"), id("c.ARW"), id("d.ARW"));
    group(&mut cat, &[a, b, c])?;
    let db = cat.db_for_tests();
    // A release without stacks moves the top to the other folder, and
    // removes c, as SQLite does with its foreign keys on.
    let y: i64 = db.query_row("SELECT folder FROM photos WHERE id=?", [d.0], |r| r.get(0))?;
    db.execute("UPDATE photos SET folder=? WHERE id=?", params![y, a.0])?;
    db.execute("PRAGMA foreign_keys=ON", [])?;
    db.execute("DELETE FROM photos WHERE id=?", [c.0])?;
    assert!(cat.stack_of(a)?.is_empty());
    assert!(cat.stack_of(b)?.is_empty(), "b is alone in its folder now");
    assert!(cat.stack_places()?.is_empty());
    Ok(())
}

#[test]
fn removing_a_photo_from_the_catalog_takes_it_out_of_its_stack() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW"])?;
    let [a, b, c] = ids[..] else { panic!() };
    group(&mut cat, &[a, b, c])?;
    remove(&mut cat, a)?;
    assert_eq!(cat.stack_of(b)?, [b, c]);
    assert!(cat.stack_places()?[&b].is_top());
    Ok(())
}

#[test]
fn auto_stack_joins_runs_of_photos_less_than_the_interval_apart() {
    let (f, g) = (FolderId(1), FolderId(2));
    let photos = [
        (PhotoId(1), f, "2024-03-02T10:00:00.000"),
        (PhotoId(2), f, "2024-03-02T10:00:09.000"),
        // 9 s after the one before, 18 s after the first: still the run.
        (PhotoId(3), f, "2024-03-02T10:00:18.000"),
        // Exactly 10 s: a new run.
        (PhotoId(4), f, "2024-03-02T10:00:28.000"),
        (PhotoId(5), f, "2024-03-02T10:00:28.500"),
        // Another folder, at the same time: never with the ones above.
        (PhotoId(6), g, "2024-03-02T10:00:28.200"),
        // No time: left alone.
        (PhotoId(7), f, ""),
        (PhotoId(8), f, "2024-03-02T10:30:00"),
        // Across midnight and a month's end.
        (PhotoId(9), g, "2024-03-31T23:59:59.900"),
        (PhotoId(10), g, "2024-04-01T00:00:00.100"),
    ];
    assert_eq!(
        auto_stacks(&photos, 10.),
        [
            vec![PhotoId(1), PhotoId(2), PhotoId(3)],
            vec![PhotoId(4), PhotoId(5)],
            vec![PhotoId(9), PhotoId(10)],
        ]
    );
    // No interval: nothing is stacked, even photos taken at the same time.
    assert!(auto_stacks(&photos, 0.).is_empty());
    let same = [
        (PhotoId(1), f, "2024-03-02T10:00:00"),
        (PhotoId(2), f, "2024-03-02T10:00:00"),
    ];
    assert_eq!(auto_stacks(&same, 1.), [vec![PhotoId(1), PhotoId(2)]]);
    assert_eq!(
        super::stacks::capture_seconds_for_tests("2024-13-02T10:00:00"),
        None
    );
}

#[test]
fn several_stacks_are_made_all_or_none() -> Result<()> {
    let (_dir, mut cat, ids, _) = catalog(&["a.ARW", "b.ARW", "c.ARW", "d.ARW"])?;
    let [a, b, c, d] = ids[..] else { panic!() };
    assert!(
        cat.change_stacks(&StackChange::Groups(vec![vec![a, b], vec![c]]))
            .is_err()
    );
    assert!(cat.stack_places()?.is_empty());
    cat.change_stacks(&StackChange::Groups(vec![vec![a, b], vec![c, d]]))?;
    assert_eq!(cat.stack_of(b)?, [a, b]);
    assert_eq!(cat.stack_of(d)?, [c, d]);
    Ok(())
}
