use super::cell::photo_cell;
use super::tree::{FolderNode, TreeAction, folder_tree_row};
use super::*;
use eframe::egui::{Color32, Vec2};
use std::{collections::HashMap, path::PathBuf};
#[test]
fn develop_workspace_drains_library_preview_results() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("previews.rawmakase");
    drop(Catalog::create(&path)?);
    let ctx = egui::Context::default();
    let mut library = Library::load(&path, ctx.clone())?;
    let (tx, rx) = std::sync::mpsc::sync_channel(24);
    library.cache.thumb_rx = rx;
    for index in 0..24 {
        let path = directory.path().join(format!("{index}.ARW"));
        library.cache.pending.insert(path.clone());
        library.cache.progress.queued();
        tx.try_send(previews::PreviewResult {
            path,
            image: Some(image::RgbImage::new(16, 16)),
            cache_error: None,
        })?;
    }
    let mut editor = crate::app::Editor::with_context(&ctx, None, Default::default(), None);
    editor.library = Some(Box::new(library));
    editor.library_mode = false;
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| editor.draw(ui));
    output.textures_delta.clear();
    let library = editor.library.as_ref().unwrap();
    assert!(library.cache.pending.is_empty());
    assert_eq!(library.cache.thumbs.len(), 24);
    assert!(!editor.library_mode);
    Ok(())
}

#[test]
fn photo_cells_preserve_texture_proportions_at_different_grid_widths() {
    let ctx = egui::Context::default();
    let photo = Photo {
        id: 1,
        folder: 1,
        path: "photo.RAF".into(),
        filename: "photo.RAF".into(),
        captured: String::new(),
        rating: 0,
        flag: 0,
        label: String::new(),
        format: "RAF".into(),
        copy_name: String::new(),
        master: None,
        keywords: String::new(),
        has_lightroom_edits: false,
    };
    for size in [[360, 240], [160, 240], [240, 240], [360, 90]] {
        let texture = ctx.load_texture(
            "aspect-test",
            egui::ColorImage::filled(size, Color32::WHITE),
            Default::default(),
        );
        for width in [80., 190., 260.] {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                let shown = cell::Shown {
                    mark: selection::Mark::None,
                    number: 1,
                    available: true,
                    quick: false,
                };
                photo_cell(ui, &photo, Some(&texture), shown, width);
            });
            let mesh = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::epaint::Shape::Mesh(mesh) if mesh.texture_id == texture.id() => {
                        Some(mesh)
                    }
                    _ => None,
                })
                .expect("photo must be painted");
            let bounds = mesh.calc_bounds();
            let expected = size[0] as f32 / size[1] as f32;
            assert!((bounds.width() / bounds.height() - expected).abs() < 0.0001);
            assert!(bounds.width() <= width * 0.84 + 0.001);
            assert!(bounds.height() <= width - 20.);
            assert_eq!(mesh.vertices[0].uv, egui::Pos2::ZERO);
            assert_eq!(mesh.vertices[3].uv, egui::pos2(1., 1.));
            output.textures_delta.clear();
        }
    }
}
#[test]
fn metadata_edits_persist_toggle_and_advance_through_filtered_photos() -> Result<()> {
    use crate::app::photo_metadata::Edit;
    let d = tempfile::tempdir()?;
    let folder = d.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in ["a.RAF", "b.RAF", "c.RAF"] {
        std::fs::write(folder.join(name), b"fixture")?;
    }
    let path = d.path().join("metadata.rawmakase");
    let mut catalog = Catalog::create(&path)?;
    catalog.add_folder(&folder)?;
    drop(catalog);
    let mut library = Library::load(&path, egui::Context::default())?;
    let ids: Vec<_> = library.photos.iter().map(|p| p.id).collect();
    library.select(Some(ids[0]));
    library.edit_metadata(ids[0], Edit::Rating(5), false)?;
    library.edit_metadata(ids[0], Edit::Flag(1), false)?;
    library.edit_metadata(ids[0], Edit::Label("Client approved".into()), false)?;
    library.edit_metadata(ids[0], Edit::RatingDelta(1), false)?;
    assert_eq!(library.photo(ids[0]).unwrap().rating, 5);
    assert_eq!(library.photo(ids[0]).unwrap().label, "Client approved");
    assert!(library.labels().contains(&"Client approved".into()));
    library.edit_metadata(ids[0], Edit::ToggleLabel("Red".into()), false)?;
    library.edit_metadata(ids[0], Edit::ToggleLabel("Red".into()), false)?;
    assert_eq!(library.photo(ids[0]).unwrap().label, "");
    library.filters.flag = 0;
    library.filter();
    library.select(Some(ids[1]));
    assert_eq!(
        library.edit_metadata(ids[1], Edit::Flag(-1), true)?,
        Some(ids[2])
    );
    assert_eq!(library.selected(), Some(ids[2]));
    assert_eq!(library.visible.len(), 1);
    assert_eq!(library.edit_metadata(ids[2], Edit::Flag(1), true)?, None);
    assert_eq!(library.selected(), None);
    assert!(library.visible.is_empty());
    library.filters.flag = 2;
    library.filters.label_filter = Some("Purple".into());
    library.filter();
    library.edit_metadata(ids[2], Edit::Label("Purple".into()), false)?;
    assert_eq!(library.visible.len(), 1);
    assert!(
        library
            .edit_metadata(ids[2], Edit::Rating(10), false)
            .is_err()
    );
    assert_eq!(library.photo(ids[2]).unwrap().rating, 0);
    library.refresh()?;
    assert_eq!(library.photo(ids[0]).unwrap().rating, 5);
    assert_eq!(library.photo(ids[1]).unwrap().flag, -1);
    assert_eq!(library.photo(ids[2]).unwrap().label, "Purple");
    assert!(std::fs::read_dir(&folder)?.all(|e| e.unwrap().path().extension().unwrap() == "RAF"));
    Ok(())
}
#[test]
fn tree_locate_action_uses_the_clicked_root() {
    let ctx = egui::Context::default();
    let root = FolderNode::root(42, "Photos".into(), "/missing".into());
    let mut expanded = HashSet::new();
    let mut target = egui::Pos2::ZERO;
    let mut located = None;
    for frame in 0..3 {
        let events = if frame == 0 {
            vec![]
        } else {
            vec![
                egui::Event::PointerMoved(target),
                egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: frame == 1,
                    modifiers: egui::Modifiers::NONE,
                },
            ]
        };
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(360., 240.),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                target = egui::Pos2::new(
                    ui.available_rect_before_wrap().right() - 12.,
                    ui.cursor().top() + 14.5,
                );
                if let Some(TreeAction::Relink(is_root, id)) =
                    folder_tree_row(ui, &root, 0, &mut expanded, "")
                {
                    located = Some((is_root, id));
                }
            },
        );
        output.textures_delta.clear();
    }
    assert_eq!(located, Some((true, 42)));
}
#[test]
fn batched_availability_distinguishes_files_directories_and_missing_paths() -> Result<()> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("present.ARW"), b"raw")?;
    let mut c = Catalog::create(&d.path().join("catalog.rawmakase"))?;
    c.add_folder(&photos)?;
    let mut rows = c.photos()?;
    let mut missing = rows[0].clone();
    missing.path = photos.join("missing.RAF");
    rows.push(missing);
    let mut directory = rows[0].clone();
    directory.path = photos.join("directory.ARW");
    std::fs::create_dir(&directory.path)?;
    rows.push(directory);
    assert_eq!(
        thumbnails::available_paths(&rows),
        HashSet::from([photos.join("present.ARW").canonicalize()?])
    );
    Ok(())
}
#[test]
fn hierarchy_includes_unregistered_parents_and_descendant_counts() {
    let mut root = FolderNode::root(1, "Photos".into(), "/old".into());
    for (id, relative, count) in [(10, "2026/09/Trip/", 2), (11, "2026/10/", 3)] {
        root.insert(&Folder {
            id,
            root: 1,
            name: relative.into(),
            relative: relative.into(),
            path: PathBuf::from("/old").join(relative),
            count,
        });
    }
    root.finish();
    assert_eq!(root.count, 5);
    assert_eq!(root.children.len(), 1);
    let year = &root.children["2026"];
    assert_eq!(year.count, 5);
    assert_eq!(year.ids, HashSet::from([10, 11]));
    assert_eq!(year.children["09"].children["Trip"].folder, Some(10));
}
#[test]
fn root_mapping_survives_reopen() -> Result<()> {
    let d = tempfile::tempdir()?;
    let old = d.path().join("old");
    let new = d.path().join("new");
    std::fs::create_dir(&old)?;
    std::fs::write(old.join("image.ARW"), b"source")?;
    let db = d.path().join("photos.rawmakase");
    let mut c = Catalog::create(&db)?;
    c.add_folder(&old)?;
    let root = c.roots()?[0].0;
    drop(c);
    std::fs::rename(&old, &new)?;
    let ctx = egui::Context::default();
    let l = Library::load(&db, ctx.clone())?;
    l.catalog.relink_root(root, &new)?;
    drop(l);
    let mut l = Library::load(&db, ctx)?;
    assert_eq!(l.photos[0].path, new.join("image.ARW"));
    l.wait_for_availability();
    assert!(l.is_available(&l.photos[0].path));
    Ok(())
}

