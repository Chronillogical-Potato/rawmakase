use super::*;
use crate::app::Editor;
use crate::camera_data::{CameraImage, Metadata};
use crate::storage::bitmaps::Bitmap;
use crate::storage::mask_assets;
use eframe::egui;
use std::sync::Arc;

/// An editor with a decoded photo in a catalog, as Develop has one, and a raster of
/// the kind a selection would register (distinct per `seed`).
struct Fixture {
    _dir: tempfile::TempDir,
    editor: Editor,
    photo: std::path::PathBuf,
}
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos).unwrap();
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"identity fixture").unwrap();
    let catalog = dir.path().join("test.rawmakase");
    let mut c = crate::catalog::Catalog::create(&catalog).unwrap();
    c.add_folder(&photos).unwrap();
    drop(c);
    let ctx = egui::Context::default();
    let library = crate::app::library::Library::load(&catalog, ctx.clone()).unwrap();
    let id = library.session.photos[0].id;
    let mut editor =
        Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    editor.library = Some(Box::new(library));
    editor.document.catalog_photo = Some(id);
    editor.document.path = Some(photo.clone());
    let metadata = Metadata {
        width: 8,
        height: 6,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    editor.document.set_image(Arc::new(CameraImage {
        recovered: Default::default(),
        width: 8,
        height: 6,
        pixels: vec![[0.2; 3]; 48],
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    Fixture {
        _dir: dir,
        editor,
        photo,
    }
}
fn generated(seed: u8) -> Generated {
    let id = mask_assets::register(Bitmap {
        width: 4,
        height: 3,
        channels: 1,
        depth: 1,
        data: (0..12).map(|i| seed.wrapping_add(i)).collect(),
    })
    .unwrap();
    Generated {
        id,
        width: 4,
        height: 3,
        source: BitmapSource {
            feature: FEATURE_SUBJECT.into(),
            model: "test@0".into(),
            input: format!("{seed}"),
        },
    }
}
/// Starts a selection the way the worker would be asked to, without a worker.
fn begin(editor: &mut Editor, request: Request) -> u64 {
    let (generation, _) = editor.selection.task.start();
    let guard = Guard::of(editor.document.edit.recipe(), request);
    editor.selection.pending = Some(Pending { request, guard });
    generation
}
fn finish(editor: &mut Editor, generation: u64, result: Result<Generated, Failure>) {
    let load = editor.load.id();
    editor.selection_done(Done {
        load,
        generation,
        result,
    });
}
const SUBJECT: Request = Request {
    feature: Feature::Subject,
    target: Target::NewMask,
};

#[test]
fn a_selection_becomes_one_named_mask_and_one_history_step() {
    let mut f = fixture();
    let e = &mut f.editor;
    let g = begin(e, SUBJECT);
    finish(e, g, Ok(generated(10)));
    let masks = &e.document.edit.recipe().masks;
    assert_eq!(masks.len(), 1);
    assert_eq!(masks[0].name, "Subject");
    assert!(masks[0].adjust.is_neutral());
    let MaskShape::Bitmap(b) = &masks[0].components[0].shape else {
        panic!("not a raster")
    };
    assert_eq!((b.width, b.height, b.sampling), (4, 3, BITMAP_SAMPLING));
    assert!(!masks[0].components[0].invert);
    assert_eq!(e.document.edit.history().steps().1, 1);
    assert_eq!(
        e.document.edit.history().steps().0[0].name,
        "Select Subject"
    );
    assert_eq!(e.view.masking.selected, Some(0));
    assert!(e.view.masking.overlay);
    assert!(e.document.edit.save_state().needs_save());
    assert!(e.selection.running().is_none());
    // Undo removes it in one step.
    assert!(e.document.edit.undo());
    assert!(e.document.edit.recipe().masks.is_empty());
}

#[test]
fn background_is_an_inverted_component_not_an_inverted_mask() {
    let mut f = fixture();
    let e = &mut f.editor;
    let request = Request {
        feature: Feature::Background,
        target: Target::NewMask,
    };
    let g = begin(e, request);
    finish(e, g, Ok(generated(20)));
    let mask = &e.document.edit.recipe().masks[0];
    assert_eq!(mask.name, "Background");
    assert!(mask.components[0].invert && !mask.invert);
    assert_eq!(
        e.document.edit.history().steps().0[0].name,
        "Select Background"
    );
}

#[test]
fn a_selection_can_join_a_mask_as_a_component() {
    let mut f = fixture();
    let e = &mut f.editor;
    let g = begin(e, SUBJECT);
    finish(e, g, Ok(generated(30)));
    let request = Request {
        feature: Feature::Background,
        target: Target::Component {
            mask: 0,
            op: MaskOp::Subtract,
        },
    };
    let g = begin(e, request);
    finish(e, g, Ok(generated(31)));
    let mask = &e.document.edit.recipe().masks[0];
    assert_eq!(mask.components.len(), 2);
    assert_eq!(mask.components[1].op, MaskOp::Subtract);
    assert!(mask.components[1].invert);
    assert_eq!(e.view.masking.component, Some(1));
}

#[test]
fn failures_and_cancellation_record_no_edit() {
    let mut f = fixture();
    let e = &mut f.editor;
    let g = begin(e, SUBJECT);
    finish(e, g, Err(Failure::NoSubject));
    assert!(e.document.edit.recipe().masks.is_empty());
    assert_eq!(e.document.edit.history().steps().1, 0);
    assert!(!e.document.edit.save_state().needs_save());
    assert_eq!(
        e.selection.failure.as_ref().map(|f| &f.1),
        Some(&Failure::NoSubject)
    );
    assert_eq!(
        e.status,
        "Nothing selected there; click on the subject itself"
    );
    assert!(e.selection.running().is_none());

    e.selection.failure = None;
    let g = begin(e, SUBJECT);
    finish(e, g, Err(Failure::Cancelled));
    assert!(e.selection.failure.is_none() && e.selection.running().is_none());
    assert!(e.document.edit.recipe().masks.is_empty());
}

#[test]
fn an_obsolete_result_is_dropped_and_cannot_finish_a_newer_request() {
    let mut f = fixture();
    let e = &mut f.editor;
    let old = begin(e, SUBJECT);
    let new = begin(e, SUBJECT);
    assert_ne!(old, new);
    // The first one's success and failure both arrive late.
    finish(e, old, Ok(generated(40)));
    finish(e, old, Err(Failure::Failed("late".into())));
    assert!(e.document.edit.recipe().masks.is_empty());
    assert!(
        e.selection.running().is_some(),
        "the newer request is still wanted"
    );
    assert!(e.selection.failure.is_none());
    finish(e, new, Ok(generated(41)));
    assert_eq!(e.document.edit.recipe().masks.len(), 1);

    // A result for another photo is dropped.
    let g = begin(e, SUBJECT);
    let wrong_load = e.load.id() + 5;
    e.selection_done(Done {
        load: wrong_load,
        generation: g,
        result: Ok(generated(42)),
    });
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    // Cancelling stops it being applied even if the worker had finished.
    e.selection.cancel();
    finish(e, g, Ok(generated(43)));
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
}

#[test]
fn a_result_is_not_applied_over_masks_that_changed_meanwhile() {
    let mut f = fixture();
    let e = &mut f.editor;
    let g = begin(e, SUBJECT);
    // The user adds a mask while the model runs.
    e.change_edit(None, |r| {
        r.masks.push(MaskGroup {
            components: vec![MaskComponent::new(MaskShape::Linear {
                from: [0.; 2],
                to: [1.; 2],
            })],
            ..Default::default()
        })
    });
    finish(e, g, Ok(generated(50)));
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    assert!(matches!(
        e.document.edit.recipe().masks[0].components[0].shape,
        MaskShape::Linear { .. }
    ));
    assert!(e.status.contains("changed"));

    // Sliders, names and visibility are not structure: they survive an accepted result.
    let g = begin(e, SUBJECT);
    e.change_edit(None, |r| {
        r.exposure = 1.;
        r.masks[0].adjust.exposure = 0.5;
        r.masks[0].name = "Sky".into();
        r.masks[0].hidden = true;
    });
    finish(e, g, Ok(generated(51)));
    let r = e.document.edit.recipe();
    assert_eq!(r.masks.len(), 2);
    assert_eq!(
        (
            r.exposure,
            r.masks[0].adjust.exposure,
            r.masks[0].name.as_str()
        ),
        (1., 0.5, "Sky")
    );
}

#[test]
fn spots_changed_while_selecting_invalidate_the_result() {
    let mut f = fixture();
    let e = &mut f.editor;
    let g = begin(e, SUBJECT);
    e.change_edit(None, |r| {
        r.retouch.push(crate::model::retouch::RetouchOp {
            mode: Default::default(),
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.05,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
        });
    });
    finish(e, g, Ok(generated(60)));
    assert!(e.document.edit.recipe().masks.is_empty());
    // Changed and changed back is a different input, not the same one.
    let g = begin(e, SUBJECT);
    let before = e.document.edit.recipe().retouch.clone();
    e.change_edit(None, |r| r.retouch.clear());
    e.change_edit(None, |r| r.retouch = before);
    finish(e, g, Ok(generated(61)));
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
}

#[test]
fn regeneration_replaces_only_the_raster() {
    let mut f = fixture();
    let e = &mut f.editor;
    let g = begin(e, SUBJECT);
    finish(e, g, Ok(generated(70)));
    e.change_edit(None, |r| {
        r.masks[0].adjust.exposure = 0.7;
        r.masks[0].invert = true;
        r.masks[0].components[0].opacity = 0.5;
        r.masks[0].components[0].op = MaskOp::Intersect;
    });
    let old = e.document.edit.recipe().masks[0].components[0]
        .shape
        .clone();
    let regenerate = Request {
        feature: Feature::Subject,
        target: Target::Regenerate {
            mask: 0,
            component: 0,
        },
    };
    let g = begin(e, regenerate);
    finish(e, g, Ok(generated(71)));
    let m = &e.document.edit.recipe().masks[0];
    assert_ne!(m.components[0].shape, old);
    assert_eq!(
        (
            m.adjust.exposure,
            m.invert,
            m.components[0].opacity,
            m.components[0].op
        ),
        (0.7, true, 0.5, MaskOp::Intersect)
    );
    assert_eq!(
        e.document.edit.history().steps().0.last().unwrap().name,
        "Refine Subject"
    );

    // A failed regeneration leaves the old result in place.
    let before = e.document.edit.recipe().clone();
    let g = begin(e, regenerate);
    finish(e, g, Err(Failure::Failed("boom".into())));
    assert_eq!(*e.document.edit.recipe(), before);

    // A correction to the shape while it runs cancels the result.
    let g = begin(e, regenerate);
    e.change_edit(None, |r| {
        r.masks[0].components[0].shape = MaskShape::Linear {
            from: [0.; 2],
            to: [1.; 2],
        }
    });
    let edited = e.document.edit.recipe().clone();
    finish(e, g, Ok(generated(72)));
    assert_eq!(*e.document.edit.recipe(), edited);
}

#[test]
fn a_new_photo_forgets_the_selection_that_was_running() {
    let mut f = fixture();
    let e = &mut f.editor;
    let g = begin(e, SUBJECT);
    e.selection.clear_document();
    assert!(e.selection.running().is_none());
    finish(e, g, Ok(generated(80)));
    assert!(e.document.edit.recipe().masks.is_empty());
}

#[test]
fn a_selection_needs_a_catalog_an_upgraded_one_and_the_model() {
    let mut f = fixture();
    let e = &mut f.editor;
    // The catalog is the first format: ask to upgrade before anything else.
    e.library
        .as_ref()
        .unwrap()
        .session
        .catalog
        .db_for_tests()
        .execute_batch("PRAGMA user_version=1")
        .unwrap();
    e.request_selection(SUBJECT);
    assert_eq!(e.selection.prompt, Some(Prompt::Upgrade(SUBJECT)));
    assert!(e.selection.running().is_none());
    // Upgraded, the model is the next thing asked for (no download starts).
    e.library
        .as_mut()
        .unwrap()
        .session
        .catalog
        .upgrade_for_raster_masks()
        .unwrap();
    e.selection.prompt = None;
    if e.selection.models.installed() {
        return; // a developer machine with the model installed
    }
    e.request_selection(SUBJECT);
    assert_eq!(e.selection.prompt, Some(Prompt::Model(SUBJECT)));
    assert!(!e.selection.models.busy());
    // Without a catalog nothing is offered at all.
    e.library = None;
    e.selection.prompt = None;
    e.request_selection(SUBJECT);
    assert!(e.selection.prompt.is_none());
    assert_eq!(e.status, "Add this photo to a catalog to use AI masks");
}

#[test]
fn a_selected_mask_survives_saving_and_reopening_without_the_store() {
    let mut f = fixture();
    let id;
    {
        let e = &mut f.editor;
        let g = begin(e, SUBJECT);
        let made = generated(90);
        id = made.id.clone();
        finish(e, g, Ok(made));
        assert!(e.flush());
        assert!(!e.document.edit.save_state().needs_save());
    }
    // The raster is in the catalog now; the process forgets it and reads it back.
    let catalog = &f.editor.library.as_ref().unwrap().session.catalog;
    let loaded = catalog
        .load_edit(f.editor.document.catalog_photo.unwrap(), &f.photo)
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.recipe.mask_asset_ids().collect::<Vec<_>>(),
        [id.as_str()]
    );
    assert!(mask_assets::unsaved([id.as_str()]).is_empty());
    let read = catalog.mask_asset_loader().load(&id).unwrap().unwrap();
    assert_eq!(read.data, (0..12).map(|i| 90 + i).collect::<Vec<u8>>());
}

fn draw_masking_panel(e: &mut Editor) {
    let ctx = e.context.clone();
    let frame = e.begin_edit_frame();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400., 900.),
            )),
            ..Default::default()
        },
        |ui| e.mask_panel(ui),
    );
    output.textures_delta.clear();
    e.finish_edit_frame(frame, &ctx);
}

