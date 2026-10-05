use super::*;
use std::sync::Arc;
fn editor() -> (Editor, egui::Context) {
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::storage::Session::default(), None);
    e.onboarding.visible = false;
    e.library_mode = false;
    let metadata = crate::raw::Metadata {
        width: 12,
        height: 8,
        wb: [1.; 3],
        ..Default::default()
    };
    e.document.metadata = Some(metadata.clone());
    e.document.set_image(Arc::new(crate::raw::CameraImage {
        recovered: Default::default(),
        width: 12,
        height: 8,
        pixels: vec![[0.2, 0.1, 0.05]; 96],
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
    }));
    (e, ctx)
}
fn set(e: &mut Editor, ctx: &egui::Context, value: f32) -> Result<Value> {
    e.execute_command(Command::new(Operation::Set(Param::Exposure, value)), ctx)
}
#[test]
fn command_edit_records_history_and_undo_returns_post_action_state() {
    let (mut e, ctx) = editor();
    let initial = e.document.recipe.exposure;
    set(&mut e, &ctx, 1.25).unwrap();
    assert_eq!(e.command_state()["values"]["exposure"], 1.25);
    assert!(e.document.save.needs_save());
    assert!(e.automation.revision > 0);
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.recipe.exposure, initial);
    e.execute_command(Command::new(Operation::Action(Action::Redo)), &ctx)
        .unwrap();
    assert_eq!(e.document.recipe.exposure, 1.25);
}
#[test]
fn edits_reject_library_loading_modal_and_stale_targets() {
    let (mut e, ctx) = editor();
    let original = e.document.recipe.clone();
    e.library_mode = true;
    assert_eq!(set(&mut e, &ctx, 1.).unwrap_err().code, "no_document");
    e.library_mode = false;
    e.preferences.open = true;
    assert_eq!(set(&mut e, &ctx, 1.).unwrap_err().code, "busy");
    e.preferences.open = false;
    let (generation, _) = e.load.start();
    assert_eq!(set(&mut e, &ctx, 1.).unwrap_err().code, "not_ready");
    e.load.finish(generation);
    let mut command = Command::new(Operation::Set(Param::Exposure, 1.));
    command.target.generation = Some(generation + 1);
    assert_eq!(
        e.execute_command(command, &ctx).unwrap_err().code,
        "stale_target"
    );
    assert_eq!(e.document.recipe, original);
}
#[test]
fn explicit_mask_has_local_units_and_rejects_stale_index() {
    let (mut e, ctx) = editor();
    e.document.recipe.masks.push(Default::default());
    let target = Target {
        mask: Some(0),
        generation: Some(e.load.id()),
        revision: Some(e.automation.revision),
        ..Default::default()
    };
    let command = Command {
        operation: Operation::Set(Param::Temperature, 35.),
        target: target.clone(),
    };
    let global = e.document.recipe.temperature;
    e.execute_command(command.clone(), &ctx).unwrap();
    assert_eq!(e.document.recipe.temperature, global);
    assert_eq!(e.document.recipe.masks[0].adjust.temperature, 0.35);
    assert_eq!(
        e.execute_command(command, &ctx).unwrap_err().code,
        "stale_target"
    );
    let target = Target {
        revision: Some(e.automation.revision),
        ..target
    };
    let before = e.document.recipe.clone();
    assert_eq!(
        e.execute_command(
            Command {
                operation: Operation::Set(Param::Vibrance, 20.),
                target
            },
            &ctx
        )
        .unwrap_err()
        .code,
        "unsupported_parameter"
    );
    assert_eq!(e.document.recipe, before);
}
#[test]
fn black_white_action_matches_existing_treatment_workflow() {
    let (mut e, ctx) = editor();
    let (mut reference, _) = editor();
    reference.toggle_treatment();
    e.execute_command(Command::new(Operation::Action(Action::ToggleMono)), &ctx)
        .unwrap();
    assert_eq!(e.document.recipe, reference.document.recipe);
}
#[test]
fn named_actions_are_discoverable_and_parameter_values_are_finite() {
    for name in Action::NAMES {
        assert!(Action::parse(name).is_some(), "{name}");
    }
    let (mut e, ctx) = editor();
    assert_eq!(
        set(&mut e, &ctx, f32::NAN).unwrap_err().code,
        "invalid_value"
    );
    let capabilities = e
        .execute_command(Command::new(Operation::Capabilities), &ctx)
        .unwrap();
    assert_eq!(capabilities["protocol"], PROTOCOL);
    assert!(
        capabilities["parameters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"] == "temperature"
                && v["unit"] == "kelvin"
                && v["mask_unit"] == "percent")
    );
}
#[test]
fn refused_open_does_not_claim_another_photo_opened() -> anyhow::Result<()> {
    let (mut e, ctx) = editor();
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let missing = photos.join("missing.DNG");
    std::fs::write(&missing, b"fixture")?;
    let db = dir.path().join("catalog.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&db)?;
    catalog.add_folder(&photos)?;
    drop(catalog);
    e.library = Some(Box::new(super::super::library::Library::load(
        &db,
        ctx.clone(),
    )?));
    let id = e.library.as_ref().unwrap().photos[0].id;
    std::fs::remove_file(missing)?;
    e.document.catalog_photo = Some(999);
    let error = e
        .execute_command(Command::new(Operation::Open(PhotoTarget::Id(id))), &ctx)
        .unwrap_err();
    assert_eq!(error.code, "not_editable");
    assert_eq!(e.document.catalog_photo, Some(999));
    assert!(!e.library_mode);
    Ok(())
}
#[test]
fn output_job_publishes_the_captured_revision_and_never_clobbers() -> anyhow::Result<()> {
    let (mut e, ctx) = editor();
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("synthetic.dng");
    // The decoded synthetic pixels are supplied directly; no private RAW fixture.
    std::fs::write(&source, b"synthetic")?;
    e.document.path = Some(source);
    let path = dir.path().join("preview.jpg");
    let result = e
        .execute_command(
            Command::new(Operation::Output {
                path: path.clone(),
                max_edge: 16,
            }),
            &ctx,
        )
        .unwrap();
    let id = result["job_id"].as_u64().unwrap();
    let revision = result["revision"].clone();
    set(&mut e, &ctx, 1.).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let job = loop {
        let job = e
            .execute_command(Command::new(Operation::Job(id)), &ctx)
            .unwrap();
        if job["status"] != "running" {
            break job;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "output never completed"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert_eq!(job["status"], "completed", "{job}");
    assert_eq!(job["revision"], revision);
    assert!(path.is_file());
    assert_eq!(
        e.execute_command(Command::new(Operation::Output { path, max_edge: 16 }), &ctx)
            .unwrap_err()
            .code,
        "already_exists"
    );
    Ok(())
}

#[test]
fn save_rejects_protected_and_uncataloged_edits() {
    let (mut e, ctx) = editor();
    set(&mut e, &ctx, 1.).unwrap();
    assert_eq!(
        e.execute_command(Command::new(Operation::Save), &ctx)
            .unwrap_err()
            .code,
        "no_catalog"
    );
    assert!(e.document.save.needs_save());
    e.document
        .save
        .protect("Conflicting source identity".into());
    assert_eq!(
        e.execute_command(Command::new(Operation::Save), &ctx)
            .unwrap_err()
            .code,
        "protected"
    );
    assert!(e.document.save.is_protected());
}

#[test]
fn save_success_means_the_catalog_contains_the_current_edit() -> anyhow::Result<()> {
    let (mut e, ctx) = editor();
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let source = photos.join("synthetic.DNG");
    std::fs::write(&source, b"fixture")?;
    let db = dir.path().join("catalog.rawmakase");
    let mut catalog = crate::catalog::Catalog::create(&db)?;
    catalog.add_folder(&photos)?;
    drop(catalog);
    e.library = Some(Box::new(super::super::library::Library::load(
        &db,
        ctx.clone(),
    )?));
    let id = e.library.as_ref().unwrap().photos[0].id;
    e.document.catalog_photo = Some(id);
    e.document.path = Some(source.clone());
    set(&mut e, &ctx, 1.25).unwrap();
    let result = e
        .execute_command(Command::new(Operation::Save), &ctx)
        .unwrap();
    assert_eq!(result["saved"], true);
    assert!(!e.document.save.needs_save());
    assert_eq!(
        e.library
            .as_ref()
            .unwrap()
            .catalog
            .load_edit(id, &source)?
            .unwrap()
            .recipe
            .exposure,
        1.25
    );
    Ok(())
}

#[test]
fn curve_save_is_modal_for_commands_and_state() {
    let (mut e, ctx) = editor();
    e.choose_point_curve(super::super::curve_menu::CurveChoice::Save);
    assert_eq!(e.command_state()["modal"], true);
    assert_eq!(set(&mut e, &ctx, 1.).unwrap_err().code, "busy");
    assert_eq!(
        e.execute_command(Command::new(Operation::Action(Action::Reset)), &ctx)
            .unwrap_err()
            .code,
        "busy"
    );
    assert!(
        e.execute_command(Command::new(Operation::State), &ctx)
            .is_ok()
    );
}

#[test]
fn asynchronous_edits_and_out_of_frame_undo_invalidate_guards() {
    let (mut e, ctx) = editor();
    let initial = e.command_state()["revision"].as_u64().unwrap();
    let mut auto = e.document.recipe.clone();
    auto.exposure = 1.25;
    e.auto_ready(super::super::worker::AutoKind::Settings, Ok(Box::new(auto)));
    let mut command = Command::new(Operation::Set(Param::Contrast, 10.));
    command.target.revision = Some(initial);
    assert_eq!(
        e.execute_command(command, &ctx).unwrap_err().code,
        "stale_target"
    );
    let after_auto = e.command_state()["revision"].as_u64().unwrap();
    assert!(after_auto > initial);
    e.undo();
    let after_undo = e.command_state()["revision"].as_u64().unwrap();
    assert!(after_undo > after_auto);
    // Direct worker-style recipe changes must also be caught before a frame snapshot.
    e.document.recipe.straighten = 1.;
    let frame = e.begin_edit_frame();
    e.finish_edit_frame(frame, &ctx);
    assert!(e.command_state()["revision"].as_u64().unwrap() > after_undo);
}

#[test]
fn no_op_parameters_do_not_name_the_next_unrelated_edit() {
    let (mut e, ctx) = editor();
    let exposure = e.document.recipe.exposure;
    set(&mut e, &ctx, exposure).unwrap();
    let old = e.document.recipe.clone();
    e.document.recipe.preset_name = "Example".into();
    e.history(old);
    assert_eq!(e.document.history.steps().0.last().unwrap().name, "Preset");
    set(&mut e, &ctx, 100.).unwrap();
    e.execute_command(Command::new(Operation::Adjust(Param::Exposure, 1)), &ctx)
        .unwrap();
    e.surface.end_turn();
    e.finish_gesture();
    let old = e.document.recipe.clone();
    e.document.recipe.preset_name = "Another".into();
    e.history(old);
    assert_eq!(e.document.history.steps().0.last().unwrap().name, "Preset");
}

#[test]
fn turns_group_only_the_same_parameter_and_scope() {
    let (mut e, ctx) = editor();
    for _ in 0..2 {
        e.execute_command(Command::new(Operation::Adjust(Param::Exposure, 1)), &ctx)
            .unwrap();
    }
    let exposure = e.document.recipe.exposure;
    e.execute_command(Command::new(Operation::Adjust(Param::Contrast, 1)), &ctx)
        .unwrap();
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.recipe.contrast, 0.);
    assert_eq!(e.document.recipe.exposure, exposure);
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.recipe.exposure, 0.);
    e.document.recipe.masks.push(Default::default());
    e.command_state();
    e.execute_command(Command::new(Operation::Adjust(Param::Exposure, 1)), &ctx)
        .unwrap();
    let global = e.document.recipe.exposure;
    let mut command = Command::new(Operation::Adjust(Param::Exposure, 1));
    command.target = Target {
        mask: Some(0),
        generation: Some(e.load.id()),
        revision: Some(e.automation.revision),
        ..Default::default()
    };
    e.execute_command(command, &ctx).unwrap();
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.recipe.masks[0].adjust.exposure, 0.);
    assert_eq!(e.document.recipe.exposure, global);
}