/// Listing every folder can take seconds on a network share, so the Library
/// opens first and marks missing originals once the check is done.
#[test]
fn library_opens_before_the_online_check_and_then_marks_missing_photos() -> Result<()> {
    let d = tempfile::tempdir()?;
    std::fs::write(d.path().join("kept.ARW"), b"source")?;
    std::fs::write(d.path().join("gone.ARW"), b"source")?;
    let db = d.path().join("photos.rawmakase");
    Catalog::create(&db)?.add_folder(d.path())?;
    std::fs::remove_file(d.path().join("gone.ARW"))?;
    let mut l = Library::load(&db, egui::Context::default())?;
    let gone = d.path().canonicalize()?.join("gone.ARW");
    assert!(l.photos.iter().any(|p| p.path == gone));
    assert_eq!(l.available_count(), 2);
    l.filters.only_missing = true;
    l.filter();
    assert!(l.visible.is_empty());
    l.wait_for_availability();
    assert_eq!(l.available_count(), 1);
    assert!(!l.is_available(&gone));
    assert_eq!(l.visible.len(), 1);
    Ok(())
}

#[test]
fn thumbnails_keep_portrait_and_landscape_proportions() {
    use super::thumbnails::fit;
    assert_eq!(fit(4000, 6000, 640), (427, 640));
    assert_eq!(fit(6000, 4000, 640), (640, 427));
    assert_eq!(fit(300, 200, 640), (300, 200));
}