#[test]
fn the_drawer_draws_in_every_state_a_selection_can_be_in() {
    let mut f = fixture();
    let e = &mut f.editor;
    draw_masking_panel(e);
    let g = begin(e, SUBJECT);
    draw_masking_panel(e); // running
    finish(e, g, Ok(generated(100)));
    draw_masking_panel(e); // with a generated component selected
    e.selection.failure = Some((SUBJECT, Failure::NoSubject));
    draw_masking_panel(e);
    e.selection.failure = None;
    e.selection.prompting = Some(Prompting {
        request: SUBJECT,
        points: vec![Point {
            x: 0.5,
            y: 0.5,
            positive: true,
        }],
        bounds: Some([0.1, 0.1, 0.9, 0.9]),
        applied: None,
    });
    draw_masking_panel(e);
    e.selection.prompting = None;
    e.selection.prompt = Some(Prompt::Model(SUBJECT));
    draw_masking_panel(e);
    e.selection.prompt = Some(Prompt::Upgrade(SUBJECT));
    draw_masking_panel(e);
    e.library = None;
    draw_masking_panel(e); // disabled with its reason
}

#[test]
#[ignore = "needs RAWMAKASE_TEST_MODEL, RAWMAKASE_ORT_LIB and RAWMAKASE_TEST_RAW"]
fn the_real_model_selects_through_the_worker_and_the_mask_changes_the_render() {
    let model = std::path::PathBuf::from(std::env::var("RAWMAKASE_TEST_MODEL").unwrap());
    let raw = std::path::PathBuf::from(std::env::var("RAWMAKASE_TEST_RAW").unwrap());
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let image = Arc::new(
        crate::photo::open(&raw)
            .unwrap()
            .develop(
                crate::camera_data::Decode::full(Default::default()),
                &cancel,
            )
            .unwrap(),
    );
    let mut f = fixture();
    let e = &mut f.editor;
    e.document.set_image(image.clone());
    let (generation, cancel) = e.selection.task.start();
    e.selection.pending = Some(Pending {
        request: SUBJECT,
        guard: Guard::of(e.document.edit.recipe(), SUBJECT),
    });
    e.selection.worker.submit(
        worker::Job {
            load: e.load.id(),
            generation,
            cancel,
            image: image.clone(),
            recipe: e.document.edit.recipe().clone(),
            model,
            prompt: rawmakase_inference::Prompt {
                points: vec![rawmakase_inference::Point {
                    x: 0.5,
                    y: 0.62,
                    positive: true,
                }],
                bounds: None,
            },
        },
        e.tx.clone(),
        e.context.clone(),
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    loop {
        match e.rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(crate::app::worker::Event::Selection(done)) => {
                e.selection_done(*done);
                break;
            }
            _ => assert!(std::time::Instant::now() < deadline, "no result"),
        }
    }
    assert_eq!(e.document.edit.recipe().masks.len(), 1, "{}", e.status);
    // The mask selects part of the photo: lift it and compare the two regions.
    let mut recipe = e.document.edit.recipe().clone();
    recipe.masks[0].adjust.exposure = 2.;
    let base = crate::develop::render(&image, &Recipe::default().checked().unwrap(), 400).unwrap();
    let lit = crate::develop::render(&image, &recipe.checked().unwrap(), 400).unwrap();
    let mean = |r: &crate::rendered::Rendered| {
        r.pixels.iter().map(|p| p[1]).sum::<f32>() / r.pixels.len() as f32
    };
    assert!(
        mean(&lit) > mean(&base) * 1.02,
        "{} {}",
        mean(&lit),
        mean(&base)
    );
    let changed = base
        .pixels
        .iter()
        .zip(&lit.pixels)
        .filter(|(a, b)| (a[1] - b[1]).abs() > 0.02)
        .count();
    let share = changed as f32 / base.pixels.len() as f32;
    assert!((0.01..0.9).contains(&share), "mask covers {share}");
    // And it survives a save.
    assert!(e.flush());
}

