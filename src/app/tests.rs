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
        assert!(!e.view.zoom100);
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
        assert_eq!(e.view.zoom100, expected);
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
    e.view.zoom100 = true;
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
            editor.view.zoom100,
            editor.view.pan,
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
        histogram: Box::new([[0; 256]; 3]),
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
                histogram: Box::new([[0; 256]; 3]),
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
            editor.view.zoom100,
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
    assert!(!editor.view.zoom100);
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
    // Auto sets tone only; white balance is the WB menu's Auto.
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
    editor.copy_settings();
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
    editor.auto_ready(worker::AutoKind::Settings, Ok(Box::new(auto)));
    assert!(editor.auto_in_effect());
    // Any change, to a slider Auto sets or to what it measured, turns it back on, and
    // so does undoing Auto.
    editor.document.recipe.exposure = 0.5;
    assert!(!editor.auto_in_effect());
    editor.document.recipe.exposure = 1.;
    assert!(editor.auto_in_effect());
    editor.document.recipe.crop[0] = 0.1;
    assert!(!editor.auto_in_effect());
    editor.document.recipe.crop[0] = 0.;
    assert!(editor.auto_in_effect());
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
    // Pasted onto another photo, the mode comes along but not the corrections.
    e.document.recipe.upright.mode = UprightMode::Vertical;
    e.copy_settings();
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
    editor.library.as_mut().unwrap().discard_copy_name();
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
            events: vec![egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers,
            }],
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
