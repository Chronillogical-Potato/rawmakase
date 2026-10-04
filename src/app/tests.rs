use super::widgets::tone_curve_ui;
use super::*;
use crate::develop;
use crate::raw::{CameraImage, Metadata};
use eframe::egui::{Pos2, Rect};
use std::sync::Arc;
#[test]
fn autosave_writes_the_catalog_in_the_background() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"identity fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    let mut c = crate::catalog::Catalog::create(&catalog)?;
    c.add_folder(&photos)?;
    drop(c);
    let ctx = egui::Context::default();
    let l = crate::app::library::Library::load(&catalog, ctx.clone())?;
    let id = l.photos[0].id;
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.library = Some(Box::new(l));
    editor.document.catalog_photo = Some(id);
    editor.document.path = Some(photo.clone());
    let settled = || {
        save_state::SaveState::Pending(
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        )
    };
    editor.document.recipe.exposure = 0.8;
    editor.document.save = settled();
    editor.autosave(&ctx);
    assert!(editor.autosave.busy());
    // Edited again while that save runs: still unsaved once it finishes.
    editor.document.recipe.exposure = 1.1;
    editor.document.save.mark_changed();
    // On a slow machine the save can outlast the settle delay; keep the loop
    // below from starting the next save before this one is checked.
    if let save_state::SaveState::Saving { changed: Some(at) } = &mut editor.document.save {
        *at += std::time::Duration::from_secs(3600);
    }
    while editor.autosave.busy() {
        std::thread::sleep(std::time::Duration::from_millis(5));
        editor.autosave(&ctx);
    }
    assert!(matches!(
        editor.document.save,
        save_state::SaveState::Pending(_)
    ));
    let saved = |editor: &Editor| -> anyhow::Result<f32> {
        let library = editor.library.as_ref().unwrap();
        Ok(library
            .catalog
            .load_edit(id, &photo)?
            .unwrap()
            .recipe
            .exposure)
    };
    assert_eq!(saved(&editor)?, 0.8);
    // A save before navigation waits for the one in flight, then saves.
    editor.document.save = settled();
    editor.autosave(&ctx);
    editor.document.recipe.exposure = 1.4;
    editor.document.save.mark_changed();
    assert!(editor.flush());
    assert!(!editor.autosave.busy());
    assert!(!editor.document.save.needs_save());
    assert_eq!(saved(&editor)?, 1.4);
    Ok(())
}
#[test]
fn catalog_edits_save_to_database_and_library_renders() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"identity fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    let mut c = crate::catalog::Catalog::create(&catalog)?;
    c.add_folder(&photos)?;
    drop(c);
    let ctx = egui::Context::default();
    let l = crate::app::library::Library::load(&catalog, ctx.clone())?;
    let id = l.photos[0].id;
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.library = Some(Box::new(l));
    editor.document.catalog_photo = Some(id);
    editor.document.path = Some(photo.clone());
    editor.document.recipe.exposure = 1.2;
    editor.document.save.mark_changed();
    assert!(editor.flush());
    assert!(!crate::storage::sidecar_path(&photo).exists());
    assert_eq!(
        editor
            .library
            .as_ref()
            .unwrap()
            .catalog
            .load_edit(id, &photo)?
            .unwrap()
            .recipe
            .exposure,
        1.2
    );
    editor.library_mode = true;
    for _ in 0..2 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                ..Default::default()
            },
            |ui| editor.draw(ui),
        );
        output.textures_delta.clear();
    }
    assert!(editor.library_mode);
    Ok(())
}
#[test]
fn curve_pointer_add_drag_and_remove() {
    let ctx = egui::Context::default();
    let mut curve = crate::develop::curve::ToneCurve::default();
    let mut frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(320.))),
                events,
                ..Default::default()
            },
            |ui| tone_curve_ui(ui, &mut curve, &[[0; 256]; 3], 0),
        );
        output.textures_delta.clear();
        curve.points.clone()
    };
    let event = |p: Pos2, button, pressed| egui::Event::PointerButton {
        pos: p,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![]);
    let p = Pos2::new(150., 160.);
    frame(vec![
        egui::Event::PointerMoved(p),
        event(p, egui::PointerButton::Primary, true),
    ]);
    assert_eq!(
        frame(vec![event(p, egui::PointerButton::Primary, false)]).len(),
        3
    );
    frame(vec![event(p, egui::PointerButton::Primary, true)]);
    let q = Pos2::new(200., 110.);
    let moved = frame(vec![egui::Event::PointerMoved(q)]);
    assert!(moved[1][0] > 0.6 && moved[1][1] > 0.6);
    frame(vec![event(q, egui::PointerButton::Primary, false)]);
    frame(vec![event(q, egui::PointerButton::Secondary, true)]);
    assert_eq!(
        frame(vec![event(q, egui::PointerButton::Secondary, false)]).len(),
        2
    );
}
#[test]
fn catalog_metadata_keys_work_in_both_modules_without_zoom_or_dialog_edits() -> anyhow::Result<()> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("a.RAF"), b"fixture")?;
    std::fs::write(photos.join("b.RAF"), b"fixture")?;
    let path = d.path().join("test.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&path)?;
    catalog.add_folder(&photos)?;
    drop(catalog);
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let mut library = crate::app::library::Library::load(&path, ctx.clone())?;
    let ids: Vec<_> = library.photos.iter().map(|p| p.id).collect();
    library.select(Some(ids[0]));
    e.library = Some(Box::new(library));
    for (library_mode, key, expected_rating, expected_flag) in [
        (true, egui::Key::Num5, 5, 0),
        (true, egui::Key::P, 5, 1),
        (false, egui::Key::Num1, 1, 0),
        (false, egui::Key::X, 1, -1),
        (false, egui::Key::U, 1, 0),
        (false, egui::Key::Num0, 0, 0),
    ] {
        e.library_mode = library_mode;
        e.document.catalog_photo = Some(ids[1]);
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                events: vec![egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
        let id = ids[usize::from(!library_mode)];
        let photo = e.library.as_ref().unwrap().photo(id).unwrap();
        assert_eq!((photo.rating, photo.flag), (expected_rating, expected_flag));
        assert!(!e.view.zoom.on);
    }
    assert!(e.activity.begin_dialog());
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Num5,
                physical_key: Some(egui::Key::Num5),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| e.draw(ui),
    );
    output.textures_delta.clear();
    assert_eq!(e.library.as_ref().unwrap().photo(ids[1]).unwrap().rating, 0);
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().rating, 5);
    let reopened = crate::catalog::Catalog::open(&path)?;
    assert_eq!(
        reopened
            .photos()?
            .iter()
            .find(|p| p.id == ids[0])
            .unwrap()
            .flag,
        1
    );
    Ok(())
}
#[test]
fn keyboard_fit_and_physical_pixel_region() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    for (key, expected) in [(egui::Key::Z, true), (egui::Key::F, false)] {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events: vec![egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| e.draw(ui));
        output.textures_delta.clear();
        assert_eq!(e.view.zoom.on, expected);
    }
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 12,
        height: 8,
        pixels: vec![[0.1; 3]; 96],
        metadata: Metadata {
            width: 12,
            height: 8,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    e.document.set_image(image);
    e.view.zoom.on = true;
    e.view.viewport = Vec2::new(4., 2.);
    assert_eq!(e.region(), Some([4, 3, 4, 2]));
}
#[test]
fn photo_click_zooms_and_drag_pans_without_editing() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: vec![[0.1; 3]; 160000],
        metadata: Metadata {
            width: 400,
            height: 400,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    let recipe = editor.document.recipe.clone();
    let mut frame = |events| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| editor.viewport_ui(ui),
        );
        output.textures_delta.clear();
        (
            editor.view.zoom.on,
            editor.view.zoom.pan,
            editor.document.recipe.clone(),
        )
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let p = Pos2::new(100., 100.);
    frame(vec![]);
    frame(vec![egui::Event::PointerMoved(p), button(p, true)]);
    let (zoom, original_pan, _) = frame(vec![button(p, false)]);
    assert!(zoom);
    frame(vec![button(p, true)]);
    let q = Pos2::new(140., 130.);
    let (zoom, pan, after) = frame(vec![egui::Event::PointerMoved(q)]);
    assert!(zoom);
    assert!(pan[0] < original_pan[0] && pan[1] < original_pan[1]);
    assert_eq!(recipe, after);
    let (zoom, _, _) = frame(vec![button(q, false)]);
    assert!(zoom, "Releasing a pan must not toggle zoom");
    frame(vec![button(q, true)]);
    assert!(!frame(vec![button(q, false)]).0);
}

#[test]
fn compact_inspector_keeps_canvas_and_before_preserves_edits() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.document.recipe.exposure = 1.25;
    editor.document.recipe.crop = [0.1, 0.1, 0.9, 0.9];
    let saved = editor.document.recipe.clone();
    for frame in 0..30 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                events: if frame == 28 {
                    vec![egui::Event::Key {
                        key: egui::Key::Backslash,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    }]
                } else {
                    vec![]
                },
                ..Default::default()
            },
            |ui| editor.draw(ui),
        );
        output.textures_delta.clear();
        assert!(
            editor.view.viewport.x > 400.,
            "Inspector consumed canvas on frame {frame}"
        );
    }
    assert!(editor.view.compare);
    assert_eq!(editor.document.recipe, saved);
    assert_eq!(editor.effective_recipe().crop, saved.crop);
    assert_eq!(editor.effective_recipe().exposure, 0.);
}

#[test]
fn history_snapshot_undo_and_redo() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let original = e.document.recipe.clone();
    e.document.recipe.exposure = 2.;
    e.history(original.clone());
    assert!(e.document.history.can_undo());
    e.undo();
    assert_eq!(e.document.recipe, original);
    assert!(e.document.history.can_redo());
    e.redo();
    assert_eq!(e.document.recipe.exposure, 2.);
    assert!(e.document.history.can_undo());
}
#[test]
fn undo_and_redo_keys_work_while_a_button_has_focus() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let original = e.document.recipe.clone();
    e.document.recipe.exposure = 2.;
    e.history(original.clone());
    // A clicked button or the tone curve keeps focus; that must not block shortcuts.
    let frame = |input, e: &mut Editor| {
        let mut output = ctx.run_ui(input, |ui| {
            ui.button("focused").request_focus();
            assert!(ctx.egui_wants_keyboard_input());
            e.develop_shortcuts(&ctx);
        });
        output.textures_delta.clear();
    };
    let press = |shift: bool| {
        let modifiers = egui::Modifiers {
            command: true,
            mac_cmd: cfg!(target_os = "macos"),
            ctrl: !cfg!(target_os = "macos"),
            shift,
            ..Default::default()
        };
        egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key: egui::Key::Z,
                    physical_key: Some(egui::Key::Z),
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        }
    };
    frame(egui::RawInput::default(), &mut e);
    frame(press(false), &mut e);
    assert_eq!(e.document.recipe, original);
    frame(press(true), &mut e);
    assert_eq!(e.document.recipe.exposure, 2.);
}
#[test]
fn stale_preview_results_are_discarded() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let (old, _) = e.preview.task.start();
    e.preview.task.start();
    e.tx.send(Event::Rendered {
        id: old,
        preview: worker::Preview::Pixels {
            image: develop::Rendered {
                width: 1,
                height: 1,
                pixels: vec![[1.; 3]],
            },
            display_rgb: vec![255; 3],
            navigator: None,
        },
        histogram: Box::new(develop::Histogram::EMPTY),
        thumbnail: None,
        samples: None,
        stage: worker::RenderStage::Fit,
        status: "stale".into(),
    })
    .unwrap();
    e.events(&ctx);
    assert!(e.preview.texture.is_none());
}

