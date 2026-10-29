//! Stack Camera Bursts when adding photos: asked with the import batch, kept
//! until its stacks are made, and made once.
use super::*;
use crate::catalog::export_catalog::ExportScope;

fn stacking() -> NewPhotoDefaults {
    NewPhotoDefaults::default().with_options(ImportOptions {
        stack_bursts: true,
        ..Default::default()
    })
}

/// A catalog and a folder of `names` on disk, not added yet.
fn folder(names: &[&str]) -> Result<(tempfile::TempDir, Catalog, std::path::PathBuf)> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in names {
        std::fs::write(folder.join(name), b"synthetic raw")?;
    }
    let cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    Ok((dir, cat, folder))
}
fn ids(cat: &Catalog) -> Result<Vec<PhotoId>> {
    let mut ids: Vec<PhotoId> = cat.photos()?.iter().map(|p| p.id).collect();
    ids.sort();
    Ok(ids)
}
fn rows(cat: &Catalog) -> Result<i64> {
    Ok(cat
        .db_for_tests()
        .query_row("SELECT count(*) FROM import_burst_stacks", [], |r| r.get(0))?)
}

#[test]
fn a_batch_added_with_the_option_is_pending_with_its_photos() -> Result<()> {
    let (dir, mut cat, folder) = folder(&["a.arw", "b.arw"])?;
    cat.import_folder(&folder, &stacking(), &[])?;
    let ids = ids(&cat)?;
    let pending = cat.pending_burst_stacks()?;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].photos, ids);
    assert_eq!(pending[0].seconds, IMPORT_BURST_SECONDS);
    // Adding nothing new asks for nothing.
    cat.import_folder(&folder, &stacking(), &[])?;
    assert_eq!(cat.pending_burst_stacks()?.len(), 1);
    // Without the option, nothing is asked.
    let other = dir.path().join("other");
    std::fs::create_dir(&other)?;
    std::fs::write(other.join("c.arw"), b"synthetic raw")?;
    cat.import_folder(&other, &NewPhotoDefaults::default(), &[])?;
    assert_eq!(cat.pending_burst_stacks()?.len(), 1);
    // A photo removed leaves the batch; the batch stays to be done.
    cat.db.write(|w| super::copies::delete_photo(w, ids[0]))?;
    assert_eq!(cat.pending_burst_stacks()?[0].photos, [ids[1]]);
    Ok(())
}

#[test]
fn stacking_a_batch_makes_its_stacks_and_marks_it_done_once() -> Result<()> {
    let (_dir, mut cat, folder) = folder(&["a.arw", "b.arw", "c.arw", "d.arw"])?;
    cat.import_folder(&folder, &stacking(), &[])?;
    let ids = ids(&cat)?;
    let batch = cat.pending_burst_stacks()?[0].batch;
    let done = cat
        .stack_imported_bursts(&[batch], &[vec![ids[0], ids[1]], vec![ids[2], ids[3]]])?
        .expect("pending");
    assert_eq!(done.after.stacks.len(), 2);
    assert!(done.before.stacks.is_empty());
    assert_eq!(rows(&cat)?, 0);
    assert_eq!(cat.stack_of(ids[0])?, [ids[0], ids[1]]);
    // Done: asked again (a second completion), nothing changes.
    cat.change_stacks(&StackChange::Unstack(vec![ids[0]]))?;
    assert_eq!(
        cat.stack_imported_bursts(&[batch], &[vec![ids[0], ids[1]]])?,
        None
    );
    assert!(cat.stack_of(ids[0])?.is_empty());
    Ok(())
}