#[test]
fn point_curve_edits_validate_preserve_channels_and_undo() {
    let (mut e, ctx) = editor();
    let original = e.document.recipe.clone();
    let points = vec![[0., 0.], [0.25, 0.2], [0.75, 0.8], [1., 1.]];
    e.execute_command(
        Command::new(Operation::Curve(CurveChannel::Red, points.clone())),
        &ctx,
    )
    .unwrap();
    assert_eq!(e.document.recipe.effects.channels[0].points, points);
    assert_eq!(e.document.recipe.curve, original.curve);
    assert_eq!(e.command_state()["tone_curve"]["red"]["points"][1][0], 0.25);
    let before = e.document.recipe.clone();
    for points in [
        vec![[0., 0.]],
        vec![[0.5, 0.], [0.5, 1.]],
        vec![[0., -1.], [1., 1.]],
        vec![[0., f32::NAN], [1., 1.]],
    ] {
        assert_eq!(
            e.execute_command(
                Command::new(Operation::Curve(CurveChannel::Rgb, points)),
                &ctx
            )
            .unwrap_err()
            .code,
            "invalid_curve"
        );
        assert_eq!(e.document.recipe, before);
    }
    e.execute_command(Command::new(Operation::Action(Action::Undo)), &ctx)
        .unwrap();
    assert_eq!(e.document.recipe, original);
    e.execute_command(Command::new(Operation::Action(Action::CurveMedium)), &ctx)
        .unwrap();
    assert_eq!(
        e.document.recipe.curve,
        crate::presets::curves::BuiltinCurve::MediumContrast.curve()
    );
    assert_eq!(
        e.document.recipe.effects.channels,
        original.effects.channels
    );
}