#[test]
fn worker_failures_are_scoped_and_render_stages_do_not_depend_on_status_text() {
    use worker::{RenderStage, TaskKind};
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let (load_id, _) = editor.load.start();
    let (render_id, _) = editor.preview.task.start();
    editor
        .tx
        .send(Event::Failed {
            id: render_id,
            task: TaskKind::Render,
            error: "render failed".into(),
        })
        .unwrap();
    editor.events(&ctx);
    assert!(editor.load.is_running());
    assert!(!editor.preview.task.is_running());

    let (current, _) = editor.preview.task.start();
    editor
        .tx
        .send(Event::Failed {
            id: render_id,
            task: TaskKind::Render,
            error: "stale failure".into(),
        })
        .unwrap();
    editor
        .tx
        .send(Event::Failed {
            id: load_id,
            task: TaskKind::Load,
            error: "load failed".into(),
        })
        .unwrap();
    editor.events(&ctx);
    assert!(!editor.load.is_running());
    assert!(editor.preview.task.is_running());
    for (stage, status, running) in [
        (RenderStage::Draft, "localized preview text", true),
        (RenderStage::Fit, "Draft is just text here", false),
    ] {
        editor
            .tx
            .send(Event::Rendered {
                id: current,
                preview: worker::Preview::Pixels {
                    image: develop::Rendered {
                        width: 1,
                        height: 1,
                        pixels: vec![[0.5; 3]],
                    },
                    display_rgb: vec![128; 3],
                    navigator: None,
                },
                histogram: Box::new(develop::Histogram::EMPTY),
                thumbnail: None,
                samples: None,
                stage,
                status: status.into(),
            })
            .unwrap();
        editor.events(&ctx);
        assert_eq!(editor.preview.task.is_running(), running);
    }
}

#[test]
fn catalog_header_keeps_the_recipe_resolved_by_the_loader() -> anyhow::Result<()> {
    use worker::LoadedHeader;
    let dir = tempfile::tempdir()?;
    let raw = dir.path().join("photo.ARW");
    std::fs::write(&raw, b"identity fixture")?;
    let path = dir.path().join("photos.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&path)?;
    catalog.add_folder(dir.path())?;
    let id = catalog.photos()?[0].id;
    drop(catalog);
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    editor.document.catalog_photo = Some(id);
    let (generation, _) = editor.load.start();
    let recipe = Recipe {
        exposure: 0.75,
        ..Default::default()
    };
    editor
        .tx
        .send(Event::Header(Box::new(LoadedHeader {
            id: generation,
            path: raw,
            metadata: Metadata::default(),
            recipe: recipe.clone(),
            export: Default::default(),
            protected: false,
            status: "Original".into(),
        })))
        .unwrap();
    editor.events(&ctx);
    assert_eq!(editor.document.recipe, recipe);
    Ok(())
}

#[test]
fn develop_history_survives_reopening_the_photo() -> anyhow::Result<()> {
    use worker::LoadedHeader;
    let dir = tempfile::tempdir()?;
    let raw = dir.path().join("photo.ARW");
    std::fs::write(&raw, b"identity fixture")?;
    let path = dir.path().join("photos.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&path)?;
    catalog.add_folder(dir.path())?;
    let id = catalog.photos()?[0].id;
    drop(catalog);
    let ctx = egui::Context::default();
    let open = |editor: &mut Editor| {
        editor.document.reset(Some(id));
        let (generation, _) = editor.load.start();
        editor
            .tx
            .send(Event::Header(Box::new(LoadedHeader {
                id: generation,
                path: raw.clone(),
                metadata: Metadata::default(),
                recipe: Recipe::default(),
                export: Default::default(),
                protected: false,
                status: "Original".into(),
            })))
            .unwrap();
        editor.events(&ctx);
    };
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    open(&mut editor);
    for exposure in [0.5, 1.] {
        let before = editor.document.recipe.clone();
        editor.document.recipe.exposure = exposure;
        editor.history(before);
    }
    assert!(editor.flush());
    // Another photo, or a restart, starts a new document.
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    open(&mut editor);
    assert_eq!(editor.document.recipe.exposure, 1.);
    assert_eq!(editor.document.history.steps().0.len(), 2);
    let mut recipe = editor.document.recipe.clone();
    assert!(editor.document.history.undo(&mut recipe));
    assert_eq!(recipe.exposure, 0.5);
    Ok(())
}
#[test]
fn navigation_during_an_edit_frame_cannot_dirty_the_next_document() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.document.recipe.exposure = 1.25;
    editor.presets.preview = Some(editor.document.recipe.clone());
    editor.view.crop_drag = Some(([0., 0., 1., 1.], 0));
    let frame = editor.begin_edit_frame();
    // This does not need a valid RAW: navigation resets state before asynchronous decoding.
    editor.open_raw(
        std::path::PathBuf::from("missing-navigation-fixture.ARW"),
        None,
    );
    editor.finish_edit_frame(frame, &ctx);
    assert!(!editor.document.save.needs_save());
    assert!(!editor.document.history.can_undo());
    assert!(!editor.document.history.in_gesture());
    assert!(editor.document.full().is_none());
    assert!(editor.presets.preview.is_none());
    assert!(editor.view.crop_drag.is_none());
}

#[test]
fn refreshing_preset_support_cancels_the_hover_render() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 1,
        height: 1,
        pixels: vec![[0.1; 3]],
        metadata: Metadata {
            width: 1,
            height: 1,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.presets.preview = Some(Recipe {
        exposure: 1.,
        ..Default::default()
    });
    let (previous, cancelled) = editor.preview.task.start();
    editor.refresh_preset_support();
    assert!(editor.presets.preview.is_none());
    assert!(cancelled.load(std::sync::atomic::Ordering::Relaxed));
    assert!(editor.preview.task.id() > previous);
}

#[test]
fn session_preferences_use_the_injected_store() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("session.json");
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::storage::Session::default(),
        Some(path.clone()),
    );
    editor.document.path = Some(dir.path().join("photo.ARW"));
    editor
        .tx
        .send(Event::Monitor(dir.path().join("display.icc")))
        .unwrap();
    editor.events(&ctx);
    let saved: crate::storage::Session = serde_json::from_slice(&std::fs::read(path)?)?;
    assert_eq!(saved.last_path, editor.document.path);
    assert_eq!(saved.monitor, editor.view.monitor);
    Ok(())
}
#[test]
fn remove_tool_adds_spots_paints_brushes_and_edits_the_selection() {
    use crate::develop::retouch::RetouchShape;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: (0..160000)
            .map(|i| {
                let (x, y) = ((i % 400) as f32, (i / 400) as f32);
                [0.2 + 0.05 * (x * 0.1).sin() * (y * 0.13).cos(); 3]
            })
            .collect(),
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    editor.view.tool = state::Tool::Remove;
    let mut frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                ctx.input(|i| {
                    if editor.view.is(state::Tool::Remove) {
                        editor.retouch_keys(i)
                    }
                });
                editor.viewport_ui(ui)
            },
        );
        output.textures_delta.clear();
        (
            editor.document.recipe.retouch.clone(),
            editor.view.zoom.on,
            editor.view.retouch.selected,
        )
    };
    let button = |pos, pressed, modifiers| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    };
    let none = egui::Modifiers::NONE;
    frame(vec![]);
    // Click: a spot with an automatic source, and no zoom.
    let p = Pos2::new(60., 60.);
    frame(vec![egui::Event::PointerMoved(p), button(p, true, none)]);
    let (ops, zoomed, _) = frame(vec![button(p, false, none)]);
    assert_eq!(ops.len(), 1);
    assert!(!zoomed);
    assert!(ops[0].offset != [0., 0.] && ops[0].offset.iter().all(|v| v.is_finite()));
    assert!(matches!(ops[0].shape, RetouchShape::Spot { center, .. }
        if (center[0] - 0.3).abs() < 0.02 && (center[1] - 0.3).abs() < 0.02));
    // A drag from empty space paints a brushed area.
    let (a, b) = (Pos2::new(150., 40.), Pos2::new(150., 150.));
    frame(vec![egui::Event::PointerMoved(a), button(a, true, none)]);
    for k in 1..=10 {
        frame(vec![egui::Event::PointerMoved(
            a + (b - a) * k as f32 / 10.,
        )]);
    }
    let (ops, _, selected) = frame(vec![button(b, false, none)]);
    assert_eq!(ops.len(), 2);
    assert!(matches!(&ops[1].shape, RetouchShape::Brush { points, .. } if points.len() > 3));
    assert_eq!(selected, Some(1));
    // ] grows the selected area; Delete removes it.
    let radius = ops[1].radius();
    let key = |key| egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let (ops, ..) = frame(vec![key(egui::Key::CloseBracket)]);
    assert!(ops[1].radius() > radius);
    let (ops, ..) = frame(vec![key(egui::Key::Delete)]);
    assert_eq!(ops.len(), 1);
    // Dragging the remaining spot moves it and keeps its source in place.
    let before = ops[0].clone();
    let q = Pos2::new(80., 70.);
    frame(vec![egui::Event::PointerMoved(p), button(p, true, none)]);
    frame(vec![egui::Event::PointerMoved(q)]);
    let (ops, ..) = frame(vec![button(q, false, none)]);
    let source = |op: &crate::develop::retouch::RetouchOp| {
        [op.pin()[0] + op.offset[0], op.pin()[1] + op.offset[1]]
    };
    assert!((ops[0].pin()[0] - before.pin()[0] - 0.1).abs() < 0.01);
    assert!((source(&ops[0])[0] - source(&before)[0]).abs() < 1e-5);
}
#[test]
fn red_eye_tool_adds_moves_and_deletes_one_history_step_each() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let eyes = [([100., 100.], 10.), ([300., 260.], 8.)];
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: (0..160000)
            .map(|i| {
                let (x, y) = ((i % 400) as f32, (i / 400) as f32);
                let red = eyes.iter().any(|(c, r)| (x - c[0]).hypot(y - c[1]) <= *r);
                if red {
                    [0.6, 0.03, 0.03]
                } else {
                    [0.55, 0.35, 0.25]
                }
            })
            .collect(),
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    editor.view.tool = state::Tool::RedEye;
    let mut frame = |events: Vec<egui::Event>| {
        let edit = editor.begin_edit_frame();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                ctx.input(|i| editor.red_eye_keys(i));
                editor.viewport_ui(ui)
            },
        );
        editor.finish_edit_frame(edit, &ctx);
        output.textures_delta.clear();
        let (steps, applied) = editor.document.history.steps();
        let names: Vec<String> = steps[..applied].iter().map(|s| s.name.clone()).collect();
        (
            editor.document.recipe.red_eye.clone(),
            names,
            editor.status.to_string(),
        )
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![]);
    // Dragging from the first eye's centre outward finds its pupil: one step.
    let (a, b) = (Pos2::new(50., 50.), Pos2::new(62., 50.));
    frame(vec![egui::Event::PointerMoved(a), button(a, true)]);
    for k in 1..=6 {
        frame(vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 6.)]);
    }
    let (ops, names, _) = frame(vec![button(b, false)]);
    assert_eq!(ops.len(), 1);
    assert!((ops[0].center[0] - 0.25).abs() < 0.005 && (ops[0].center[1] - 0.25).abs() < 0.005);
    assert!(
        (ops[0].radius[0] * 400. - 10.).abs() < 1.5,
        "{:?}",
        ops[0].radius
    );
    assert_eq!(names, ["Add Red Eye Correction"]);
    // A click on the second eye uses the last size.
    let c = Pos2::new(150., 130.);
    frame(vec![egui::Event::PointerMoved(c), button(c, true)]);
    let (ops, names, _) = frame(vec![button(c, false)]);
    assert_eq!(ops.len(), 2);
    assert!((ops[1].center[0] - 0.75).abs() < 0.005 && (ops[1].center[1] - 0.65).abs() < 0.005);
    assert_eq!(names.len(), 2);
    // A click on nothing red adds nothing and says why.
    let d = Pos2::new(180., 20.);
    frame(vec![egui::Event::PointerMoved(d), button(d, true)]);
    let (ops, names, status) = frame(vec![button(d, false)]);
    assert_eq!((ops.len(), names.len()), (2, 2));
    assert!(status.contains("Unable to find red eye"), "{status}");
    // Dragging the first correction moves it in one step.
    let e = Pos2::new(70., 60.);
    frame(vec![egui::Event::PointerMoved(a), button(a, true)]);
    for k in 1..=5 {
        frame(vec![egui::Event::PointerMoved(a + (e - a) * k as f32 / 5.)]);
    }
    let (ops, names, _) = frame(vec![button(e, false)]);
    assert!((ops[0].center[0] - 0.35).abs() < 0.005);
    assert_eq!(names.last().unwrap(), "Update Red Eye Correction");
    assert_eq!(names.len(), 3);
    // Delete removes the selected correction.
    let key = egui::Event::Key {
        key: egui::Key::Delete,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let (ops, names, _) = frame(vec![key]);
    assert_eq!(ops.len(), 1);
    assert_eq!(names.last().unwrap(), "Delete Red Eye Correction");
    // Undo brings it back where it was.
    let mut recipe = editor.document.recipe.clone();
    assert!(editor.document.history.undo(&mut recipe));
    assert_eq!(recipe.red_eye.len(), 2);
    assert!((recipe.red_eye[0].center[0] - 0.35).abs() < 0.005);
}
#[test]
fn a_new_red_eye_correction_turns_the_red_eye_switch_on() {
    use crate::develop::panels::{Panel, PanelState};
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 200,
        height: 200,
        pixels: (0..40000)
            .map(|i| {
                let (x, y) = ((i % 200) as f32, (i / 200) as f32);
                if (x - 100.).hypot(y - 100.) <= 8. {
                    [0.6, 0.03, 0.03]
                } else {
                    [0.55, 0.35, 0.25]
                }
            })
            .collect(),
        metadata: Metadata {
            width: 200,
            height: 200,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    let panels = &mut editor.document.recipe.panels;
    panels.set(Panel::RedEye, PanelState::Off);
    editor.add_red_eye([0.5, 0.5], 0.1);
    assert_eq!(editor.document.recipe.red_eye.len(), 1);
    assert_eq!(
        editor.document.recipe.panels.state(Panel::RedEye),
        PanelState::On
    );
}
#[test]
fn pet_eye_type_finds_a_glowing_pupil_and_adds_a_catchlight() {
    use crate::develop::red_eye::{DEFAULT_CATCHLIGHT, EyeKind};
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 200,
        height: 200,
        pixels: (0..40000)
            .map(|i| {
                let d = ((i % 200) as f32 - 100.).hypot((i / 200) as f32 - 100.);
                if d <= 10. {
                    [0.5, 0.8, 0.15]
                } else if d <= 24. {
                    [0.25, 0.18, 0.05]
                } else {
                    [0.3, 0.25, 0.2]
                }
            })
            .collect(),
        metadata: Metadata {
            width: 200,
            height: 200,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    // Red Eye finds nothing red there.
    editor.add_red_eye([0.5, 0.5], 0.15);
    assert!(editor.document.recipe.red_eye.is_empty());
    assert!(editor.status.contains("Unable to find red eye"));
    editor.view.red_eye.pet = red_eye_tool::PupilType::Pet;
    editor.add_red_eye([0.5, 0.5], 0.15);
    let eyes = &editor.document.recipe.red_eye;
    assert_eq!(eyes.len(), 1);
    assert_eq!(
        eyes[0].kind,
        EyeKind::Pet {
            catchlight: Some(DEFAULT_CATCHLIGHT)
        }
    );
    assert!((eyes[0].radius[0] * 200. - 10.).abs() < 1.5);
    // Selecting a correction makes its type the one new corrections get.
    editor.view.red_eye.pet = red_eye_tool::PupilType::Red;
    editor.select_red_eye(Some(0));
    assert_eq!(editor.view.red_eye.pet, red_eye_tool::PupilType::Pet);
    // Even when the remembered type is stale (after an undo, say), a new correction
    // takes the selected one's type, as the Type menu shows.
    editor.view.red_eye.pet = red_eye_tool::PupilType::Red;
    editor.add_red_eye([0.5, 0.5], 0.15);
    assert_eq!(editor.document.recipe.red_eye.len(), 2);
    editor.document.recipe.red_eye.pop();
    editor.select_red_eye(Some(0));
    // A click on the catchlight's handle, even at the pupil's edge outside the
    // ellipse, is not a new search.
    editor.document.recipe.red_eye[0].kind = EyeKind::Pet {
        catchlight: Some([1., 0.]),
    };
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    editor.view.tool = state::Tool::RedEye;
    let handle = editor.document.recipe.red_eye[0].catchlight_at(1.).unwrap();
    let mut frame = |events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| editor.viewport_ui(ui),
        );
        output.textures_delta.clear();
        editor.document.recipe.red_eye.len()
    };
    frame(vec![]);
    let p = Pos2::new(handle[0] * 200., handle[1] * 200.);
    let button = |pressed| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    frame(vec![egui::Event::PointerMoved(p), button(true)]);
    assert_eq!(frame(vec![button(false)]), 1);
}
#[test]
fn red_eye_tool_refuses_a_red_area_too_large_to_be_a_pupil() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: (0..160000)
            .map(|i| {
                let (x, y) = ((i % 400) as f32, (i / 400) as f32);
                if (x - 200.).hypot(y - 200.) <= 150. {
                    [0.6, 0.03, 0.03]
                } else {
                    [0.55, 0.35, 0.25]
                }
            })
            .collect(),
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.add_red_eye([0.5, 0.5], 0.48);
    assert!(editor.document.recipe.red_eye.is_empty());
    assert!(
        editor.status.contains("Unable to find red eye"),
        "{}",
        editor.status
    );
    editor.document.recipe.validate().unwrap();
}
#[test]
fn masking_tool_draws_gradients_paints_brushes_and_edits_handles() {
    use crate::develop::masks::MaskShape;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 400,
        height: 400,
        pixels: vec![[0.2; 3]; 160000],
        metadata: Metadata {
            width: 400,
            height: 400,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    editor.preview.texture = Some(
        ctx.load_texture(
            "photo",
            egui::ColorImage::filled([200, 200], egui::Color32::GRAY),
            egui::TextureOptions::LINEAR,
        )
        .into(),
    );
    let mut frame = |editor: &mut Editor, events: Vec<egui::Event>| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.))),
                events,
                ..Default::default()
            },
            |ui| {
                ctx.input(|i| {
                    if editor.view.is(state::Tool::Mask) {
                        editor.mask_keys(i)
                    }
                });
                editor.viewport_ui(ui)
            },
        );
        output.textures_delta.clear();
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let drag = |editor: &mut Editor,
                frame: &mut dyn FnMut(&mut Editor, Vec<egui::Event>),
                a: Pos2,
                b: Pos2| {
        frame(editor, vec![egui::Event::PointerMoved(a), button(a, true)]);
        for k in 1..=8 {
            frame(
                editor,
                vec![egui::Event::PointerMoved(a + (b - a) * k as f32 / 8.)],
            );
        }
        frame(editor, vec![button(b, false)]);
    };
    frame(&mut editor, vec![]);
    // M, then a drag, draws a linear gradient as a new mask.
    editor.create_mask(mask_tool::Kind::Linear, None);
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(100., 40.),
        Pos2::new(100., 160.),
    );
    let masks = editor.document.recipe.masks.clone();
    assert_eq!(masks.len(), 1);
    let MaskShape::Linear { from, to } = masks[0].components[0].shape else {
        panic!("{:?}", masks[0].components[0].shape);
    };
    assert!(
        (from[1] - 0.2).abs() < 0.02 && (to[1] - 0.8).abs() < 0.02,
        "{from:?} {to:?}"
    );
    assert!(!editor.view.zoom.on);
    // Dragging its end handle moves only that end.
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(100., 160.),
        Pos2::new(140., 170.),
    );
    let MaskShape::Linear { from: f2, to: t2 } =
        editor.document.recipe.masks[0].components[0].shape
    else {
        panic!()
    };
    assert_eq!(f2, from);
    assert!(
        (t2[0] - 0.7).abs() < 0.02 && (t2[1] - 0.85).abs() < 0.02,
        "{t2:?}"
    );
    // Shift+M makes a radial gradient; K a brush that paints.
    editor.create_mask(mask_tool::Kind::Radial, None);
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(60., 60.),
        Pos2::new(90., 80.),
    );
    assert!(matches!(
        editor.document.recipe.masks[1].components[0].shape,
        MaskShape::Radial { radii, .. } if (radii[0] - 0.15).abs() < 0.02 && (radii[1] - 0.1).abs() < 0.02
    ));
    editor.create_mask(mask_tool::Kind::Brush, None);
    drag(
        &mut editor,
        &mut frame,
        Pos2::new(20., 180.),
        Pos2::new(180., 180.),
    );
    let MaskShape::Brush { strokes } = &editor.document.recipe.masks[2].components[0].shape else {
        panic!()
    };
    assert_eq!(strokes.len(), 1);
    assert!(strokes[0].points.len() > 3);
    // Add a subtracted brush to the brush mask, then delete the mask with Delete.
    editor.create_mask(
        mask_tool::Kind::Brush,
        Some(crate::develop::masks::MaskOp::Subtract),
    );
    assert_eq!(editor.document.recipe.masks[2].components.len(), 2);
    frame(
        &mut editor,
        vec![egui::Event::Key {
            key: egui::Key::Delete,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
    assert_eq!(editor.document.recipe.masks.len(), 2);
    editor.document.recipe.validate().unwrap();
    // The whole editor draws the Masking and Remove drawers without disturbing edits.
    editor.view.masking.selected = Some(0);
    editor.view.masking.component = Some(0);
    let saved = editor.document.recipe.clone();
    for tool in [state::Tool::Mask, state::Tool::Remove, state::Tool::Crop] {
        editor.view.tool = tool;
        for _ in 0..3 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 900.))),
                    ..Default::default()
                },
                |ui| editor.draw(ui),
            );
            output.textures_delta.clear();
        }
    }
    assert_eq!(editor.document.recipe, saved);
}