#[test]
fn only_masters_of_the_batch_in_no_stack_and_one_folder_are_stacked() -> Result<()> {
    let (dir, mut cat, folder) = folder(&["a.arw", "b.arw", "c.arw", "d.arw", "e.arw"])?;
    cat.import_folder(&folder, &stacking(), &[])?;
    let ids = ids(&cat)?;
    let batch = cat.pending_burst_stacks()?[0].batch;
    // Stacked meanwhile by hand, a virtual copy (stacked with its master),
    // and a photo of another batch.
    cat.change_stacks(&StackChange::Group(vec![ids[0], ids[1]]))?;
    let copy = cat.create_virtual_copy(ids[2])?;
    let other = dir.path().join("other");
    std::fs::create_dir(&other)?;
    std::fs::write(other.join("f.arw"), b"synthetic raw")?;
    cat.import_folder(&other, &NewPhotoDefaults::default(), &[])?;
    let outsider = *ids_of(&cat)?.iter().max().unwrap();
    let done = cat
        .stack_imported_bursts(
            &[batch],
            &[
                vec![ids[0], ids[3]],
                vec![copy, ids[4], outsider, ids[3], ids[4]],
                vec![ids[2], ids[3], ids[4]],
            ],
        )?
        .expect("pending");
    // Only the second keeps two photos or more, each once; the third's are
    // stacked already or taken by the second.
    assert_eq!(cat.stack_of(ids[4])?, [ids[4], ids[3]]);
    assert_eq!(cat.stack_of(ids[2])?, [ids[2], copy]);
    assert_eq!(cat.stack_of(ids[0])?, [ids[0], ids[1]]);
    assert!(cat.stack_of(outsider)?.is_empty());
    assert_eq!(done.after.stacks.len(), 1);
    Ok(())
}
fn ids_of(cat: &Catalog) -> Result<Vec<PhotoId>> {
    ids(cat)
}

#[test]
fn a_batch_with_nothing_to_stack_is_done_and_changes_nothing() -> Result<()> {
    let (_dir, mut cat, folder) = folder(&["a.arw"])?;
    cat.import_folder(&folder, &stacking(), &[])?;
    let batch = cat.pending_burst_stacks()?[0].batch;
    let done = cat.stack_imported_bursts(&[batch], &[])?.expect("pending");
    assert_eq!(done.before, done.after);
    assert_eq!(rows(&cat)?, 0);
    Ok(())
}

#[test]
fn an_export_leaves_out_bursts_still_to_stack_and_batches_can_still_go() -> Result<()> {
    let (dir, mut cat, folder) = folder(&["a.arw", "b.arw"])?;
    cat.import_folder(&folder, &stacking(), &[])?;
    let folder_id = cat.folders()?[0].id;
    let destination = dir.path().join("out/Copy.rawmakase");
    cat.export_catalog(&ExportScope::Folders(vec![folder_id]), &destination)?;
    assert_eq!(rows(&Catalog::open(&destination)?)?, 0);
    assert_eq!(rows(&cat)?, 1);
    // A release that doesn't know the table removes a batch: its row goes too.
    let conn = cat.db_for_tests();
    conn.execute_batch("DELETE FROM photo_imports; DELETE FROM import_batches;")?;
    assert_eq!(rows(&cat)?, 0);
    Ok(())
}

#[test]
fn a_photo_left_alone_in_its_old_stack_is_stacked() -> Result<()> {
    let (_dir, mut cat, folder) = folder(&["a.arw", "b.arw", "c.arw"])?;
    cat.import_folder(&folder, &stacking(), &[])?;
    let ids = ids(&cat)?;
    let batch = cat.pending_burst_stacks()?[0].batch;
    // a and c stacked, then c taken out: a keeps a row of a stack of one,
    // which is no stack.
    cat.change_stacks(&StackChange::Group(vec![ids[0], ids[2]]))?;
    cat.change_stacks(&StackChange::Remove(vec![ids[2]]))?;
    assert!(cat.stack_of(ids[0])?.is_empty());
    cat.stack_imported_bursts(&[batch], &[vec![ids[0], ids[1]]])?;
    assert_eq!(cat.stack_of(ids[0])?, [ids[0], ids[1]]);
    Ok(())
}

#[test]
fn import_options_keep_what_a_later_release_saved() -> Result<()> {
    let read: ImportOptions = serde_json::from_str(r#"{"stack_bursts":true,"later":{"x":1}}"#)?;
    assert!(read.stack_bursts);
    let saved = serde_json::to_value(&read)?;
    assert_eq!(saved["later"]["x"], 1);
    let none: ImportOptions = serde_json::from_str("{}")?;
    assert!(!none.stack_bursts);
    Ok(())
}