#[test]
fn a_stuck_volume_check_is_not_started_again() {
    use std::sync::{Arc, Mutex, mpsc};
    let online = Arc::new(Mutex::new(HashMap::new()));
    let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (release, stalled) = mpsc::channel::<()>();
    let stalled = Mutex::new(stalled);
    let (changed_tx, changed) = mpsc::channel();
    let mounts = || vec![PathBuf::from("/Volumes/Stalled")];
    // A probe that hangs, as `is_dir` can on a stalled mount.
    assert!(volumes::spawn_volume_check(
        &online,
        &busy,
        mounts(),
        move || changed_tx.send(()).unwrap(),
        move |_| {
            stalled.lock().unwrap().recv().unwrap();
            (true, None)
        },
    ));
    for _ in 0..3 {
        assert!(!volumes::spawn_volume_check(
            &online,
            &busy,
            mounts(),
            || {},
            |_| unreachable!("a second check started"),
        ));
    }
    release.send(()).unwrap();
    changed
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    assert_eq!(
        online
            .lock()
            .unwrap()
            .get(&PathBuf::from("/Volumes/Stalled")),
        Some(&(true, None))
    );
    // Once it finishes, the next check runs.
    let started = std::time::Instant::now();
    while busy.load(std::sync::atomic::Ordering::Acquire) {
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        std::thread::yield_now();
    }
    assert!(volumes::spawn_volume_check(
        &online,
        &busy,
        mounts(),
        || {},
        |_| (false, None)
    ));
}
#[test]
fn copy_previews_ignore_stale_results_and_reuse_of_a_removed_id() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("image.ARW"), b"synthetic raw")?;
    let path = directory.path().join("copies.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    let mut library = Library::load(&path, ctx.clone())?;
    let master = library.photos[0].id;
    let (tx, rx) = std::sync::mpsc::channel();
    library.cache.edit_rx = rx;
    let copy = library.create_virtual_copy(master)?;
    library.cache.edited_requested.insert(copy, 7);
    library.cache.edit_seen.insert(copy);
    // A result for an older request is dropped.
    tx.send(previews::EditResult::Ready(
        copy,
        6,
        image::RgbImage::new(4, 4),
    ))?;
    library.poll_previews(&ctx);
    assert!(!library.has_edited_thumbnail(copy));
    tx.send(previews::EditResult::Ready(
        copy,
        7,
        image::RgbImage::new(4, 4),
    ))?;
    library.poll_previews(&ctx);
    assert!(library.has_edited_thumbnail(copy));
    // Removing the copy forgets it, so a new copy given its id renders again,
    // and the removed copy's late result is dropped.
    assert_eq!(library.remove_virtual_copy(copy)?, Some(master));
    assert!(!library.cache.edited_requested.contains_key(&copy));
    assert!(!library.cache.edited_order.contains(&copy));
    assert!(!library.cache.edit_seen.contains(&copy));
    tx.send(previews::EditResult::Ready(
        copy,
        7,
        image::RgbImage::new(4, 4),
    ))?;
    library.poll_previews(&ctx);
    assert!(!library.has_edited_thumbnail(copy));
    Ok(())
}
#[test]
fn a_copy_name_being_typed_is_saved_when_committed() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("image.ARW"), b"synthetic raw")?;
    let path = directory.path().join("names.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let mut library = Library::load(&path, egui::Context::default())?;
    let copy = library.create_virtual_copy(library.photos[0].id)?;
    library.copy_names.draft = Some((copy, " B&W ".into()));
    library.commit_copy_name()?;
    let saved = library.catalog.photos()?;
    assert_eq!(
        saved.iter().find(|p| p.id == copy).unwrap().copy_name,
        "B&W"
    );
    assert_eq!(library.photo(copy).unwrap().copy_name, "B&W");
    // A removed copy's draft never renames a new copy that reuses its id.
    library.remove_virtual_copy(copy)?;
    let next = library.create_virtual_copy(library.photos[0].id)?;
    library.commit_copy_name()?;
    assert_eq!(library.photo(next).unwrap().copy_name, "Copy 1");
    Ok(())
}
#[test]
fn selecting_another_copy_keeps_the_name_being_typed() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("image.ARW"), b"synthetic raw")?;
    let path = directory.path().join("names.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    let mut library = Library::load(&path, ctx.clone())?;
    let master = library.photos[0].id;
    let first = library.create_virtual_copy(master)?;
    let second = library.create_virtual_copy(master)?;
    library.copy_names.draft = Some((first, "B&W".into()));
    // The panel is drawn for the newly selected copy before the field
    // reports losing focus.
    let photo = library.photo(second).unwrap().clone();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let _ = library
            .copy_names
            .row(ui, &photo, &library.catalog, &mut library.photos);
    });
    output.textures_delta.clear();
    assert_eq!(library.photo(first).unwrap().copy_name, "B&W");
    assert_eq!(library.copy_names.draft, Some((second, "Copy 2".into())));
    Ok(())
}
#[test]
fn a_copy_name_that_fails_to_save_survives_selecting_another_copy() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("image.ARW"), b"synthetic raw")?;
    let path = directory.path().join("names.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    let mut library = Library::load(&path, ctx.clone())?;
    let master = library.photos[0].id;
    let first = library.create_virtual_copy(master)?;
    let second = library.create_virtual_copy(master)?;
    // Renaming fails once the copy is gone from the catalog.
    library.catalog.remove_virtual_copy(first)?;
    library.copy_names.draft = Some((first, "B&W".into()));
    let photo = library.photo(second).unwrap().clone();
    for _ in 0..2 {
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _ = library
                .copy_names
                .row(ui, &photo, &library.catalog, &mut library.photos);
        });
        output.textures_delta.clear();
    }
    assert_eq!(library.copy_names.draft, Some((first, "B&W".into())));
    assert!(library.commit_copy_name().is_err());
    library.discard_copy_name();
    assert!(library.commit_copy_name().is_ok());
    Ok(())
}