#[test]
fn a_photo_from_outside_the_library_is_added_and_opened() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("photos.rawmakase");
    crate::catalog::Catalog::create(&path)?;
    let outside = dir.path().join("outside");
    std::fs::create_dir(&outside)?;
    let raw = outside.join("photo.ARW");
    std::fs::write(&raw, b"identity fixture")?;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.library = Some(Box::new(library::Library::load(&path, ctx.clone())?));
    // As if dropped on the window.
    editor.open(raw.clone());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while editor.pending_photo.is_some() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
        editor.events(&ctx);
    }
    let raw = raw.canonicalize()?;
    let id = editor
        .library
        .as_ref()
        .and_then(|l| l.photos.iter().find(|p| p.path == raw))
        .map(|p| p.id);
    assert!(id.is_some(), "the photo's folder was added to the catalog");
    assert_eq!(editor.document.catalog_photo, id);
    assert!(!editor.library_mode);
    Ok(())
}

#[test]
fn the_prefetched_neighbour_follows_the_direction_of_travel() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    for name in ["a.ARW", "b.ARW", "c.ARW"] {
        std::fs::write(dir.path().join(name), name)?;
    }
    let path = dir.path().join("photos.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(dir.path())?;
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let library = library::Library::load(&path, ctx.clone())?;
    // The filmstrip order, first to last.
    let mut order = vec![library.photos[0].id];
    while let Some(previous) = library.navigate(order[0], -1).filter(|p| *p != order[0]) {
        order.insert(0, previous);
    }
    while let Some(next) = library
        .navigate(order[order.len() - 1], 1)
        .filter(|n| !order.contains(n))
    {
        order.push(next);
    }
    let paths: Vec<_> = order
        .iter()
        .map(|id| library.photo(*id).unwrap().path.clone())
        .collect();
    let [first, middle, last] = [order[0], order[1], order[2]];
    editor.library = Some(Box::new(library));
    // Opening a photo, or stepping forward: the next one.
    assert_eq!(editor.prefetch_neighbour(middle), Some(paths[2].clone()));
    editor.document.catalog_photo = Some(first);
    assert_eq!(editor.prefetch_neighbour(middle), Some(paths[2].clone()));
    // Stepping back: the previous one.
    editor.document.catalog_photo = Some(last);
    assert_eq!(editor.prefetch_neighbour(middle), Some(paths[0].clone()));
    // Nothing past the end of the filmstrip.
    editor.document.catalog_photo = Some(middle);
    assert_eq!(editor.prefetch_neighbour(last), None);
    Ok(())
}
#[test]
fn auto_is_one_undoable_step_that_keeps_edits_made_while_it_ran() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let (width, height) = (64u32, 48u32);
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width,
        height,
        // A dim, warm gradient: Auto brightens it, and the WB menu's Auto cools it.
        pixels: (0..width * height)
            .map(|i| {
                let v = 0.002 + 0.06 * (i % width) as f32 / width as f32;
                [v * 1.2, v, v * 0.8]
            })
            .collect(),
        metadata: Metadata {
            width,
            height,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    let before = editor.document.recipe.clone();
    editor.start_auto(worker::AutoKind::Settings);
    assert!(editor.document.auto.is_running());
    // A second request while the first runs is ignored.
    editor.start_auto(worker::AutoKind::Settings);
    editor.document.recipe.saturation = 0.25;
    let start = std::time::Instant::now();
    while editor.document.auto.is_running() {
        assert!(start.elapsed().as_secs() < 60, "Auto did not finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
        editor.events(&ctx);
    }
    let auto = editor.document.recipe.clone();
    assert!(auto.exposure > 1., "exposure {}", auto.exposure);
    // Auto sets the tone sliders and Vibrance, as Lightroom's does; white balance is the
    // WB menu's Auto.
    assert!(auto.vibrance > 0., "vibrance {}", auto.vibrance);
    assert_eq!(
        (auto.wb, auto.temperature, auto.tint),
        (before.wb, before.temperature, before.tint)
    );
    assert_eq!(auto.saturation, 0.25);
    let (steps, applied) = editor.document.history.steps();
    assert_eq!(applied, 1);
    assert_eq!(steps[0].name, "Auto Settings");
    editor.undo();
    let mut expected = before;
    expected.saturation = 0.25;
    assert_eq!(editor.document.recipe, expected);
    editor.redo();
    assert_eq!(editor.document.recipe, auto);

    editor.start_auto(worker::AutoKind::WhiteBalance);
    while editor.document.auto.is_running() {
        assert!(start.elapsed().as_secs() < 60, "Auto did not finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
        editor.events(&ctx);
    }
    let r = &editor.document.recipe;
    assert!(r.wb[0] < 1. && r.wb[2] > 1., "wb {:?}", r.wb);
    assert_eq!(r.auto_white_balance, Some([r.temperature, r.tint]));
    assert_eq!(r.exposure, auto.exposure);
    let (steps, _) = editor.document.history.steps();
    assert_eq!(steps[1].name, "White Balance");
    // Pasted onto a photo, the values were not estimated for it: the WB menu says Custom.
    editor.copy_settings(crate::develop::settings_groups::GroupSelection::default());
    editor.paste_settings();
    let r = &editor.document.recipe;
    assert!(r.wb[0] < 1. && r.wb[2] > 1., "wb {:?}", r.wb);
    assert_eq!(r.auto_white_balance, None);
}
#[test]
fn stale_auto_results_are_ignored_after_moving_on() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let before = editor.document.recipe.clone();
    let mut auto = before.clone();
    auto.exposure = 2.;
    editor
        .tx
        .send(worker::Event::Auto {
            id: editor.load.id() + 1,
            kind: worker::AutoKind::Settings,
            result: Ok(Box::new(auto)),
        })
        .unwrap();
    editor.events(&ctx);
    assert_eq!(editor.document.recipe, before);
    assert!(!editor.document.history.can_undo());
}
#[test]
fn auto_shortcut_starts_auto_once_the_photo_is_decoded() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let modifiers = egui::Modifiers {
        command: true,
        mac_cmd: cfg!(target_os = "macos"),
        ctrl: !cfg!(target_os = "macos"),
        shift: true,
        ..Default::default()
    };
    let press = |e: &mut Editor| {
        let input = egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key: egui::Key::U,
                    physical_key: Some(egui::Key::U),
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |_| e.develop_shortcuts(&ctx));
        output.textures_delta.clear();
    };
    press(&mut e);
    assert!(!e.document.auto.is_running(), "no photo yet");
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 8,
        height: 8,
        pixels: vec![[0.1; 3]; 64],
        metadata: Metadata {
            width: 8,
            height: 8,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    press(&mut e);
    assert!(e.document.auto.is_running());
}
#[test]
fn auto_arriving_mid_drag_lands_between_the_two_halves_of_the_drag() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let start = editor.document.recipe.clone();
    // A Shadows drag is under way when the estimate arrives. Auto sets Shadows too,
    // so the drag does not make the estimate stale.
    editor.document.recipe.shadows = 0.1;
    let mid = editor.document.recipe.clone();
    editor.document.history.observe(start.clone(), &mid, true);
    let mut auto = start.clone();
    auto.exposure = 1.;
    editor.auto_ready(worker::AutoKind::Settings, Ok(Box::new(auto)));
    let with_auto = editor.document.recipe.clone();
    assert_eq!(with_auto.exposure, 1.);
    editor.document.recipe.shadows = 0.2;
    let end = editor.document.recipe.clone();
    editor
        .document
        .history
        .observe(with_auto.clone(), &end, false);
    let names: Vec<_> = editor
        .document
        .history
        .steps()
        .0
        .iter()
        .map(|s| s.name.clone())
        .collect();
    assert_eq!(names.len(), 3, "{names:?}");
    assert_eq!(names[1], "Auto Settings");
    // Undoing the rest of the drag keeps Auto; the next undo removes only Auto.
    editor.undo();
    assert_eq!(editor.document.recipe, with_auto);
    editor.undo();
    assert_eq!(editor.document.recipe, mid);
    editor.undo();
    assert_eq!(editor.document.recipe, start);
}
#[test]
fn auto_runs_again_when_the_crop_changed_while_it_ran() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 8,
        height: 8,
        pixels: vec![[0.1; 3]; 64],
        metadata: Metadata {
            width: 8,
            height: 8,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    // An estimate for the uncropped photo arrives after the photo was cropped.
    let mut auto = editor.document.recipe.clone();
    auto.exposure = 1.;
    editor.document.recipe.crop = [0.1, 0.1, 0.9, 0.9];
    editor.auto_ready(worker::AutoKind::Settings, Ok(Box::new(auto)));
    assert_eq!(editor.document.recipe.exposure, 0.);
    assert!(!editor.document.history.can_undo());
    assert!(
        editor.document.auto.is_running(),
        "Auto runs again for the crop"
    );
}
#[test]
fn auto_runs_again_when_it_failed_on_settings_changed_since() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 8,
        height: 8,
        pixels: vec![[0.1; 3]; 64],
        metadata: Metadata {
            width: 8,
            height: 8,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    // An estimate that failed on the uncropped photo arrives after the photo was cropped.
    editor.document.auto_input = Some(editor.document.recipe.clone());
    editor.document.recipe.crop = [0.1, 0.1, 0.9, 0.9];
    editor.auto_ready(
        worker::AutoKind::Settings,
        Err("Auto: no usable pixels".into()),
    );
    assert!(!editor.status.contains("usable"), "{}", editor.status);
    assert!(
        editor.document.auto.is_running(),
        "Auto runs again for the crop"
    );
}
#[test]
fn auto_is_off_while_its_settings_stand() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    assert!(!editor.auto_in_effect());
    let mut auto = editor.document.recipe.clone();
    auto.exposure = 1.;
    auto.vibrance = 0.15;
    editor.auto_ready(worker::AutoKind::Settings, Ok(Box::new(auto)));
    assert_eq!(editor.document.recipe.vibrance, 0.15);
    assert!(editor.auto_in_effect());
    // Any change, to a slider Auto sets or to what it measured, turns it back on, and
    // so does undoing Auto.
    editor.document.recipe.vibrance = 0.;
    assert!(!editor.auto_in_effect());
    editor.document.recipe.vibrance = 0.15;
    assert!(editor.auto_in_effect());
    editor.document.recipe.exposure = 0.5;
    assert!(!editor.auto_in_effect());
    editor.document.recipe.exposure = 1.;
    assert!(editor.auto_in_effect());
    editor.document.recipe.crop[0] = 0.1;
    assert!(!editor.auto_in_effect());
    editor.document.recipe.crop[0] = 0.;
    assert!(editor.auto_in_effect());
    // Adjustments Auto does not measure leave it off.
    editor.document.recipe.effects.clarity = 0.3;
    editor.document.recipe.curve.points[0] = [0., 0.2];
    editor.document.recipe.preset_name = "Curve only".into();
    assert!(editor.auto_in_effect());
    editor.document.recipe.effects.clarity = 0.;
    editor.document.recipe.curve.points[0] = [0., 0.];
    editor.undo();
    assert!(!editor.auto_in_effect());
}
#[test]
fn undoing_an_upright_mode_turns_it_off_once_analysed() {
    use crate::develop::UprightMode;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let original = e.document.recipe.clone();
    e.document.recipe.upright.mode = UprightMode::Vertical;
    e.history(original.clone());
    // The analysis arrives after the click that chose the mode.
    let (generation, _) = e.document.upright.start();
    let analysed = e.document.recipe.clone();
    let mut corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 6];
    corrections[4][6] = 0.1;
    e.upright_ready(generation, &analysed, Ok(corrections.clone()));
    assert_eq!(e.document.recipe.upright.corrections, corrections);
    // It is not a step of its own: one undo leaves Upright off, redo brings it back
    // corrected.
    assert_eq!(e.document.history.steps().1, 1);
    e.undo();
    assert_eq!(e.document.recipe.upright.mode, UprightMode::Off);
    e.redo();
    assert_eq!(e.document.recipe.upright.mode, UprightMode::Vertical);
    assert_eq!(e.document.recipe.upright.corrections, corrections);
}
#[test]
fn upright_analysis_stays_with_its_photo_and_keeps_imported_guided() {
    use crate::develop::UprightMode;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let identity = [1., 0., 0., 0., 1., 0., 0., 0., 1.];
    let mut guided = identity;
    guided[2] = 0.05;
    e.document.recipe.upright.mode = UprightMode::Guided;
    e.document.recipe.upright.corrections =
        vec![identity, identity, identity, identity, identity, guided];
    // Update analyses the other modes; Lightroom's Guided correction survives.
    let (generation, _) = e.document.upright.start();
    let analysed = e.document.recipe.clone();
    e.upright_ready(generation, &analysed, Ok(vec![identity; 5]));
    assert_eq!(e.document.recipe.upright.corrections[5], guided);
    // Pasted onto another photo, the mode comes along but not the corrections: that
    // photo keeps its own, here none yet, for the editor to analyse.
    e.document.recipe.upright.mode = UprightMode::Vertical;
    e.document.metadata = Some(Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    });
    e.copy_settings(crate::develop::settings_groups::GroupSelection::default());
    e.document.recipe = Recipe::default();
    e.paste_settings();
    assert_eq!(e.document.recipe.upright.mode, UprightMode::Vertical);
    assert!(e.document.recipe.upright.corrections.is_empty());
}
#[test]
fn upright_analysis_yields_to_corrections_applied_meanwhile() {
    use crate::develop::UprightMode;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    e.document.recipe.upright.mode = UprightMode::Level;
    let (generation, _) = e.document.upright.start();
    let analysed = e.document.recipe.clone();
    // A Lightroom preset with its own corrections lands before the analysis.
    let mut imported = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 6];
    imported[3][2] = 0.02;
    e.document.recipe.upright.corrections = imported.clone();
    e.upright_ready(
        generation,
        &analysed,
        Ok(vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 5]),
    );
    assert_eq!(e.document.recipe.upright.corrections, imported);
}

