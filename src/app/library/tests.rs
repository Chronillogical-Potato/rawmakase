use super::*;
#[test]
fn develop_workspace_drains_library_preview_results() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("previews.rawmakase");
    drop(Catalog::create(&path)?);
    let ctx = egui::Context::default();
    let mut library = Library::load(&path, ctx.clone())?;
    let (tx, rx) = std::sync::mpsc::sync_channel(24);
    library.thumb_rx = rx;
    for index in 0..24 {
        let path = directory.path().join(format!("{index}.ARW"));
        library.pending.insert(path.clone());
        library.preview_progress.queued();
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
    assert!(library.pending.is_empty());
    assert_eq!(library.thumbs.len(), 24);
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
                photo_cell(ui, &photo, Some(&texture), false, 1, true, width);
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
    library.selected = Some(ids[0]);
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
    library.flag = 0;
    library.filter();
    library.selected = Some(ids[1]);
    assert_eq!(
        library.edit_metadata(ids[1], Edit::Flag(-1), true)?,
        Some(ids[2])
    );
    assert_eq!(library.selected, Some(ids[2]));
    assert_eq!(library.visible.len(), 1);
    assert_eq!(library.edit_metadata(ids[2], Edit::Flag(1), true)?, None);
    assert_eq!(library.selected, None);
    assert!(library.visible.is_empty());
    library.flag = 2;
    library.label_filter = Some("Purple".into());
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
        available_paths(&rows),
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
    let l = Library::load(&db, ctx)?;
    assert_eq!(l.photos[0].path, new.join("image.ARW"));
    assert!(l.available.contains(&l.photos[0].path));
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
    assert!(super::spawn_volume_check(
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
        assert!(!super::spawn_volume_check(
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
    assert!(super::spawn_volume_check(
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
    library.edit_rx = rx;
    let copy = library.create_virtual_copy(master)?;
    library.edited_requested.insert(copy, 7);
    library.edit_seen.insert(copy);
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
    assert!(!library.edited_requested.contains_key(&copy));
    assert!(!library.edited_order.contains(&copy));
    assert!(!library.edit_seen.contains(&copy));
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
    library.copy_name = Some((copy, " B&W ".into()));
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