/// A catalog of `names` in one folder, with the Library open on it.
fn library_of(names: &[&str]) -> Result<(tempfile::TempDir, Library)> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in names {
        std::fs::write(folder.join(name), b"synthetic raw")?;
    }
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let library = Library::load(&path, egui::Context::default())?;
    Ok((directory, library))
}
fn visible_names(library: &Library) -> Vec<&str> {
    library
        .visible
        .iter()
        .map(|i| library.photos[*i].filename.as_str())
        .collect()
}
#[test]
fn filters_combine_and_a_hidden_selection_is_cleared() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF"])?;
    let id = |library: &Library, name: &str| {
        library
            .photos
            .iter()
            .find(|p| p.filename == name)
            .unwrap()
            .id
    };
    let (a, b, c) = (
        id(&library, "a.RAF"),
        id(&library, "b.RAF"),
        id(&library, "c.RAF"),
    );
    library.catalog.set_metadata(a, 3, 1, "Red")?;
    library.catalog.set_metadata(b, 5, 0, "")?;
    library.catalog.set_metadata(c, 0, -1, "Client")?;
    library.refresh()?;
    library.wait_for_availability();
    assert_eq!(visible_names(&library), ["a.RAF", "b.RAF", "c.RAF"]);
    library.filters.reverse = true;
    library.filter();
    assert_eq!(visible_names(&library), ["c.RAF", "b.RAF", "a.RAF"]);
    library.filters.reverse = false;
    library.filters.rating = 3;
    library.filter();
    assert_eq!(visible_names(&library), ["a.RAF", "b.RAF"]);
    library.filters.flag = 1;
    library.filter();
    assert_eq!(visible_names(&library), ["a.RAF"]);
    library.filters.flag = 2;
    library.filters.rating = 0;
    library.filters.label_filter = Some("Client".into());
    library.filter();
    assert_eq!(visible_names(&library), ["c.RAF"]);
    library.filters.label_filter = None;
    library.filters.query = "B.r".into();
    library.filter();
    assert_eq!(visible_names(&library), ["b.RAF"]);
    library.filters.query = "client".into();
    library.filter();
    assert_eq!(visible_names(&library), ["c.RAF"]);
    library.filters.query.clear();
    library.filters.folder_scope = Some(HashSet::new());
    library.filter();
    assert!(library.visible.is_empty());
    library.filters.folder_scope = None;
    library.filters.only_missing = true;
    library.filter();
    assert!(library.visible.is_empty());
    library.filters.only_missing = false;
    // The selection follows the filter out, and comes back through `show`.
    library.select(Some(c));
    library.filters.flag = 1;
    library.filter();
    assert_eq!(library.selected(), None);
    library.show(c);
    assert_eq!(library.filters.flag, 2);
    assert_eq!(library.selected(), Some(c));
    assert_eq!(visible_names(&library).len(), 3);
    // Navigation clamps at both ends of the visible order.
    assert_eq!(library.navigate(a, -1), Some(a));
    assert_eq!(library.navigate(a, 2), Some(c));
    assert_eq!(library.navigate(c, 5), Some(c));
    assert_eq!(library.navigate(999, 1), None);
    Ok(())
}
#[test]
fn restore_source_scopes_to_the_folder_and_its_subfolders() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let root = directory.path().join("photos");
    for sub in ["", "trip", "trip/day2", "other"] {
        let folder = root.join(sub);
        std::fs::create_dir_all(&folder)?;
        std::fs::write(folder.join("image.RAF"), b"synthetic raw")?;
    }
    let path = directory.path().join("sources.rawmakase");
    Catalog::create(&path)?.add_folder(&root)?;
    let mut library = Library::load(&path, egui::Context::default())?;
    assert_eq!(library.visible.len(), 4);
    let root_id = library.roots[0].0;
    let in_day2 = library
        .photos
        .iter()
        .find(|p| p.path.ends_with("day2/image.RAF"))
        .unwrap()
        .id;
    let key = format!("root:{root_id}/trip");
    library.restore_source(&key, Some(in_day2));
    assert_eq!(library.source_key(), key);
    assert_eq!(library.visible.len(), 2);
    assert_eq!(library.selected(), Some(in_day2));
    assert!(library.expanded.contains(&format!("root:{root_id}")));
    assert!(library.expanded.contains(&key));
    assert_eq!(library.source_name(), "trip");
    // A folder that is not in the catalog leaves the scope as it was.
    library.restore_source("root:999/elsewhere", None);
    assert_eq!(library.source_key(), key);
    assert_eq!(library.visible.len(), 2);
    library.restore_source("", None);
    assert_eq!(library.visible.len(), 2);
    Ok(())
}
#[test]
fn thumbnail_requests_are_not_repeated_while_pending_or_failed() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF"])?;
    let ctx = library.ctx.clone();
    let (tx, rx) = std::sync::mpsc::sync_channel(8);
    library.cache.thumb_tx = tx;
    let path = library.photos[0].path.clone();
    library.cache.request_thumbnail(&path, &ctx);
    library.cache.request_thumbnail(&path, &ctx);
    assert_eq!(rx.try_iter().count(), 1);
    assert!(library.cache.pending.contains(&path));
    library.cache.pending.remove(&path);
    library.cache.failed.insert(path.clone());
    library.cache.request_thumbnail(&path, &ctx);
    assert_eq!(rx.try_iter().count(), 0);
    library.cache.failed.clear();
    library
        .cache
        .insert_thumb(&ctx, path.clone(), &image::RgbImage::new(2, 2));
    library.cache.request_thumbnail(&path, &ctx);
    assert_eq!(rx.try_iter().count(), 0);
    assert!(library.texture(&library.photos[0]).is_some());
    Ok(())
}
#[test]
fn preview_textures_keep_the_newest_192() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF"])?;
    let ctx = library.ctx.clone();
    let image = image::RgbImage::new(2, 2);
    for n in 0..193 {
        library
            .cache
            .insert_thumb(&ctx, PathBuf::from(format!("{n}.RAF")), &image);
        library.cache.edited_requested.insert(n, 1);
        library.cache.insert_edited(&ctx, n, &image);
    }
    assert_eq!(library.cache.thumbs.len(), 192);
    assert!(!library.cache.thumbs.contains_key(&PathBuf::from("0.RAF")));
    assert!(library.cache.thumbs.contains_key(&PathBuf::from("192.RAF")));
    assert_eq!(library.cache.edited.len(), 192);
    assert!(!library.has_edited_thumbnail(0));
    assert!(!library.cache.edited_requested.contains_key(&0));
    assert!(library.has_edited_thumbnail(192));
    // Replacing a texture does not count as a new one.
    library
        .cache
        .insert_thumb(&ctx, PathBuf::from("192.RAF"), &image);
    assert_eq!(library.cache.thumb_order.len(), 192);
    Ok(())
}
#[test]
fn an_edited_preview_from_develop_outranks_renders_in_flight() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF"])?;
    let ctx = library.ctx.clone();
    let id = library.photos[0].id;
    let (job_tx, jobs) = std::sync::mpsc::channel();
    let (result_tx, results) = std::sync::mpsc::channel();
    let (thumb_tx, _thumbs) = std::sync::mpsc::sync_channel(8);
    library.cache.edit_tx = job_tx;
    library.cache.edit_rx = results;
    library.cache.thumb_tx = thumb_tx;
    // A photo without an edit asks the catalog once and sends nothing.
    let photo = library.photos[0].clone();
    library.request_previews(&photo, &ctx);
    library.request_previews(&photo, &ctx);
    assert!(jobs.try_recv().is_err());
    assert_eq!(library.cache.edits_pending, 0);
    let first = library.cache.edited_requested[&id];
    // Develop's render arrives: it is shown, kept, and outranks `first`.
    library.update_edited(&ctx, id, image::RgbImage::new(4, 4), "{}".into());
    assert!(library.has_edited_thumbnail(id));
    assert!(matches!(
        jobs.try_recv(),
        Ok(previews::EditJob::Store { .. })
    ));
    let newest = library.cache.edited_requested[&id];
    assert!(newest > first);
    library.cache.edits_pending = 1;
    result_tx.send(previews::EditResult::Skipped(id, first))?;
    library.poll_previews(&ctx);
    assert_eq!(library.cache.edited_requested.get(&id), Some(&newest));
    assert_eq!(library.cache.edits_pending, 0);
    // A cache error is reported without touching the previews.
    result_tx.send(previews::EditResult::CacheError("disk full".into()))?;
    library.poll_previews(&ctx);
    assert!(library.preview_progress_active());
    assert!(library.has_edited_thumbnail(id));
    Ok(())
}
#[test]
fn collections_panel_shows_imported_collections_and_filters_through_them() -> Result<()> {
    let (directory, library) = library_of(&["a.RAF", "b.RAF", "c.RAF"])?;
    let path = library.catalog.path.clone();
    let ids: Vec<i64> = library.photos.iter().map(|p| p.id).collect();
    drop(library);
    {
        let db = rusqlite::Connection::open(&path)?;
        db.execute_batch(
            "INSERT INTO collections VALUES
                (2,'quick collection',NULL,'com.adobe.ag.library.collection'),
                (3,'Smart Collections',NULL,'com.adobe.ag.library.group'),
                (4,'Five Stars',3,'com.adobe.ag.library.smart_collection'),
                (5,'Unsaved Print',NULL,'com.adobe.ag.print.unsaved'),
                (10,'Trips',NULL,'com.adobe.ag.library.group'),
                (11,'Japan',10,'com.adobe.ag.library.collection'),
                (12,'Alps',10,'com.adobe.ag.library.collection'),
                (13,'Empty set',NULL,'com.adobe.ag.library.group'),
                (14,'Archive',NULL,'com.adobe.ag.library.collection');",
        )?;
        for photo in &ids[..2] {
            db.execute("INSERT INTO collection_photos VALUES(11,?,NULL)", [photo])?;
            db.execute("INSERT INTO collection_photos VALUES(2,?,NULL)", [photo])?;
        }
    }
    let mut library = Library::load(&path, egui::Context::default())?;
    let tree = collections::tree(&library.collections, &library.collection_photos);
    // Sets first, then collections, by name; smart and system ones hidden,
    // and a set left empty by hiding them is dropped too.
    let names: Vec<_> = tree.iter().map(|n| n.name.as_str()).collect();
    assert_eq!(names, ["Empty set", "Trips", "Archive"]);
    let trips: Vec<_> = tree[1]
        .children
        .iter()
        .map(|n| (n.name.as_str(), n.count))
        .collect();
    assert_eq!(trips, [("Alps", 0), ("Japan", 2)]);

    library.select_collection(11);
    assert_eq!(library.visible.len(), 2);
    assert_eq!(library.source_name(), "Japan");
    assert_eq!(library.source_key(), "collection:11");
    // The filter bar still applies inside a collection.
    library.catalog.set_metadata(ids[0], 0, 1, "")?;
    library.reload()?;
    library.filters.flag = 1;
    library.filter();
    assert_eq!(library.visible.len(), 1);
    library.filters.flag = 2;
    library.filter();

    // A saved collection comes back with the session; an unknown one doesn't.
    let mut restored = Library::load(&path, egui::Context::default())?;
    restored.restore_source("collection:11", Some(ids[1]));
    assert_eq!(restored.visible.len(), 2);
    assert_eq!(restored.selected(), Some(ids[1]));
    let mut other = Library::load(&path, egui::Context::default())?;
    other.restore_source("collection:4", None);
    assert_eq!(other.filters.collection, None);
    assert_eq!(other.visible.len(), 3);

    // Choosing a folder clears the collection.
    let root = library.roots[0].0;
    library.restore_source(&format!("root:{root}"), None);
    assert_eq!(library.filters.collection, None);
    assert_eq!(library.visible.len(), 3);
    drop(directory);
    Ok(())
}
#[test]
fn capture_times_are_read_in_the_background_and_resort_in_place() -> Result<()> {
    use crate::export::exif::dated_file;
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    // File names sort the other way round from capture times.
    std::fs::write(
        folder.join("a.tif"),
        dated_file(false, "2024:03:02 10:00:02", ""),
    )?;
    std::fs::write(
        folder.join("b.jpg"),
        dated_file(true, "2024:03:02 10:00:01", "9"),
    )?;
    std::fs::write(
        folder.join("c.jpg"),
        dated_file(true, "2024:03:02 10:00:01", "1"),
    )?;
    std::fs::write(folder.join("z.ARW"), b"synthetic raw")?;
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let mut library = Library::load(&path, egui::Context::default())?;
    assert_eq!(
        visible_names(&library),
        ["a.tif", "b.jpg", "c.jpg", "z.ARW"]
    );
    let a = library.photos[0].id;
    library.select(Some(a));
    library.wait_for_availability();
    let started = std::time::Instant::now();
    while library.capture.is_some() {
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        library.poll_capture_times();
        std::thread::yield_now();
    }
    // The undated file sorts first, and subseconds order the same second.
    assert_eq!(
        visible_names(&library),
        ["z.ARW", "c.jpg", "b.jpg", "a.tif"]
    );
    assert_eq!(library.photos[1].captured, "2024-03-02T10:00:01.100");
    // The selection stays, and the grid follows it from where it was.
    assert_eq!(library.selected(), Some(a));
    assert_eq!(library.keep_in_place, Some((a, 0)));
    // Saved in the catalog; the undated file isn't read again this session.
    let reopened = Library::load(&path, egui::Context::default())?;
    assert_eq!(
        visible_names(&reopened),
        ["z.ARW", "c.jpg", "b.jpg", "a.tif"]
    );
    library.start_capture_times();
    assert!(library.capture.is_none());
    Ok(())
}
fn ids_of(library: &Library) -> Vec<i64> {
    library
        .visible
        .iter()
        .map(|i| library.photos[*i].id)
        .collect()
}
fn selected_names(library: &Library) -> Vec<&str> {
    library
        .selected_ids()
        .into_iter()
        .map(|id| library.photo(id).unwrap().filename.as_str())
        .collect()
}
#[test]
fn clicks_select_like_lightroom_grid() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF", "e.RAF"])?;
    let [a, b, c, d, e] = ids_of(&library)[..] else {
        unreachable!()
    };
    let none = egui::Modifiers::NONE;
    let command = egui::Modifiers::COMMAND;
    let shift = egui::Modifiers::SHIFT;
    library.click(b, none);
    library.click(d, shift);
    assert_eq!(selected_names(&library), ["b.RAF", "c.RAF", "d.RAF"]);
    assert_eq!(library.selected(), Some(d));
    // Cmd toggles one photo; Cmd+Shift adds a range from the anchor.
    library.click(a, command);
    assert_eq!(
        selected_names(&library),
        ["a.RAF", "b.RAF", "c.RAF", "d.RAF"]
    );
    assert_eq!(library.selected(), Some(a));
    library.click(c, command);
    assert_eq!(selected_names(&library), ["a.RAF", "b.RAF", "d.RAF"]);
    assert_eq!(library.selected(), Some(a));
    library.click(e, command | shift);
    assert_eq!(selected_names(&library).len(), 5);
    // A plain click inside the selection only makes that photo active;
    // outside it, it selects the photo alone.
    library.click(b, none);
    assert_eq!(library.selected_ids().len(), 5);
    assert_eq!(library.selected(), Some(b));
    library.click(c, command);
    assert_eq!(library.mark(b), selection::Mark::Active);
    assert_eq!(library.mark(a), selection::Mark::Selected);
    assert_eq!(library.mark(c), selection::Mark::None);
    library.click(c, none);
    assert_eq!(selected_names(&library), ["c.RAF"]);
    Ok(())
}
#[test]
fn grid_keys_move_extend_and_clear_the_selection() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF", "e.RAF"])?;
    let ids = ids_of(&library);
    library.grid_columns = 2;
    library.select(Some(ids[0]));
    library.step(selection::Step::By(2), false);
    assert_eq!(library.selected(), Some(ids[2]));
    assert!(library.scroll_to_active);
    library.step(selection::Step::By(1), true);
    library.step(selection::Step::End, true);
    assert_eq!(selected_names(&library), ["c.RAF", "d.RAF", "e.RAF"]);
    assert_eq!(library.selected(), Some(ids[4]));
    // `/` drops the active photo; the next selected one takes over.
    library.deselect_active();
    assert_eq!(selected_names(&library), ["c.RAF", "d.RAF"]);
    assert_eq!(library.selected(), Some(ids[3]));
    library.select_all();
    assert_eq!(library.selected_ids(), ids);
    library.step(selection::Step::Home, false);
    assert_eq!(selected_names(&library), ["a.RAF"]);
    library.select(None);
    assert!(library.selected_ids().is_empty());
    // From nothing, a step starts at the first photo.
    library.step(selection::Step::By(1), false);
    assert_eq!(library.selected(), Some(ids[0]));
    Ok(())
}
#[test]
fn a_rejected_range_leaves_the_unflagged_view_in_one_write() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF", "e.RAF"])?;
    let ids = ids_of(&library);
    library.filters.flag = 0;
    library.filter();
    library.click(ids[1], egui::Modifiers::NONE);
    library.click(ids[3], egui::Modifiers::SHIFT);
    library.edit_selection(crate::app::photo_metadata::Edit::Flag(-1), false)?;
    assert_eq!(visible_names(&library), ["a.RAF", "e.RAF"]);
    assert_eq!(library.selected(), Some(ids[4]));
    assert_eq!(library.message, "3 photos · Reject");
    let saved = library.catalog.photos()?;
    assert_eq!(saved.iter().filter(|p| p.flag == -1).count(), 3);
    // One failing photo saves none of the batch.
    assert!(
        library
            .catalog
            .set_metadata_of(&[(ids[0], 5, 0, String::new()), (9999, 5, 0, String::new())])
            .is_err()
    );
    assert!(library.catalog.photos()?.iter().all(|p| p.rating == 0));
    Ok(())
}
#[test]
fn a_toggle_on_a_selection_follows_the_active_photo() -> Result<()> {
    use crate::app::photo_metadata::Edit;
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF"])?;
    let ids = ids_of(&library);
    library.edit_metadata(ids[0], Edit::Flag(1), false)?;
    library.select(Some(ids[0]));
    library.select_all();
    // The active photo is picked, so the toggle unflags all three.
    library.edit_selection(Edit::TogglePick, false)?;
    assert!(library.photos.iter().all(|p| p.flag == 0));
    library.edit_selection(Edit::ToggleLabel("Red".into()), false)?;
    assert!(library.photos.iter().all(|p| p.label == "Red"));
    // Shift does not advance a multi-photo selection.
    assert_eq!(library.edit_selection(Edit::Rating(3), true)?, None);
    assert_eq!(library.selected_ids().len(), 3);
    Ok(())
}
#[test]
fn up_and_down_stay_in_their_column_at_the_edges() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF", "d.RAF", "e.RAF"])?;
    let ids = ids_of(&library);
    library.grid_columns = 4;
    library.select(Some(ids[3]));
    library.step(selection::Step::By(-4), false);
    assert_eq!(library.selected(), Some(ids[3]));
    library.step(selection::Step::By(4), false);
    assert_eq!(library.selected(), Some(ids[3]));
    library.select(Some(ids[0]));
    library.step(selection::Step::By(4), false);
    assert_eq!(library.selected(), Some(ids[4]));
    Ok(())
}
#[test]
fn a_hidden_active_photo_hands_over_to_the_rest_of_the_selection() -> Result<()> {
    let (_directory, mut library) = library_of(&["a.RAF", "b.RAF", "c.RAF"])?;
    let ids = ids_of(&library);
    library.edit_metadata(ids[1], crate::app::photo_metadata::Edit::Rating(3), false)?;
    library.edit_metadata(ids[2], crate::app::photo_metadata::Edit::Rating(3), false)?;
    library.select_all();
    library.select(Some(ids[0]));
    library.select_all();
    library.filters.rating = 3;
    library.filter();
    assert_eq!(library.selected(), Some(ids[1]));
    assert_eq!(library.selected_ids(), ids[1..]);
    Ok(())
}
#[test]
fn loupe_shows_a_jpeg_at_the_size_of_the_view() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    image::RgbImage::from_pixel(3000, 2000, image::Rgb([200, 120, 40]))
        .save(folder.join("a.jpg"))?;
    std::fs::write(folder.join("b.jpg"), b"not a jpeg")?;
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    let mut library = Library::load(&path, ctx.clone())?;
    library.wait_for_availability();
    library.select(Some(library.photos[0].id));
    library.open_loupe();
    assert!(library.loupe_open());
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1200., 800.),
        )),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input(), |ui| {
        library.grid(ui, &mut Default::default());
    });
    output.textures_delta.clear();
    library.loupe.wait(&ctx);
    // A 1200 px wide view asks for the next step up, 1536 px; never more.
    assert_eq!(library.loupe.state, loupe::State::Ready);
    assert_eq!(library.loupe.texture_size(), Some([1536, 1024]));
    // The next photo replaces it; a damaged file says why.
    library.step(selection::Step::By(1), false);
    let mut output = ctx.run_ui(input(), |ui| {
        library.grid(ui, &mut Default::default());
    });
    output.textures_delta.clear();
    library.loupe.wait(&ctx);
    assert!(matches!(library.loupe.state, loupe::State::Failed(_)));
    library.close_loupe();
    assert!(!library.loupe_open());
    Ok(())
}
#[test]
fn flag_steps_up_and_down_and_stops_at_the_ends() {
    use crate::app::photo_metadata::Edit;
    let photo = |flag| Photo {
        id: 1,
        folder: 1,
        path: "a.RAF".into(),
        filename: "a.RAF".into(),
        captured: String::new(),
        rating: 0,
        flag,
        label: String::new(),
        format: "RAF".into(),
        copy_name: String::new(),
        master: None,
        keywords: String::new(),
        has_lightroom_edits: false,
    };
    assert_eq!(Edit::FlagDelta(1).values(&photo(-1)).1, 0);
    assert_eq!(Edit::FlagDelta(1).values(&photo(0)).1, 1);
    assert_eq!(Edit::FlagDelta(1).values(&photo(1)).1, 1);
    assert_eq!(Edit::FlagDelta(-1).values(&photo(-1)).1, -1);
}
#[test]
fn loupe_zooms_at_the_navigator_levels_and_prepares_the_next_photo() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in ["a.jpg", "b.jpg"] {
        image::RgbImage::from_fn(3000, 2000, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
        })
        .save(folder.join(name))?;
    }
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let ctx = egui::Context::default();
    let mut library = Library::load(&path, ctx.clone())?;
    library.wait_for_availability();
    library.select(Some(library.photos[0].id));
    library.open_loupe();
    let mut zoom = crate::app::navigator::Zoom::default();
    let frame = |library: &mut Library, zoom: &mut crate::app::navigator::Zoom| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200., 800.),
                )),
                ..Default::default()
            },
            |ui| {
                library.grid(ui, zoom);
            },
        );
        output.textures_delta.clear();
    };
    frame(&mut library, &mut zoom);
    library.loupe.wait(&ctx);
    // The next photo is prepared once this one is shown, and shown at once.
    frame(&mut library, &mut zoom);
    library.loupe.wait_ahead(&ctx);
    library.step(selection::Step::By(1), false);
    frame(&mut library, &mut zoom);
    assert_eq!(library.loupe.state, loupe::State::Ready);
    // 100% reads only the view: 1200 by 672 pixels of the 3000 by 2000.
    zoom.set(1.);
    frame(&mut library, &mut zoom);
    library.loupe.regions.wait(&ctx);
    assert_eq!(library.loupe.regions.full, Some([3000, 2000]));
    let (region, rect) = library.loupe.regions.region.clone().unwrap();
    assert_eq!(region.size(), [1200, 672]);
    assert!((rect[0] - 0.3).abs() < 1e-3 && (rect[2] - 0.4).abs() < 1e-3);
    // At 200% half as many image pixels fill the view.
    zoom.set(2.);
    frame(&mut library, &mut zoom);
    library.loupe.regions.wait(&ctx);
    let (region, _) = library.loupe.regions.region.clone().unwrap();
    assert_eq!(region.size(), [600, 336]);
    // Panning past the corner stops at the edge of the photo.
    zoom.pan = [0., 0.];
    frame(&mut library, &mut zoom);
    library.loupe.regions.wait(&ctx);
    frame(&mut library, &mut zoom);
    let (_, rect) = library.loupe.regions.region.clone().unwrap();
    assert_eq!([rect[0], rect[1]], [0., 0.]);
    zoom.set(0.);
    frame(&mut library, &mut zoom);
    assert!(library.loupe.regions.region.is_none());
    Ok(())
}
#[test]
fn photo_info_of_folder_photos_is_read_once_and_kept() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let folder = directory.path().join("photos");
    std::fs::create_dir(&folder)?;
    image::RgbImage::new(300, 200).save(folder.join("a.jpg"))?;
    let path = directory.path().join("library.rawmakase");
    Catalog::create(&path)?.add_folder(&folder)?;
    let mut library = Library::load(&path, egui::Context::default())?;
    let id = library.photos[0].id;
    library.wait_for_availability();
    let started = std::time::Instant::now();
    while library.info_reader.is_some() {
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        library.poll_photo_info();
        std::thread::yield_now();
    }
    library.select(Some(id));
    let info = library.active_info().unwrap();
    assert_eq!(info.dimensions_text().as_deref(), Some("300 × 200"));
    // Kept: nothing is left to read on the next open.
    assert!(library.catalog.photos_without_info()?.is_empty());
    Ok(())
}