#[test]
fn crop_tool_reads_each_photos_own_aspect() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width: 300,
        height: 200,
        pixels: vec![[0.1; 3]; 60000],
        metadata: Metadata {
            width: 300,
            height: 200,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    editor.document.set_image(image);
    let open = |editor: &mut Editor, crop: [f32; 4]| {
        editor.view.tool = state::Tool::None;
        editor.document.recipe.crop = crop;
        editor.view.toggle(state::Tool::Crop);
        let frame = editor.begin_edit_frame();
        editor.finish_edit_frame(frame, &ctx);
        assert_eq!(editor.document.recipe.crop, crop, "reading changes no crop");
        editor.view.aspect
    };
    // The last photo's XPan crop does not carry over to an uncropped photo.
    editor.view.aspect = 65. / 24.;
    assert_eq!(open(&mut editor, [0., 0., 1., 1.]), -1.);
    // 300 × 111 is 65 x 24 to within rounding.
    let xpan = [0., 0.223, 1., 0.777];
    assert_eq!(open(&mut editor, xpan), 65. / 24.);
    let custom = open(&mut editor, [0., 0., 0.5, 1.]);
    assert!((custom - 0.75).abs() < 1e-6);
    assert_eq!(open(&mut editor, [0.1, 0.1, 0.9, 0.9]), -1.);
}
#[test]
fn a_virtual_copy_made_in_develop_keeps_the_unsaved_edit_and_opens() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"identity fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let l = library::Library::load(&catalog, ctx.clone())?;
    let id = l.photos[0].id;
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.library = Some(Box::new(l));
    editor.library_mode = false;
    editor.document.catalog_photo = Some(id);
    editor.document.path = Some(photo.clone());
    editor.document.recipe.exposure = 0.7;
    editor.document.save.mark_changed();
    editor.virtual_copy(library::CopyAction::Create(id));
    let library = editor.library.as_ref().unwrap();
    let copy = library.photos.iter().find(|p| p.id != id).unwrap();
    assert_eq!((copy.master, copy.copy_name.as_str()), (Some(id), "Copy 1"));
    assert_eq!(library.selected(), Some(copy.id));
    assert_eq!(editor.document.catalog_photo, Some(copy.id));
    for photo_id in [id, copy.id] {
        let saved = library.catalog.load_edit(photo_id, &photo)?.unwrap();
        assert_eq!(saved.recipe.exposure, 0.7);
    }
    Ok(())
}
#[test]
fn removing_a_copy_from_the_library_stays_in_the_library() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("image.ARW"), b"identity fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut l = library::Library::load(&catalog, ctx.clone())?;
    let master = l.photos[0].id;
    let copy = l.create_virtual_copy(master)?;
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.library = Some(Box::new(l));
    // The copy was last open in Develop; the user is back in the Library.
    editor.document.catalog_photo = Some(copy);
    editor.library_mode = true;
    editor.remove_copy = Some(copy);
    // Return alone never confirms the dialog.
    let input = egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| editor.remove_copy_window(ui.ctx()));
    output.textures_delta.clear();
    assert_eq!(editor.remove_copy, Some(copy));
    assert!(editor.library.as_ref().unwrap().photo(copy).is_some());
    editor.remove_copy = None;
    editor.remove_virtual_copy(copy);
    assert!(editor.library_mode);
    assert_eq!(editor.document.catalog_photo, None);
    let library = editor.library.as_ref().unwrap();
    assert!(library.photo(copy).is_none());
    assert_eq!(library.selected(), Some(master));
    Ok(())
}
#[test]
fn a_copy_name_that_cannot_be_saved_keeps_the_app_from_moving_on() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("image.ARW"), b"identity fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut l = library::Library::load(&catalog, ctx.clone())?;
    let copy = l.create_virtual_copy(l.photos[0].id)?;
    // A copy that is gone from the catalog cannot be renamed.
    l.catalog.remove_virtual_copy(copy)?;
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.library = Some(Box::new(l));
    editor
        .library
        .as_mut()
        .unwrap()
        .set_copy_name_draft(copy, "B&W");
    assert!(!editor.flush());
    editor.library.as_mut().unwrap().discard_drafts();
    assert!(editor.flush());
    Ok(())
}
#[test]
fn opening_a_file_picks_its_master_after_a_copy_is_promoted() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"identity fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut l = library::Library::load(&catalog, ctx.clone())?;
    let copy = l.create_virtual_copy(l.photos[0].id)?;
    l.set_copy_as_master(copy)?;
    let path = l.photo(copy).unwrap().path.clone();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.library = Some(Box::new(l));
    assert_eq!(editor.catalog_photo_at(&path), Some(copy));
    Ok(())
}
/// An editor with a catalog of `names`, its Library open.
fn editor_with_catalog(names: &[&str]) -> anyhow::Result<(tempfile::TempDir, Editor, Vec<i64>)> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    for name in names {
        std::fs::write(photos.join(name), b"fixture")?;
    }
    let path = d.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&path)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let library = crate::app::library::Library::load(&path, ctx.clone())?;
    let ids = library.photos.iter().map(|p| p.id).collect();
    e.library = Some(Box::new(library));
    e.library_mode = true;
    Ok((d, e, ids))
}
#[test]
fn undo_brings_back_a_range_rejected_under_the_unflagged_filter() -> anyhow::Result<()> {
    use crate::app::photo_metadata::Edit;
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF", "d.RAF", "e.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.show_unflagged();
    library.select(Some(ids[1]));
    library.select_range_to(ids[3]);
    library.edit_selection(Edit::Flag(-1), false)?;
    assert_eq!(library.shown().len(), 2);
    e.undo();
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.shown().len(), 5);
    assert_eq!(library.selected(), Some(ids[3]));
    assert_eq!(library.selected_photos(), ids[1..4]);
    assert!(library.photos.iter().all(|p| p.flag == 0));
    assert!(e.status.starts_with("Undo 3 photos"));
    e.redo();
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.shown().len(), 2);
    assert_eq!(library.photos.iter().filter(|p| p.flag == -1).count(), 3);
    // A write that fails is never logged.
    let library = e.library.as_mut().unwrap();
    assert!(library.edit_metadata(9999, Edit::Rating(5), false).is_err());
    e.sync_undo();
    assert_eq!(e.undo_log.len(), (1, 0));
    Ok(())
}
#[test]
fn undo_in_develop_reverses_the_flag_before_the_exposure() -> anyhow::Result<()> {
    use crate::app::photo_metadata::Edit;
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.library_mode = false;
    e.document.catalog_photo = Some(ids[0]);
    e.document.path = Some(d.path().join("photos/a.RAF"));
    let original = e.document.recipe.clone();
    e.document.recipe.exposure = 1.;
    e.history(original.clone());
    e.library
        .as_mut()
        .unwrap()
        .edit_metadata(ids[0], Edit::Flag(1), false)?;
    e.undo();
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().flag, 0);
    assert_eq!(e.document.recipe.exposure, 1.);
    assert!(!e.library_mode);
    e.undo();
    assert_eq!(e.document.recipe, original);
    e.redo();
    e.redo();
    assert_eq!(e.document.recipe.exposure, 1.);
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().flag, 1);
    Ok(())
}
#[test]
fn undoing_a_history_click_returns_to_the_exact_step() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    for exposure in [0.25, 0.5, 1.] {
        let before = e.document.recipe.clone();
        e.document.recipe.exposure = exposure;
        e.history(before);
    }
    // A click on the first state jumps three steps back as one command.
    assert!(e.document.history.jump(0, &mut e.document.recipe));
    assert_eq!(e.document.recipe.exposure, 0.);
    e.undo();
    assert_eq!(e.document.recipe.exposure, 1.);
    assert_eq!(e.document.history.steps().1, 3);
    e.redo();
    assert_eq!(e.document.history.steps().1, 0);
    // The History panel still lists every step.
    assert_eq!(e.document.history.steps().0.len(), 3);
}
#[test]
fn a_library_change_is_undone_in_the_library() -> anyhow::Result<()> {
    use crate::app::photo_metadata::Edit;
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[1]));
    library.edit_selection(Edit::Rating(4), false)?;
    e.sync_undo();
    // Off to Develop on the other photo.
    e.library_mode = false;
    e.document.catalog_photo = Some(ids[0]);
    e.document.path = Some(d.path().join("photos/a.RAF"));
    e.library.as_mut().unwrap().select(Some(ids[0]));
    e.undo();
    assert!(e.library_mode);
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.photo(ids[1]).unwrap().rating, 0);
    assert_eq!(library.selected(), Some(ids[1]));
    Ok(())
}
#[test]
fn undoing_a_history_click_after_a_new_branch_restores_its_own_state() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let before = e.document.recipe.clone();
    e.document.recipe.exposure = 0.5;
    e.history(before);
    // Click the opened state, then edit: History branches.
    assert!(e.document.history.jump(0, &mut e.document.recipe));
    let before = e.document.recipe.clone();
    e.document.recipe.exposure = 1.;
    e.history(before);
    e.undo();
    assert_eq!(e.document.recipe.exposure, 0.);
    // The same step count now leads to the other branch; the click's own
    // state comes back.
    e.undo();
    assert_eq!(e.document.recipe.exposure, 0.5);
}
#[test]
fn a_key_that_changes_nothing_is_not_an_undo_step() -> anyhow::Result<()> {
    use crate::app::photo_metadata::Edit;
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.edit_metadata(ids[0], Edit::Rating(5), false)?;
    library.edit_metadata(ids[0], Edit::Rating(5), false)?;
    e.sync_undo();
    assert_eq!(e.undo_log.len(), (1, 0));
    e.undo();
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().rating, 0);
    Ok(())
}
fn press(e: &mut Editor, key: egui::Key, modifiers: egui::Modifiers) {
    let ctx = e.context.clone();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            // Pressed and released, as a real key is, so the next press of
            // the same key is not a repeat.
            events: [true, false]
                .map(|pressed| egui::Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed,
                    repeat: false,
                    modifiers,
                })
                .into(),
            ..Default::default()
        },
        |ui| e.draw(ui),
    );
    output.textures_delta.clear();
}
#[test]
fn loupe_keys_change_only_the_photo_shown_and_auto_advance_moves_on() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[1]);
    press(&mut e, egui::Key::E, egui::Modifiers::NONE);
    assert!(e.library.as_ref().unwrap().loupe_open());
    // Cmd+Up picks the photo shown, not the rest of the selection.
    press(&mut e, egui::Key::ArrowUp, egui::Modifiers::COMMAND);
    let library = e.library.as_ref().unwrap();
    let flags: Vec<_> = ids
        .iter()
        .map(|id| library.photo(*id).unwrap().flag)
        .collect();
    assert_eq!(flags, [0, 1, 0]);
    press(&mut e, egui::Key::Escape, egui::Modifiers::NONE);
    assert!(!e.library.as_ref().unwrap().loupe_open());
    // With Auto Advance a key moves on as Shift would.
    e.auto_advance = true;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    press(&mut e, egui::Key::Num3, egui::Modifiers::NONE);
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.photo(ids[0]).unwrap().rating, 3);
    assert_eq!(library.selected(), Some(ids[1]));
    Ok(())
}
#[test]
fn double_click_in_the_loupe_goes_back_to_the_grid() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    e.library.as_mut().unwrap().open_loupe();
    let ctx = e.context.clone();
    let at = Pos2::new(600., 300.);
    let click = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let frame = |e: &mut Editor, events: Vec<egui::Event>, time: f64| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| e.draw(ui),
        );
        output.textures_delta.clear();
    };
    frame(&mut e, vec![egui::Event::PointerMoved(at)], 0.0);
    frame(&mut e, vec![click(true)], 0.1);
    frame(&mut e, vec![click(false)], 0.15);
    frame(&mut e, vec![click(true)], 0.2);
    frame(&mut e, vec![click(false)], 0.25);
    frame(&mut e, vec![], 0.3);
    assert!(!e.library.as_ref().unwrap().loupe_open());
    // A click just before a double-click makes egui count a triple click.
    e.library.as_mut().unwrap().open_loupe();
    for (pressed, time) in [
        (true, 1.0),
        (false, 1.05),
        (true, 1.2),
        (false, 1.25),
        (true, 1.4),
        (false, 1.45),
    ] {
        frame(&mut e, vec![click(pressed)], time);
    }
    frame(&mut e, vec![], 1.5);
    assert!(!e.library.as_ref().unwrap().loupe_open());
    Ok(())
}
#[test]
fn quick_collection_toggles_shows_clears_and_undoes() -> anyhow::Result<()> {
    let (_d, mut e, ids) = editor_with_catalog(&["a.RAF", "b.RAF", "c.RAF"])?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[1]);
    press(&mut e, egui::Key::B, egui::Modifiers::NONE);
    press(&mut e, egui::Key::B, egui::Modifiers::COMMAND);
    let library = e.library.as_ref().unwrap();
    assert_eq!(library.shown(), ids[..2]);
    assert_eq!(library.source_name(), "Quick Collection");
    // B again on photos all in it takes them out.
    press(&mut e, egui::Key::B, egui::Modifiers::NONE);
    assert!(e.library.as_ref().unwrap().shown().is_empty());
    e.undo();
    assert_eq!(e.library.as_ref().unwrap().shown(), ids[..2]);
    press(
        &mut e,
        egui::Key::B,
        egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
    );
    assert!(e.library.as_ref().unwrap().shown().is_empty());
    e.undo();
    assert_eq!(e.library.as_ref().unwrap().shown(), ids[..2]);
    // Kept in the catalog.
    let reopened = crate::catalog::Catalog::open(&e.library.as_ref().unwrap().catalog.path)?;
    let quick = reopened
        .collections()?
        .into_iter()
        .find(|c| c.name == crate::catalog::QUICK_COLLECTION)
        .unwrap();
    assert_eq!(reopened.collection_photos()?[&quick.id].len(), 2);
    Ok(())
}