#[test]
fn aiming_starts_with_nothing_and_each_click_refines_the_same_mask() {
    let mut f = fixture();
    let e = &mut f.editor;
    e.selection.prompting = Some(Prompting {
        request: SUBJECT,
        points: Vec::new(),
        bounds: None,
        applied: None,
    });
    // Outside the photo, and a leave-out click before anything is selected, do nothing.
    e.prompt_click([1.5, 0.5], true);
    e.prompt_click([0.5, 0.5], false);
    assert!(e.selection.running().is_none());
    assert!(e.selection.prompting.as_ref().unwrap().points.is_empty());
    // A first result makes the mask and remembers it; later ones replace its raster.
    let g = begin(e, SUBJECT);
    finish(e, g, Ok(generated(110)));
    assert_eq!(
        e.selection.prompting.as_ref().unwrap().applied,
        Some((0, 0))
    );
    let request = Request {
        feature: Feature::Subject,
        target: Target::Regenerate {
            mask: 0,
            component: 0,
        },
    };
    let g = begin(e, request);
    finish(e, g, Ok(generated(111)));
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    assert_eq!(
        e.document.edit.history().steps().0.last().unwrap().name,
        "Refine Subject"
    );
    // Done ends aiming and leaves the mask; leaving the photo does too.
    e.selection.end_prompting();
    assert!(e.selection.prompting.is_none() && e.document.edit.recipe().masks.len() == 1);
    // Boxes too small to be a drag are ignored.
    e.selection.prompting = Some(Prompting {
        request: SUBJECT,
        points: Vec::new(),
        bounds: None,
        applied: None,
    });
    e.prompt_box([0.5, 0.5], [0.501, 0.501]);
    assert!(e.selection.prompting.as_ref().unwrap().bounds.is_none());
    e.selection.clear_document();
    assert!(e.selection.prompting.is_none());
}