#[test]
fn the_preset_list_is_kept_until_what_it_shows_changes() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let preset = |id: &str, group: &str, name: &str| crate::xmp::Preset {
        id: id.into(),
        name: name.into(),
        group: group.into(),
        path: Default::default(),
        settings: Default::default(),
        curves: Default::default(),
        look: String::new(),
        blockers: Vec::new(),
        notes: Vec::new(),
        photo_settings: false,
        local: Default::default(),
        builtin: false,
    };
    editor.presets.library = std::sync::Arc::new(crate::presets::Library {
        presets: vec![
            preset("a", "Film", "Warm⁺"),
            preset("b", "Film", "Cool"),
            preset("c", "Mono", "Grain"),
        ],
        errors: Vec::new(),
    });
    let names = |editor: &mut Editor| -> Vec<(String, Vec<String>)> {
        editor
            .preset_list()
            .iter()
            .map(|g| {
                (
                    g.name.clone(),
                    g.presets.iter().map(|(_, n)| n.clone()).collect(),
                )
            })
            .collect()
    };
    assert_eq!(
        names(&mut editor),
        [
            ("Film".into(), vec!["Warm+".into(), "Cool".into()]),
            ("Mono".into(), vec!["Grain".into()])
        ]
    );
    // Kept while nothing it depends on changes.
    let first = editor.preset_list();
    assert!(std::sync::Arc::ptr_eq(&first, &editor.preset_list()));
    // Rebuilt for a search, and for a favorite.
    editor.presets.filter = "WARM+".into();
    assert_eq!(names(&mut editor), [("Film".into(), vec!["Warm+".into()])]);
    editor.presets.filter.clear();
    editor.presets.favorites_only = true;
    assert!(names(&mut editor).is_empty());
    editor.presets.favorites.insert("c".into());
    editor.presets.revision += 1;
    assert_eq!(names(&mut editor), [("Mono".into(), vec!["Grain".into()])]);
    // Rebuilt when another photo's compatibility replaces this one's.
    editor.presets.favorites_only = false;
    editor.presets.compatible_only = true;
    editor.presets.issues = vec![Some("no".into()), None, Some("no".into())];
    editor.presets.revision += 1;
    assert_eq!(names(&mut editor), [("Film".into(), vec!["Cool".into()])]);
    editor.presets.clear_document();
    assert_eq!(names(&mut editor).len(), 2);
}
#[test]
fn double_clicking_a_defringe_hue_resets_it_to_its_colors_default() {
    let ctx = egui::Context::default();
    let mut effects = crate::develop::effects::Effects {
        // Green below its default, as the fringe selector can leave it.
        defringe_ranges: [[0.1, 0.95], [0.0, 0.2]],
        ..Default::default()
    };
    let mut time = 0.;
    let mut frame =
        |effects: &mut crate::develop::effects::Effects, events: Vec<egui::Event>, wait: f64| {
            time += wait;
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320., 400.))),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| super::inspector::defringe_sliders(ui, effects),
            );
            output.textures_delta.clear();
        };
    // Six rows, Purple then Green: Amount, Hue (low end), Hue (high end).
    frame(&mut effects, vec![], 0.);
    let row = 24. + ctx.global_style().spacing.item_spacing.y;
    // Either end resets the whole range: Purple's high end, Green's low end.
    for i in [2, 4] {
        let at = Pos2::new(
            40.,
            ctx.global_style().spacing.window_margin.top as f32 + row * i as f32 + 12.,
        );
        let click = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        // Apart in time, so egui never counts a triple click.
        frame(&mut effects, vec![egui::Event::PointerMoved(at)], 1.);
        for pressed in [true, false, true, false] {
            frame(&mut effects, vec![click(pressed)], 0.05);
        }
    }
    assert_eq!(effects.defringe_ranges, [[0.3, 0.7], [0.4, 0.6]]);
    // History names the range reset, not the value the clamped end briefly took.
    let step: Option<(String, String)> =
        ctx.data(|d| d.get_temp(super::widgets::history_step_id()));
    assert_eq!(
        step,
        Some(("Defringe Green Hue".to_string(), "40 / 60".to_string()))
    );
}
#[test]
fn paste_works_out_white_balance_for_this_camera_and_previous_pastes_the_last_photo() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let camera = |wb: [f32; 3]| Metadata {
        wb,
        daylight_wb: wb,
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let (first, second) = (camera([2., 1., 1.8]), camera([2.6, 1., 1.3]));
    // A photo from one camera, its settings copied.
    editor.document.metadata = Some(first.clone());
    let mut copied = Recipe {
        temperature: 4200.,
        tint: 8.,
        exposure: 0.4,
        ..Default::default()
    };
    copied.update_wb(&first);
    editor.document.recipe = copied.clone();
    editor.copy_settings(crate::develop::settings_groups::GroupSelection::default());
    // Pasted onto a photo from another camera.
    editor.document.metadata = Some(second.clone());
    editor.document.recipe = Recipe::default();
    editor.paste_settings();
    let pasted = editor.document.recipe.clone();
    assert_eq!((pasted.temperature, pasted.exposure), (4200., 0.4));
    let mut expected = pasted.clone();
    expected.update_wb(&second);
    assert_eq!(pasted.wb, expected.wb);
    assert_ne!(pasted.wb, copied.wb);
    // Moving to another photo makes this one's settings the Previous; opening that
    // photo again leaves it.
    let other = std::path::PathBuf::from("missing-previous-fixture.ARW");
    editor.open_raw(other.clone(), None);
    editor.document.metadata = Some(first);
    editor.document.path = Some(other.clone());
    editor.document.recipe.exposure = -1.;
    editor.open_raw(other, None);
    editor.document.metadata = Some(second);
    editor.paste_previous();
    assert_eq!(editor.document.recipe.exposure, 0.4);
}
#[test]
fn copy_settings_copies_the_chosen_groups_and_remembers_them() {
    use crate::develop::settings_groups::{GroupInclusion, GroupSelection, SettingGroup};
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.document.metadata = Some(Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    });
    editor.document.recipe.exposure = 0.6;
    editor.document.recipe.contrast = 0.3;
    editor.open_copy_dialog(settings_transfer::Transfer::Copy);
    // The dialog draws, with its buttons in view in the smallest window.
    for _ in 0..2 {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000., 650.))),
                ..Default::default()
            },
            |ui| editor.draw(ui),
        );
        output.textures_delta.clear();
    }
    let copy = ctx.memory(|m| m.area_rect(egui::Id::new("copy-settings")));
    assert!(copy.is_some_and(|r| r.bottom() <= 650.), "{copy:?}");
    let dialog = editor.copy_dialog.as_mut().unwrap();
    dialog.groups = GroupSelection::none();
    dialog
        .groups
        .set(SettingGroup::Exposure, GroupInclusion::Included);
    editor.close_copy_dialog(settings_transfer::CopyChoice::Confirm);
    assert!(editor.copy_dialog.is_none());
    editor.document.recipe = Recipe::default();
    editor.paste_settings();
    assert_eq!(editor.document.recipe.exposure, 0.6);
    assert_eq!(editor.document.recipe.contrast, 0.);
    // The next Copy Settings starts from that choice; Cancel copies nothing.
    editor.open_copy_dialog(settings_transfer::Transfer::Copy);
    assert!(
        editor
            .copy_dialog
            .as_ref()
            .unwrap()
            .groups
            .contains(SettingGroup::Exposure)
    );
    assert!(
        !editor
            .copy_dialog
            .as_ref()
            .unwrap()
            .groups
            .contains(SettingGroup::Contrast)
    );
    editor.document.recipe.exposure = -1.;
    editor.close_copy_dialog(settings_transfer::CopyChoice::Cancel);
    editor.document.recipe = Recipe::default();
    editor.paste_settings();
    assert_eq!(editor.document.recipe.exposure, 0.6);
}
#[test]
fn j_toggles_both_clipping_warnings_but_not_while_typing() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let mut text = String::new();
    let j = || egui::Event::Key {
        key: egui::Key::J,
        physical_key: Some(egui::Key::J),
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    let mut frame = |events: Vec<egui::Event>, typing: bool, e: &mut Editor| {
        let mut output = ctx.run_ui(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ui| {
                if typing {
                    ui.text_edit_singleline(&mut text).request_focus();
                }
                e.develop_shortcuts(&ctx);
            },
        );
        output.textures_delta.clear();
    };
    frame(vec![], false, &mut e);
    frame(vec![j()], false, &mut e);
    let both = develop::ClipOverlay {
        shadows: true,
        highlights: true,
    };
    assert_eq!(e.view.clipping.overlay(), both);
    // A J typed into a field stays there.
    frame(vec![], true, &mut e);
    frame(vec![j()], true, &mut e);
    assert_eq!(e.view.clipping.overlay(), both);
    frame(vec![], false, &mut e);
    frame(vec![j()], false, &mut e);
    assert_eq!(e.view.clipping.overlay(), develop::ClipOverlay::NONE);
}
#[test]
fn a_hovered_clipping_triangle_shows_its_warning_until_the_pointer_leaves() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    let frame = e.begin_edit_frame();
    // As the histogram does while its highlight triangle is under the pointer.
    e.view
        .clipping
        .set_hover(Some(super::clipping::ClipSide::Highlights));
    e.finish_edit_frame(frame, &ctx);
    assert!(e.view.clipping.overlay().highlights);
    // The next frame draws no hovered triangle: the warning goes, and nothing
    // was turned on.
    let frame = e.begin_edit_frame();
    e.finish_edit_frame(frame, &ctx);
    assert_eq!(e.view.clipping.overlay(), develop::ClipOverlay::NONE);
}
#[test]
fn crop_keys_swap_and_cycle_the_overlay_but_not_while_typing() -> anyhow::Result<()> {
    let (d, mut e, ids) = editor_with_catalog(&["a.RAF"])?;
    let session = d.path().join("session.json");
    e.session_file = Some(session.clone());
    e.library_mode = false;
    e.document.catalog_photo = Some(ids[0]);
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 300,
        height: 200,
        pixels: vec![[0.1; 3]; 60000],
        metadata: Metadata {
            width: 300,
            height: 200,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    let crop = [0.1, 0.2, 0.5, 0.6];
    e.document.recipe.crop = crop;
    e.view.toggle(state::Tool::Crop);
    let ctx = e.context.clone();
    let press = |e: &mut Editor, key: egui::Key, shift: bool, typing: bool| {
        let modifiers = egui::Modifiers {
            shift,
            ..Default::default()
        };
        let mut text = String::new();
        // A text field takes focus in one frame and has it for the keys in the next.
        for events in [vec![], vec![true, false]] {
            let input = egui::RawInput {
                events: std::iter::once(egui::Event::ModifiersChanged(modifiers))
                    .chain(events.into_iter().map(|pressed| egui::Event::Key {
                        key,
                        physical_key: Some(key),
                        pressed,
                        repeat: false,
                        modifiers,
                    }))
                    .collect(),
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                if typing {
                    ui.text_edit_singleline(&mut text).request_focus();
                }
                e.metadata_shortcuts(&ctx);
                e.develop_shortcuts(&ctx);
            });
            output.textures_delta.clear();
        }
    };
    // Typing an X or an O in a field changes nothing.
    press(&mut e, egui::Key::X, false, true);
    press(&mut e, egui::Key::O, false, true);
    assert_eq!(e.document.recipe.crop, crop);
    assert_eq!(e.view.crop_guides, Default::default());
    // X swaps the 120 × 80 crop to 80 × 120 about its centre, and rejects nothing.
    press(&mut e, egui::Key::X, false, false);
    let c = e.document.recipe.crop;
    assert!(
        ((c[2] - c[0]) * 300. - 80.).abs() < 1e-3 && ((c[3] - c[1]) * 200. - 120.).abs() < 1e-3,
        "{c:?}"
    );
    assert!(((c[0] + c[2]) / 2. - 0.3).abs() < 1e-6 && ((c[1] + c[3]) / 2. - 0.4).abs() < 1e-6);
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().flag, 0);
    // O cycles the overlay and Shift+O turns it; both are kept in the session.
    use super::crop_tool::Guide;
    press(&mut e, egui::Key::O, false, false);
    assert_eq!(e.view.crop_guides.guide, Guide::Diagonal);
    press(&mut e, egui::Key::O, false, false);
    press(&mut e, egui::Key::O, true, false);
    assert_eq!(
        (e.view.crop_guides.guide, e.view.crop_guides.orientation),
        (Guide::Triangle, 1)
    );
    // Shift released before the frame is drawn: the press still had it.
    let shifted = egui::Modifiers {
        shift: true,
        ..Default::default()
    };
    let mut output = ctx.run_ui(
        egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(shifted),
                egui::Event::Key {
                    key: egui::Key::O,
                    physical_key: Some(egui::Key::O),
                    pressed: true,
                    repeat: false,
                    modifiers: shifted,
                },
                egui::Event::ModifiersChanged(egui::Modifiers::NONE),
            ],
            ..Default::default()
        },
        |_| e.develop_shortcuts(&ctx),
    );
    output.textures_delta.clear();
    assert_eq!(
        (e.view.crop_guides.guide, e.view.crop_guides.orientation),
        (Guide::Triangle, 0)
    );
    press(&mut e, egui::Key::O, true, false);
    let saved: crate::storage::Session = serde_json::from_slice(&std::fs::read(&session)?)?;
    assert_eq!(saved.crop_guides.guide, "triangle");
    assert_eq!(saved.crop_guides.orientation, 1);
    // Outside the Crop tool X rejects as before.
    e.view.tool = state::Tool::None;
    press(&mut e, egui::Key::X, false, false);
    assert_eq!(e.library.as_ref().unwrap().photo(ids[0]).unwrap().flag, -1);
    Ok(())
}

#[test]
fn auto_straighten_sets_the_level_angle_as_one_step_and_measures_again_after_a_turn() {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    // Stripes falling 3° to the right.
    let (w, h) = (600usize, 400usize);
    let (s, c) = 3f32.to_radians().sin_cos();
    let pixels = (0..w * h)
        .map(|i| {
            // Supersampled, so edges are smooth rather than pixel steps.
            let mut sum = 0.;
            for j in 0..4 {
                let x = (i % w) as f32 + (j % 2) as f32 * 0.5 - 299.75;
                let y = (i / w) as f32 + (j / 2) as f32 * 0.5 - 199.75;
                let across = -s * x + c * y;
                sum += if (across / 70.).rem_euclid(2.) < 1. {
                    0.78
                } else {
                    0.1
                };
            }
            [sum / 4.; 3]
        })
        .collect();
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: w as u32,
        height: h as u32,
        pixels,
        metadata: Metadata {
            width: w as u32,
            height: h as u32,
            wb: [1.; 3],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    e.document.recipe.wb = [1.; 3];
    let upright = e.document.recipe.upright.clone();
    let next = |e: &mut Editor| loop {
        match e.rx.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(worker::Event::Straighten {
                generation,
                analysed,
                result,
                ..
            }) => break (generation, analysed, result),
            Ok(_) => continue,
            Err(err) => panic!("no Auto straighten result: {err}"),
        }
    };
    e.start_auto_straighten();
    assert!(e.document.straighten.is_running());
    // Measured before the photo was turned: it is measured again, and nothing changes yet.
    let (generation, analysed, result) = next(&mut e);
    crate::develop::turn(&mut e.document.recipe, crate::develop::QuarterTurn::Right);
    crate::develop::turn(&mut e.document.recipe, crate::develop::QuarterTurn::Left);
    e.document.recipe.flip_x = true;
    e.auto_straighten_ready(generation, &analysed, result);
    assert_eq!(e.document.recipe.straighten, 0.);
    assert!(e.document.straighten.is_running(), "measured again");
    e.document.recipe.flip_x = false;
    let (generation, analysed, result) = next(&mut e);
    e.auto_straighten_ready(generation, &analysed, result);
    // The stale analysis restarted once more for the flip back; take its result.
    let (generation, analysed, result) = next(&mut e);
    e.auto_straighten_ready(generation, &analysed, result);
    let angle = e.document.recipe.straighten;
    assert!((angle + 3.).abs() < 0.2, "{angle}");
    let (steps, applied) = e.document.history.steps();
    assert_eq!(applied, 1);
    assert_eq!(
        (steps[0].name.as_str(), steps[0].value.as_str()),
        ("Straighten", "Auto")
    );
    // Upright is left as it was.
    assert_eq!(e.document.recipe.upright, upright);
    e.undo();
    assert_eq!(e.document.recipe.straighten, 0.);
}

#[test]
fn the_crop_drawer_keeps_its_layout_whatever_its_buttons_show() {
    let ctx = egui::Context::default();
    super::icons::install(&ctx);
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    e.view.tool = state::Tool::Crop;
    let size = |e: &mut Editor, width: f32| {
        let mut rect = Rect::NOTHING;
        for _ in 0..2 {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(width);
                    rect = ui.scope(|ui| e.tool_strip(ui)).response.rect;
                },
            );
            output.textures_delta.clear();
        }
        rect
    };
    let plain = size(&mut e, 320.);
    e.view.ruler = super::crop_tool::Ruler::Armed;
    let _ = e.document.straighten.start();
    e.view.aspect = 0.8;
    e.view.crop_guides.guide = super::crop_tool::Guide::GoldenSpiral;
    let busy = size(&mut e, 320.);
    assert_eq!(plain.size(), busy.size());
}
#[test]
fn leaving_a_photo_mid_drag_saves_the_drag_as_a_history_step() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"gesture fixture")?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let l = crate::app::library::Library::load(&catalog, ctx.clone())?;
    let id = l.photos[0].id;
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.library = Some(Box::new(l));
    editor.document.catalog_photo = Some(id);
    editor.document.path = Some(photo.clone());
    // A slider still held down when Left or Right leaves the photo.
    let before = editor.document.recipe.clone();
    editor.document.recipe.exposure = 0.6;
    editor
        .document
        .history
        .observe(before, &editor.document.recipe, true);
    editor.document.save.mark_changed();
    assert!(editor.flush());
    assert!(!editor.document.history.in_gesture());
    let library = editor.library.as_ref().unwrap();
    let history = library.catalog.load_history(id)?.unwrap();
    assert_eq!(history.applied, 1);
    assert_eq!(history.steps.len(), 1);
    assert_eq!(history.steps[0].recipe.exposure, 0.6);
    // Undo on the next photo reaches it.
    assert_eq!(editor.undo_log.len(), (1, 0));
    // A save that fails keeps the drag going, as one step.
    editor.document.catalog_photo = Some(id + 1000);
    let before = editor.document.recipe.clone();
    editor.document.recipe.exposure = 0.9;
    editor
        .document
        .history
        .observe(before, &editor.document.recipe, true);
    editor.document.save.mark_changed();
    assert!(!editor.flush());
    assert!(editor.document.history.in_gesture());
    assert_eq!(editor.undo_log.len(), (1, 0));
    Ok(())
}
#[test]
fn undo_during_a_drag_takes_back_the_drag_and_can_be_redone() {
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    editor.library_mode = false;
    let edit = |editor: &mut Editor, exposure: f32, held: bool| {
        let before = editor.document.recipe.clone();
        editor.document.recipe.exposure = exposure;
        editor
            .document
            .history
            .observe(before, &editor.document.recipe, held);
        editor.sync_undo();
    };
    edit(&mut editor, 0.3, false);
    edit(&mut editor, 0.6, true);
    editor.undo();
    assert_eq!(editor.document.recipe.exposure, 0.3);
    assert_eq!(editor.undo_log.len(), (1, 1));
    editor.redo();
    assert_eq!(editor.document.recipe.exposure, 0.6);
}

/// The Guided Upright tool on the photo: drawing a guide, drawing a second, moving an
/// end and deleting a guide are one History step each, and two guides solve to a
/// correction (docs/transform.md#guided-upright).
#[test]
fn guided_upright_gestures_are_one_history_step_each() {
    use crate::develop::UprightMode;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    e.library_mode = false;
    e.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 300,
        height: 200,
        pixels: vec![[0.1; 3]; 60000],
        metadata: Metadata {
            width: 300,
            height: 200,
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    // The other modes analysed already, so the guides are solved at once.
    e.document.recipe.upright.corrections = vec![[1., 0., 0., 0., 1., 0., 0., 0., 1.]; 5];
    e.view.tool = state::Tool::Guided;
    let area = Rect::from_min_size(Pos2::ZERO, Vec2::new(600., 400.));
    let mut frame = |e: &mut Editor, events: Vec<egui::Event>| {
        let edit = e.begin_edit_frame();
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(area),
                events,
                ..Default::default()
            },
            |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(area.size(), egui::Sense::click_and_drag());
                e.guided_overlay(ui, &response, rect, rect);
            },
        );
        output.textures_delta.clear();
        e.finish_edit_frame(edit, &ctx);
    };
    let button = |p: Pos2, pressed| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    };
    let drag = |e: &mut Editor,
                frame: &mut dyn FnMut(&mut Editor, Vec<egui::Event>),
                from: Pos2,
                to: Pos2| {
        frame(e, vec![egui::Event::PointerMoved(from), button(from, true)]);
        for k in 1..=4 {
            frame(
                e,
                vec![egui::Event::PointerMoved(
                    from + (to - from) * k as f32 / 4.,
                )],
            );
        }
        frame(e, vec![button(to, false)]);
        frame(e, vec![]);
    };
    frame(&mut e, vec![]);
    // Two converging verticals.
    drag(
        &mut e,
        &mut frame,
        Pos2::new(120., 40.),
        Pos2::new(100., 360.),
    );
    let guides = &e.document.recipe.upright.guides;
    assert_eq!(guides.len(), 1);
    assert!(
        (guides[0].a[0] - 0.2).abs() < 1e-3 && (guides[0].a[1] - 0.1).abs() < 1e-3,
        "{guides:?}"
    );
    assert_eq!(e.document.recipe.upright.mode, UprightMode::Guided);
    drag(
        &mut e,
        &mut frame,
        Pos2::new(480., 40.),
        Pos2::new(500., 360.),
    );
    assert_eq!(e.document.recipe.upright.guides.len(), 2);
    let solved = e
        .document
        .recipe
        .upright
        .correction()
        .expect("a correction");
    assert!(solved[2][1].abs() > 1e-3, "{solved:?}");
    // The first guide's lower end, where it shows now the photo is corrected.
    let g = develop::Geometry::new(e.document.full().unwrap(), &e.document.recipe, 0);
    let [u, v] = g.from_upright_frame(e.document.recipe.upright.guides[0].b);
    let end = Pos2::new(u * 600., v * 400.);
    drag(&mut e, &mut frame, end, end + Vec2::new(-10., 0.));
    assert_eq!(e.document.recipe.upright.guides.len(), 2);
    // Selected by the drag, and deleted.
    let edit = e.begin_edit_frame();
    e.delete_guide();
    e.finish_edit_frame(edit, &ctx);
    assert_eq!(e.document.recipe.upright.guides.len(), 1);
    let (steps, applied) = e.document.history.steps();
    let names: Vec<&str> = steps.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["Add Guide", "Add Guide", "Move Guide", "Delete Guide"]
    );
    assert_eq!(applied, 4);
    // One guide corrects nothing, and says so.
    assert_eq!(e.document.recipe.upright.correction(), None);
    assert!(e.status.contains("two or more guides"), "{}", e.status);
    e.undo();
    assert_eq!(e.document.recipe.upright.guides.len(), 2);
    assert!(e.document.recipe.upright.correction().is_some());
    // Leaving Guided by any route closes the tool, so a drag can't switch it back.
    e.view.tool = state::Tool::Guided;
    let edit = e.begin_edit_frame();
    e.document.recipe.upright = Default::default();
    e.finish_edit_frame(edit, &ctx);
    assert!(!e.view.is(state::Tool::Guided));
}

/// A small photo of colors spread along blue, with its metadata; decoded unless
/// `decoded` is false.
fn editor_with_blue_photo(
    ctx: &egui::Context,
    session: crate::storage::Session,
    decoded: bool,
) -> (Editor, Arc<CameraImage>) {
    let mut editor = Editor::with_context(ctx, None, session, None);
    editor.library_mode = false;
    let (width, height) = (32u32, 24u32);
    let metadata = Metadata {
        width,
        height,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    editor.document.metadata = Some(metadata.clone());
    let image = Arc::new(CameraImage {
        recovered: Default::default(),
        width,
        height,
        pixels: (0..width * height)
            .map(|i| {
                let v = 0.05 + 0.3 * (i % width) as f32 / width as f32;
                [v, v, 2. * v]
            })
            .collect(),
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    });
    if decoded {
        editor.document.set_image(image.clone());
    }
    (editor, image)
}
/// Runs `action` in an edit frame, as the panels and shortcuts do.
fn in_edit_frame(ctx: &egui::Context, editor: &mut Editor, action: impl FnOnce(&mut Editor)) {
    let frame = editor.begin_edit_frame();
    action(editor);
    editor.finish_edit_frame(frame, ctx);
}
#[test]
fn v_converts_to_black_and_white_with_the_auto_mix_as_one_step() {
    let ctx = egui::Context::default();
    let (mut editor, _) = editor_with_blue_photo(&ctx, crate::storage::Session::default(), true);
    let before = editor.document.recipe.clone();
    // V, through a whole frame of the Develop module.
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events: vec![egui::Event::Key {
                key: egui::Key::V,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| editor.draw(ui),
    );
    output.textures_delta.clear();
    // Held down, V repeats; the repeats change nothing.
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1200., 800.))),
            events: vec![egui::Event::Key {
                key: egui::Key::V,
                physical_key: None,
                pressed: true,
                repeat: true,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        },
        |ui| editor.draw(ui),
    );
    output.textures_delta.clear();
    let auto = editor
        .photo_colors()
        .unwrap()
        .auto_mix()
        .for_recipe(&editor.document.recipe);
    let r = &editor.document.recipe;
    assert!(r.effects.monochrome);
    assert_ne!(auto, [0.; 8]);
    assert_eq!(r.effects.gray_mix, auto);
    let (steps, applied) = editor.document.history.steps();
    assert_eq!(
        (applied, steps[0].name.as_str()),
        (1, "Convert to Black & White")
    );
    // Back to color keeps the mix for the next conversion, as Lightroom does.
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    assert!(!editor.document.recipe.effects.monochrome);
    assert_eq!(editor.document.recipe.effects.gray_mix, auto);
    let (steps, applied) = editor.document.history.steps();
    assert_eq!((applied, steps[1].name.as_str()), (2, "Convert to Color"));
    editor.undo();
    editor.undo();
    assert_eq!(editor.document.recipe, before);

    // The B&W panel's Auto brings back the Auto mix after a slider moved, as one step.
    editor.redo();
    in_edit_frame(&ctx, &mut editor, |e| {
        e.document.recipe.effects.gray_mix[3] = 0.6
    });
    in_edit_frame(&ctx, &mut editor, Editor::auto_black_white_mix);
    assert_eq!(editor.document.recipe.effects.gray_mix, auto);
    let (steps, applied) = editor.document.history.steps();
    assert_eq!(applied, 3);
    assert_eq!(
        (steps[2].name.as_str(), steps[2].value.as_str()),
        ("Black & White Mix", "Auto")
    );
    // With the preference off, the first conversion keeps the mix at zero.
    let session = crate::storage::Session {
        no_auto_black_white_mix: true,
        ..Default::default()
    };
    let (mut editor, _) = editor_with_blue_photo(&ctx, session, true);
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    assert!(editor.document.recipe.effects.monochrome);
    assert_eq!(editor.document.recipe.effects.gray_mix, [0.; 8]);
}
#[test]
fn converting_while_the_photo_decodes_waits_for_its_auto_mix() {
    let ctx = egui::Context::default();
    // V twice while decoding: the second cancels the first.
    let (mut editor, image) =
        editor_with_blue_photo(&ctx, crate::storage::Session::default(), false);
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    editor.document.set_image(image);
    in_edit_frame(&ctx, &mut editor, Editor::finish_pending_treatment);
    assert!(!editor.document.recipe.effects.monochrome);
    assert_eq!(editor.document.history.steps().1, 0);
    // Once: the conversion waits for the photo.
    let (mut editor, image) =
        editor_with_blue_photo(&ctx, crate::storage::Session::default(), false);
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    assert!(!editor.document.recipe.effects.monochrome);
    assert_eq!(editor.document.history.steps().1, 0);
    editor.document.set_image(image);
    in_edit_frame(&ctx, &mut editor, Editor::finish_pending_treatment);
    let r = &editor.document.recipe;
    assert!(r.effects.monochrome);
    assert_ne!(r.effects.gray_mix, [0.; 8]);
    let (steps, applied) = editor.document.history.steps();
    assert_eq!(
        (applied, steps[0].name.as_str()),
        (1, "Convert to Black & White")
    );
    // An edit in the frame the photo decodes in drops the request too.
    let (mut editor, image) =
        editor_with_blue_photo(&ctx, crate::storage::Session::default(), false);
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    editor.document.set_image(image);
    in_edit_frame(&ctx, &mut editor, |e| e.document.recipe.exposure = 0.3);
    assert!(!editor.document.recipe.effects.monochrome);
    in_edit_frame(&ctx, &mut editor, |_| {});
    assert!(!editor.document.recipe.effects.monochrome);
    // A request lapses when the recipe changes otherwise before the photo decodes.
    let (mut editor, image) =
        editor_with_blue_photo(&ctx, crate::storage::Session::default(), false);
    let before_exposure = editor.document.recipe.exposure;
    in_edit_frame(&ctx, &mut editor, Editor::toggle_treatment);
    in_edit_frame(&ctx, &mut editor, |e| e.document.recipe.exposure = 0.5);
    // Undone again before it decodes: the request still lapsed.
    in_edit_frame(&ctx, &mut editor, Editor::undo);
    assert_eq!(editor.document.recipe.exposure, before_exposure);
    editor.document.set_image(image);
    in_edit_frame(&ctx, &mut editor, Editor::finish_pending_treatment);
    assert!(!editor.document.recipe.effects.monochrome);
}

/// A Lightroom preset from its settings, as a file would hold them.
fn preset_from(name: &str, settings: &str) -> crate::xmp::Preset {
    let text = format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:PresetType="Normal" crs:HasSettings="True" {settings}><crs:Name><rdf:Alt><rdf:li xml:lang="x-default">{name}</rdf:li></rdf:Alt></crs:Name></rdf:Description></rdf:RDF></x:xmpmeta>"#
    );
    crate::xmp::parse(std::path::Path::new(&format!("{name}.xmp")), &text).unwrap()
}
fn editor_with_presets(ctx: &egui::Context, presets: Vec<crate::xmp::Preset>) -> Editor {
    let mut editor = Editor::with_context(ctx, None, crate::storage::Session::default(), None);
    editor.document.metadata = Some(Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    });
    editor.presets.library = Arc::new(crate::presets::Library {
        presets,
        errors: Vec::new(),
    });
    editor
}

#[test]
fn preset_amount_scales_the_preset_from_the_settings_before_it() {
    let ctx = egui::Context::default();
    let mut editor = editor_with_presets(
        &ctx,
        vec![
            preset_from(
                "Bright",
                r#"crs:SupportsAmount="True" crs:Exposure2012="+1.00" crs:Contrast2012="+40""#,
            ),
            preset_from(
                "Fixed",
                r#"crs:SupportsAmount="False" crs:Exposure2012="+1.00""#,
            ),
        ],
    );
    editor.document.recipe.exposure = 0.2;
    let frame = editor.begin_edit_frame();
    editor.apply_preset(0);
    editor.finish_edit_frame(frame, &ctx);
    assert_eq!(editor.document.recipe.exposure, 1.);
    // Each drag is one History step, computed again from the settings before the
    // preset, so going back and forth never drifts.
    for amount in [1.7, 0.3, 0.5] {
        let frame = editor.begin_edit_frame();
        editor.set_preset_amount(amount);
        editor.finish_edit_frame(frame, &ctx);
    }
    assert!((editor.document.recipe.exposure - 0.6).abs() < 1e-6);
    assert!((editor.document.recipe.contrast - 0.2).abs() < 1e-6);
    let (steps, applied) = editor.document.history.steps();
    assert_eq!(applied, 4);
    assert_eq!(steps[3].name, "Preset Amount");
    assert_eq!(steps[3].value, "50");
    // Any other edit ends it, as Lightroom hides the slider, even with the Presets
    // panel closed.
    let frame = editor.begin_edit_frame();
    editor.document.recipe.vibrance = 0.1;
    editor.finish_edit_frame(frame, &ctx);
    assert!(editor.presets.amount.is_none());
    // A preset without an Amount shows none.
    editor.apply_preset(1);
    assert!(editor.presets.amount.is_none());
    editor.apply_preset(0);
    assert!(editor.presets.amount.is_some());
    // A preset with only choices that aren't numbers looks the same at every Amount
    // above 0: moving it is no step, and leaves no name for the next one.
    editor.presets.library = Arc::new(crate::presets::Library {
        presets: vec![preset_from(
            "Mono",
            r#"crs:SupportsAmount="True" crs:ConvertToGrayscale="True""#,
        )],
        errors: Vec::new(),
    });
    let frame = editor.begin_edit_frame();
    editor.apply_preset(0);
    editor.finish_edit_frame(frame, &ctx);
    let frame = editor.begin_edit_frame();
    // As the slider does while it moves.
    ctx.data_mut(|d| {
        d.insert_temp(
            super::widgets::history_step_id(),
            ("Preset Amount".to_string(), "50".to_string()),
        )
    });
    editor.set_preset_amount(0.5);
    editor.finish_edit_frame(frame, &ctx);
    let frame = editor.begin_edit_frame();
    editor.document.recipe.exposure = 0.9;
    editor.finish_edit_frame(frame, &ctx);
    assert!(editor.presets.amount.is_none());
    editor.apply_preset(0);
    let (steps, applied) = editor.document.history.steps();
    assert_ne!(steps[applied - 1].name, "Preset Amount");
    // Undo ends it too.
    let frame = editor.begin_edit_frame();
    let mut recipe = editor.document.recipe.clone();
    editor.document.history.undo(&mut recipe);
    editor.document.recipe = recipe;
    editor.finish_edit_frame(frame, &ctx);
    assert!(editor.presets.amount.is_none());
}
