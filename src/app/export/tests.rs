use super::*;
use crate::app::Module;
use crate::app::naming::editor as name_editor;
use crate::catalog::PhotoId;
use crate::export_settings::NameContext;
use crate::export_settings::{Destination, Format};
use name_read::{FileFields, Status};

/// An editor on a catalog of two copies of the synthetic chart DNG and a JPEG.
fn editor() -> anyhow::Result<(tempfile::TempDir, Editor, Vec<PhotoId>, egui::Context)> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/charts/synthetic-d65.dng");
    for name in ["a.dng", "b.dng"] {
        std::fs::copy(&chart, photos.join(name))?;
    }
    image::RgbImage::new(8, 8).save(photos.join("c.jpg"))?;
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)?.add_folder(&photos)?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.raw_defaults = Arc::new(crate::raw_defaults::DevelopDefaults::with_presets(
        Default::default(),
        |_| None,
    ));
    let library = crate::app::library::Library::load(&catalog, ctx.clone())?;
    let ids = library.session.photos.iter().map(|p| p.id).collect();
    e.library = Some(Box::new(library));
    e.module = Module::Library;
    Ok((dir, e, ids, ctx))
}
/// Small TIFFs in `folder`.
fn settings(folder: &std::path::Path) -> ExportSettings {
    ExportSettings {
        destination: Destination::Folder,
        folder: Some(folder.to_path_buf()),
        format: Format::Tiff,
        resize: true,
        long_edge: 96,
        ..Default::default()
    }
}
/// Exports `scope` as the dialog's Export button does, short of saving the
/// settings as the last export's.
fn export(e: &mut Editor, scope: Scope, settings: ExportSettings) -> anyhow::Result<()> {
    let batch = e.batch_photos(&scope, false)?;
    e.plan_export(
        Pending {
            batch,
            settings,
            watermark: None,
            left_out: scope.left_out,
            conflicts: Vec::new(),
        },
        None,
    );
    Ok(())
}
/// Takes the editor's events until every export has finished.
fn wait(e: &mut Editor, ctx: &egui::Context) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !e.exports.queued.is_empty() {
        assert!(std::time::Instant::now() < deadline, "export not finished");
        e.events(ctx);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
fn listing(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[test]
fn export_acts_on_the_selection_and_reports_what_it_left_out() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[2]);
    // The second RAW goes offline; the JPEG is exported like the first.
    let offline = library.photo(ids[1]).unwrap().path.clone();
    std::fs::remove_file(&offline)?;
    let scope = e.export_scope().expect("photos to export");
    assert_eq!(
        scope
            .photos
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["a.dng", "c.jpg"]
    );
    assert_eq!(scope.left_out.len(), 1, "{:?}", scope.left_out);
    assert_eq!(scope.left_out[0].0, "b.dng");
    let out = dir.path().join("out");
    export(&mut e, scope.clone(), settings(&out))?;
    wait(&mut e, &ctx);
    assert_eq!(listing(&out), ["a.tif", "c.tif"]);
    assert_eq!(e.status, "Exported 2 of 3 · 1 not exported");
    // Kept in the top bar until read: the offline RAW, and why.
    let summary = e.exports.summary.clone().expect("a report");
    assert_eq!(summary.problems.len(), 1);
    assert!(summary.problems[0].1.contains("can't be exported"));

    // Again into the same folder: Ask asks once for both files.
    export(&mut e, scope, settings(&out))?;
    let pending = e.exports.pending.take().expect("the question");
    assert_eq!(pending.conflicts, [out.join("a.tif"), out.join("c.tif")]);
    e.plan_export(pending, Some(Existing::Unique));
    wait(&mut e, &ctx);
    assert_eq!(listing(&out), ["a-2.tif", "a.tif", "c-2.tif", "c.tif"]);
    // That export went well, but the first one's report is unread: it stays, and
    // the two add up until it is dismissed.
    let summary = e.exports.summary.clone().expect("the unread report");
    // The offline RAW was left out of both.
    assert_eq!(summary.line(), "Exported 4 of 6 · 2 not exported");
    Ok(())
}

#[test]
fn an_open_photo_whose_edit_isnt_final_holds_back_only_its_own_export() -> anyhow::Result<()> {
    let (_dir, mut e, ids, _ctx) = editor()?;
    let path = e
        .library
        .as_ref()
        .unwrap()
        .photo(ids[0])
        .unwrap()
        .path
        .clone();
    // Open in Develop, its Lightroom edit still to be applied.
    e.document.catalog_photo = Some(ids[0]);
    e.document.path = Some(path);
    e.document.metadata = Some(Default::default());
    e.document
        .set_image(Arc::new(crate::camera_data::CameraImage {
            recovered: Default::default(),
            width: 1,
            height: 1,
            pixels: vec![[0.1; 3]],
            metadata: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        }));
    e.document.pending_lightroom = Some("s = {}".into());
    e.module = Module::Develop;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    assert!(e.export_scope().is_none());
    assert!(
        e.status.starts_with("Export waits for the open photo"),
        "{}",
        e.status
    );
    // A Library selection without it goes ahead.
    e.module = Module::Library;
    e.library.as_mut().unwrap().select(Some(ids[1]));
    let scope = e.export_scope().expect("the other photo");
    assert_eq!(scope.photos.len(), 1);
    assert!(!scope.photos[0].open);
    // Once final, the open photo is exported as shown.
    e.document.pending_lightroom = None;
    e.module = Module::Develop;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let scope = e.export_scope().expect("the open photo");
    assert!(scope.photos[0].open);
    let batch = e.batch_photos(&scope, true)?;
    assert!(matches!(&batch[0].edit, Edit::Shown { unsaved: true, .. }));
    // Its file gone offline since it was opened: said before the dialog, as it is
    // decoded again from the file.
    std::fs::remove_file(e.document.path.clone().unwrap())?;
    assert!(e.export_scope().is_none());
    assert!(e.status.contains("can't be exported"), "{}", e.status);
    Ok(())
}

#[test]
fn a_photo_opened_without_a_catalog_is_exported_only_while_its_file_is_there() -> anyhow::Result<()>
{
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("a.dng");
    std::fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus/charts/synthetic-d65.dng"),
        &path,
    )?;
    let ctx = egui::Context::default();
    let mut e = Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
    e.document.path = Some(path.clone());
    e.document.metadata = Some(Default::default());
    e.document
        .set_image(Arc::new(crate::camera_data::CameraImage {
            recovered: Default::default(),
            width: 1,
            height: 1,
            pixels: vec![[0.1; 3]],
            metadata: Default::default(),
            fast: false,
            scale_factor: 1.,
            scale_clipped: 0,
        }));
    let scope = e.export_scope().expect("the open photo");
    assert!(scope.photos[0].open && scope.photos[0].id.is_none());
    std::fs::remove_file(&path)?;
    assert!(e.export_scope().is_none());
    assert!(e.status.contains("Offline"), "{}", e.status);
    Ok(())
}

#[test]
fn show_in_finder_opens_one_window_for_each_folder_exported_to() {
    let exported: Vec<PathBuf> = ["/a/1.jpg", "/a/2.jpg", "/b/3.jpg", "/a/4.jpg"]
        .iter()
        .map(PathBuf::from)
        .collect();
    assert_eq!(
        to_show(&exported),
        [PathBuf::from("/a/1.jpg"), PathBuf::from("/b/3.jpg")]
    );
    // However many folders, a handful of windows.
    let many: Vec<PathBuf> = (0..20)
        .map(|i| PathBuf::from(format!("/{i}/x.jpg")))
        .collect();
    assert_eq!(to_show(&many).len(), 5);
    assert_eq!(folders(&many), 20);
}

/// Choosing what to export reads no file: a JPEG Develop can't edit is chosen,
/// and fails as its export, with the reason.
#[test]
fn a_cmyk_jpeg_is_exported_and_fails_with_why() -> anyhow::Result<()> {
    use image::ImageEncoder;
    let (dir, mut e, ids, ctx) = editor()?;
    let library = e.library.as_mut().unwrap();
    let jpeg = library.photo(ids[2]).unwrap().path.clone();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new(std::fs::File::create(&jpeg)?);
    encoder.set_icc_profile(crate::app::worker::cmyk_profile())?;
    encoder.write_image(&[10; 8 * 8 * 3], 8, 8, image::ExtendedColorType::Rgb8)?;
    library.select(Some(ids[2]));
    let scope = e.export_scope().expect("photos to export");
    assert_eq!(scope.photos.len(), 1);
    assert!(scope.left_out.is_empty(), "{:?}", scope.left_out);
    let out = dir.path().join("out");
    export(&mut e, scope, settings(&out))?;
    wait(&mut e, &ctx);
    assert!(listing(&out).is_empty());
    let summary = e.exports.summary.clone().expect("a report");
    assert_eq!(summary.problems.len(), 1);
    assert_eq!(summary.problems[0].0, "c.jpg");
    assert!(
        summary.problems[0].1.contains("CMYK"),
        "{:?}",
        summary.problems
    );
    Ok(())
}

/// Return is the Export button, as in Lightroom, but not while the Edit
/// Watermark window, opened from the dialog, is on top of it.
#[test]
fn return_exports_from_the_dialog() -> anyhow::Result<()> {
    let (_dir, mut e, ids, ctx) = editor()?;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let return_key = |e: &mut Editor| {
        let key = |pressed| egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        for events in [vec![key(true), key(false)], vec![]] {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    events,
                    ..Default::default()
                },
                |ui| e.export_windows(ui.ctx()),
            );
            output.textures_delta.clear();
        }
    };
    // No folder chosen: what Export says, and nothing is exported or saved.
    let open = |e: &mut Editor| {
        e.exports.scope = e.export_scope().expect("a photo to export");
        e.exports.draft = ExportSettings {
            destination: Destination::Folder,
            folder: None,
            ..Default::default()
        };
        e.exports.dialog = true;
        e.status.clear();
    };
    open(&mut e);
    e.exports.watermark_editor = Some(super::watermark_editor::WatermarkEditor::new(
        Default::default(),
        &ctx,
    ));
    return_key(&mut e);
    assert_eq!(e.status, "");
    assert!(e.exports.dialog);
    e.exports.watermark_editor = None;
    open(&mut e);
    return_key(&mut e);
    assert_eq!(e.status, "Choose a folder to export to");
    assert!(e.exports.dialog);
    Ok(())
}

/// One frame of the export windows, with `events` as input.
fn frame(e: &mut Editor, ctx: &egui::Context, events: Vec<egui::Event>) {
    let mut output = ctx.run_ui(
        egui::RawInput {
            events,
            ..Default::default()
        },
        |ui| e.export_windows(ui.ctx()),
    );
    output.textures_delta.clear();
}
/// `key`, pressed and released, then a frame without input.
fn press(e: &mut Editor, ctx: &egui::Context, key: egui::Key) {
    let event = |pressed| egui::Event::Key {
        key,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(e, ctx, vec![event(true), event(false)]);
    frame(e, ctx, vec![]);
}
/// Return, pressed and released, then a frame without input.
fn press_return(e: &mut Editor, ctx: &egui::Context) {
    press(e, ctx, egui::Key::Enter);
}
/// Takes the editor's events until `done`.
fn until(e: &mut Editor, ctx: &egui::Context, done: impl Fn(&Editor) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !done(e) {
        assert!(std::time::Instant::now() < deadline, "never happened");
        e.events(ctx);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}
/// A reader that names every camera "Sony ILCE-7RM6", once `gate` lets each
/// photo through, counting the photos read.
fn gated_reader() -> (
    name_read::Reader,
    std::sync::mpsc::Sender<()>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let (open, gate) = std::sync::mpsc::channel::<()>();
    let gate = Mutex::new(gate);
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = count.clone();
    let reader: name_read::Reader = Arc::new(move |_photo: &name_read::ToRead| {
        let _ = gate.lock().unwrap().recv();
        counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(FileFields {
            camera: crate::export_settings::CameraNames::Read {
                make: Some("Sony".into()),
                model: Some("ILCE-7RM6".into()),
            },
            ..Default::default()
        })
    });
    (reader, open, count)
}
/// The Export dialog for the selection, with `settings`.
fn open_dialog(e: &mut Editor, settings: ExportSettings) {
    e.exports.read.cancel();
    e.exports.scope = e.export_scope().expect("photos to export");
    e.exports.draft = settings;
    e.exports.dialog = true;
    e.exports.unsaved_settings = true;
}
const MODEL: &str = "{{com.adobe.model}}-{{image_name}}";

/// The Example and the files are named from what the catalog had when Export
/// was chosen, typing still in the Library included; a change to the catalog
/// afterwards (a backfilled capture time, a title) changes neither.
#[test]
fn names_come_from_the_catalog_as_it_was_when_export_was_chosen() -> anyhow::Result<()> {
    let (dir, mut e, ids, _ctx) = editor()?;
    let library = e.library.as_mut().unwrap();
    library
        .session
        .catalog
        .set_text(&ids[..1], crate::metadata::TextField::Title, "Harbour")?;
    let copy = library.create_virtual_copy(ids[0])?.value;
    library.select(Some(copy));
    // Typed, not saved yet.
    library.set_copy_name_draft(copy, "Night");
    let scope = e.export_scope().expect("the copy");
    let fields = &scope.photos[0].fields;
    assert_eq!(
        (fields.title.as_deref(), fields.copy_name.as_str()),
        (Some("Harbour"), "Night")
    );
    assert_eq!(fields.captured, None);
    // The catalog changes after the dialog opened.
    let catalog = &mut e.library.as_mut().unwrap().session.catalog;
    catalog.set_text(&[copy], crate::metadata::TextField::Title, "Later")?;
    catalog.fill_capture_times(&[(copy, "2026-04-24T18:43:24.000".into())])?;
    let mut s = settings(&dir.path().join("out"));
    s.choose_tokens("{{com.adobe.title}} {{copy_name}} {{date_YYYY}}");
    let example = s
        .file_name_for(
            &scope.photos[0].source,
            &crate::export_settings::NameContext {
                index: 0,
                total: 1,
                fields: &scope.photos[0].fields,
            },
        )
        .0;
    assert_eq!(example, "Harbour Night.tif");
    let batch = e.batch_photos(&scope, false)?;
    let plan = batch::plan(&batch, &s, None).unwrap();
    assert_eq!(
        plan.entries[0].target.file_name().unwrap(),
        "Harbour Night.tif"
    );
    assert_eq!(plan.entries[0].notes, ["no capture date"]);
    Ok(())
}

/// A template that needs the camera's model reads it once, when it first
/// needs it; Export (and Return) wait for it; a result of another read is
/// dropped; the files are named from what was read.
#[test]
fn the_dialog_reads_the_camera_names_once_a_template_needs_them() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    let (reader, open, read) = gated_reader();
    e.exports.read.set_reader(reader);
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let out = dir.path().join("out");
    open_dialog(&mut e, settings(&out));
    // Filename needs nothing from the file.
    frame(&mut e, &ctx, vec![]);
    assert_eq!(e.exports.read.status(), Status::Idle);
    // The editor's template needs the model: read now, once.
    e.exports.draft.choose_tokens(MODEL);
    frame(&mut e, &ctx, vec![]);
    frame(&mut e, &ctx, vec![]);
    assert_eq!(
        e.exports.read.status(),
        Status::Reading { done: 0, total: 1 }
    );
    press_return(&mut e, &ctx);
    assert!(
        e.exports.dialog && e.exports.queued.is_empty(),
        "Return waits"
    );
    // A read of an earlier scope is dropped.
    e.names_read(0, Ok(vec![Ok(FileFields::default())]));
    assert!(matches!(e.exports.read.status(), Status::Reading { .. }));
    open.send(())?;
    until(&mut e, &ctx, |e| e.exports.read.status() == Status::Ready);
    frame(&mut e, &ctx, vec![]);
    assert_eq!(read.load(std::sync::atomic::Ordering::SeqCst), 1);
    press_return(&mut e, &ctx);
    assert!(!e.exports.dialog);
    wait(&mut e, &ctx);
    assert_eq!(listing(&out), ["ILCE-7RM6-a.tif"]);
    Ok(())
}

/// Cancel stops the read; its late completion changes nothing, and the next
/// dialog reads again.
#[test]
fn cancelling_the_dialog_drops_its_read() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    let (reader, open, _read) = gated_reader();
    e.exports.read.set_reader(reader);
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let mut s = settings(&dir.path().join("out"));
    s.choose_tokens(MODEL);
    open_dialog(&mut e, s.clone());
    frame(&mut e, &ctx, vec![]);
    assert!(matches!(e.exports.read.status(), Status::Reading { .. }));
    let escape = |pressed| egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut e, &ctx, vec![escape(true), escape(false)]);
    frame(&mut e, &ctx, vec![]);
    assert!(!e.exports.dialog);
    assert_eq!(e.exports.read.status(), Status::Idle);
    // The cancelled worker stops without a result; any that came is dropped.
    open.send(())?;
    e.names_read(1, Ok(vec![Ok(FileFields::default())]));
    assert_eq!(e.exports.read.status(), Status::Idle);
    open_dialog(&mut e, s);
    frame(&mut e, &ctx, vec![]);
    open.send(())?;
    until(&mut e, &ctx, |e| e.exports.read.status() == Status::Ready);
    assert!(matches!(
        e.exports.scope.photos[0].fields.camera,
        crate::export_settings::CameraNames::Read { .. }
    ));
    Ok(())
}

/// A reader that panics: the read fails, nothing stays busy, and Export says
/// it goes on without the names' fields; the files then say what they lack.
#[test]
fn a_failed_read_is_said_and_exports_without_it() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    e.exports.read.set_reader(Arc::new(
        |_: &name_read::ToRead| -> Result<FileFields, String> { panic!("disk gone") },
    ));
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let out = dir.path().join("out");
    let mut s = settings(&out);
    s.choose_tokens(MODEL);
    // Export with Previous waits, then opens the dialog to say so.
    e.exports.unsaved_settings = true;
    e.export_previous(Some(s));
    assert!(e.exports.waiting.is_some() && e.export_modal());
    until(&mut e, &ctx, |e| e.exports.dialog);
    assert!(e.exports.waiting.is_none());
    match e.exports.read.status() {
        Status::Failed(why) => assert!(why.contains("disk gone"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(e.status.contains("could not be read"), "{}", e.status);
    press_return(&mut e, &ctx);
    wait(&mut e, &ctx);
    // As Lightroom: the text around a value it lacks stays.
    assert_eq!(listing(&out), ["-a.tif"]);
    let summary = e.exports.summary.clone().expect("the notes");
    assert_eq!(
        summary.notes,
        [(
            "a.dng".to_string(),
            "the camera's make and model were not read".to_string()
        )]
    );
    Ok(())
}

/// Export with Previous with a template that needs the files waits for them in
/// a window that can cancel, then exports; one that needs nothing exports at
/// once.
#[test]
fn export_with_previous_waits_for_what_its_names_need() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    let (reader, open, _read) = gated_reader();
    e.exports.read.set_reader(reader);
    e.exports.unsaved_settings = true;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let out = dir.path().join("out");
    let mut s = settings(&out);
    s.choose_tokens(MODEL);
    e.export_previous(Some(s.clone()));
    assert!(e.exports.waiting.is_some() && e.exports.queued.is_empty());
    // Cancel in the window.
    frame(&mut e, &ctx, vec![]);
    let escape = |pressed| egui::Event::Key {
        key: egui::Key::Escape,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    };
    frame(&mut e, &ctx, vec![escape(true), escape(false)]);
    assert!(e.exports.waiting.is_none() && !e.export_modal());
    assert_eq!(e.status, "Export cancelled");
    open.send(())?;
    // Again, to the end.
    e.export_previous(Some(s));
    open.send(())?;
    wait_for_queue(&mut e, &ctx);
    assert_eq!(listing(&out), ["ILCE-7RM6-a.tif"]);
    // Nothing to read: at once.
    e.export_previous(Some(settings(&dir.path().join("plain"))));
    assert!(e.exports.waiting.is_none() && !e.exports.queued.is_empty());
    wait(&mut e, &ctx);
    assert_eq!(listing(&dir.path().join("plain")), ["a.tif"]);
    Ok(())
}
/// Takes events until an export was queued and has finished.
fn wait_for_queue(e: &mut Editor, ctx: &egui::Context) {
    until(e, ctx, |e| !e.exports.queued.is_empty());
    wait(e, ctx);
}

/// A photo opened without a catalog has its capture time in its file only:
/// Date - Filename waits for it, which a catalog photo doesn't.
#[test]
fn a_photo_without_a_catalog_reads_its_date_from_its_file() -> anyhow::Result<()> {
    let (_dir, mut e, ids, _ctx) = editor()?;
    let date = ExportSettings {
        naming: Some(crate::export_settings::Naming::DateFilename),
        ..Default::default()
    };
    e.library.as_mut().unwrap().select(Some(ids[0]));
    e.exports.scope = e.export_scope().unwrap();
    assert!(!e.names_wait(&date.name_template()));
    let mut model = ExportSettings::default();
    model.choose_tokens(MODEL);
    e.exports
        .read
        .set_reader(Arc::new(|_: &name_read::ToRead| Ok(FileFields::default())));
    assert!(
        e.names_wait(&model.name_template()),
        "a catalog photo's model"
    );
    e.exports.read.cancel();
    // Standalone.
    e.exports.scope.photos[0].id = None;
    assert!(e.names_wait(&date.name_template()));
    e.exports.read.cancel();
    // What its file gives goes in its fields.
    let mut fields = crate::export_settings::NameFields::default();
    name_read::fill(
        &mut fields,
        FileFields {
            captured: Some("2026-04-24T18:43:24.000".into()),
            info: Some(crate::metadata::PhotoInfo {
                iso: Some(100.),
                ..Default::default()
            }),
            ..Default::default()
        },
        true,
    );
    assert_eq!(fields.captured.as_deref(), Some("2026-04-24T18:43:24.000"));
    assert_eq!(fields.info.iso, Some(100.));
    Ok(())
}

/// The real reader: a DNG's make and model from its EXIF, the maker spelled
/// as RAWmakase spells it; a file with none has none.
#[test]
fn the_reader_takes_the_files_own_names() -> anyhow::Result<()> {
    let (dir, e, ids, _ctx) = editor()?;
    let dng = e
        .library
        .as_ref()
        .unwrap()
        .photo(ids[0])
        .unwrap()
        .path
        .clone();
    let (make, model) = crate::exif::camera_names(&dng).unwrap_or_default();
    let read = name_read::read_path(&dng, true).unwrap();
    // Its info as the catalog reads it, dimensions included.
    assert!(
        read.info.as_ref().is_some_and(|i| i.dimensions.is_some()),
        "{read:?}"
    );
    assert_eq!(
        read.camera,
        crate::export_settings::CameraNames::Read {
            make: Some(crate::raw::maker_name(&make)).filter(|m| !m.is_empty()),
            model: Some(model).filter(|m| !m.is_empty()),
        }
    );
    let text = dir.path().join("note.txt");
    std::fs::write(&text, "not a photo")?;
    assert_eq!(
        name_read::read_path(&text, false).unwrap().camera,
        crate::export_settings::CameraNames::Read {
            make: None,
            model: None
        }
    );
    // A file gone is an error, not a photo without names.
    std::fs::remove_file(&text)?;
    assert!(name_read::read_path(&text, false).is_err());
    Ok(())
}

/// An X3F, which RAWmakase doesn't decode, names its export from its
/// preview's EXIF, else its property list; opened without a catalog, it
/// gets its photo info too, not "can't be read" (#671).
#[test]
fn an_x3f_names_its_export_from_its_exif_or_property_list() -> anyhow::Result<()> {
    use crate::exif::{CameraExif, Field, tag};
    use crate::export::exif::{Container, camera_raw, x3f_with_properties};
    let dir = tempfile::tempdir()?;
    let quattro = dir.path().join("SDIM4388.X3F");
    let camera = CameraExif {
        main: vec![
            Field::ascii(tag::MAKE, "SIGMA"),
            Field::ascii(tag::MODEL, "sd Quattro"),
        ],
        ..Default::default()
    };
    std::fs::write(&quattro, camera_raw(Container::X3f, camera))?;
    let sd10 = dir.path().join("RAW_SIGMA_SD10.X3F");
    std::fs::write(
        &sd10,
        x3f_with_properties(&[("CAMMANUF", "SIGMA"), ("CAMMODEL", "SIGMA SD10")]),
    )?;
    for (path, model, camera) in [
        (&quattro, "sd Quattro", "SIGMA sd Quattro"),
        (&sd10, "SIGMA SD10", "SIGMA SD10"),
    ] {
        let read = name_read::read_path(path, true).unwrap();
        assert_eq!(
            read.camera,
            crate::export_settings::CameraNames::Read {
                make: Some("Sigma".into()),
                model: Some(model.into()),
            },
            "{model}"
        );
        assert_eq!(read.info.and_then(|i| i.camera).as_deref(), Some(camera));
    }
    Ok(())
}

/// A file's own title, caption, copyright and creator: its sidecar's XMP
/// first, then its EXIF's Image Description, Copyright (the photographer's
/// part) and Artist (several people as Lightroom names them); they fill
/// only what the catalog has none of, a cleared caption staying empty.
#[test]
fn a_files_own_descriptive_text_names_what_the_catalog_lacks() -> anyhow::Result<()> {
    use crate::exif::{CameraExif, Field, tag};
    use crate::export::exif::{Container, camera_raw};
    let dir = tempfile::tempdir()?;
    let x3f = dir.path().join("SDIM0001.X3F");
    let mut copyright = b"Ann Example\0Ed Itor\0".to_vec();
    let camera = CameraExif {
        main: vec![
            Field::ascii(tag::MAKE, "SIGMA"),
            Field::ascii(tag::MODEL, "sd Quattro"),
            Field::ascii(tag::IMAGE_DESCRIPTION, "From EXIF"),
            Field::ascii(tag::ARTIST, "A One; B Two"),
            Field {
                tag: tag::COPYRIGHT,
                kind: 2,
                count: copyright.len() as u32,
                bytes: std::mem::take(&mut copyright),
            },
        ],
        ..Default::default()
    };
    std::fs::write(&x3f, camera_raw(Container::X3f, camera))?;
    std::fs::write(
        dir.path().join("SDIM0001.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" dc:title="Harbour"/>
        </rdf:RDF></x:xmpmeta>"#,
    )?;
    let read = name_read::read_path(&x3f, false).unwrap();
    assert_eq!(
        read.descriptive,
        name_read::FileText {
            title: Some("Harbour".into()),
            caption: Some("From EXIF".into()),
            copyright: Some("Ann Example".into()),
            creator: Some("A One, B Two".into()),
        }
    );
    let mut fields = NameFields {
        caption: Some(String::new()),
        creator: Some("Catalog Person".into()),
        ..Default::default()
    };
    name_read::fill(&mut fields, read, false);
    assert_eq!(fields.title.as_deref(), Some("Harbour"));
    assert_eq!(fields.caption.as_deref(), Some(""), "cleared stays cleared");
    assert_eq!(fields.copyright.as_deref(), Some("Ann Example"));
    assert_eq!(fields.creator.as_deref(), Some("Catalog Person"));
    Ok(())
}

/// A title only the photo's sidecar has names its export: the Example waits
/// for the file, then has it.
#[test]
fn a_title_only_the_file_has_names_the_export() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    let source = e
        .library
        .as_ref()
        .unwrap()
        .photo(ids[0])
        .unwrap()
        .path
        .clone();
    std::fs::write(
        source.with_extension("xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/" dc:title="Harbour"/>
        </rdf:RDF></x:xmpmeta>"#,
    )?;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    let mut s = settings(&dir.path().join("out"));
    s.choose_tokens("{{com.adobe.title}}");
    open_dialog(&mut e, s);
    frame(&mut e, &ctx, vec![]);
    assert!(matches!(e.exports.read.status(), Status::Reading { .. }));
    until(&mut e, &ctx, |e| e.exports.read.status() == Status::Ready);
    let photo = &e.exports.scope.photos[0];
    let at = NameContext {
        index: 0,
        total: 1,
        fields: &photo.fields,
    };
    assert_eq!(
        e.exports.draft.file_name_for(&photo.source, &at).0,
        "Harbour.tif"
    );
    Ok(())
}

/// Return belongs to the Filename Template Editor while it is open over the
/// dialog: it is Done, keeping the template, and nothing is exported. With a
/// mistake in the template it does nothing, and never types a line break.
#[test]
fn return_in_the_template_editor_is_done_and_does_not_export() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    open_dialog(&mut e, settings(&dir.path().join("out")));
    let mut wrong = name_editor::NameEditor::new(MODEL);
    wrong.text.push('{');
    let typed = wrong.text.clone();
    e.exports.name_editor = Some(wrong);
    // Typing in the text, as when Return is pressed in the app.
    frame(&mut e, &ctx, vec![]);
    ctx.memory_mut(|m| m.request_focus(name_editor::text_id()));
    frame(&mut e, &ctx, vec![]);
    assert!(ctx.memory(|m| m.has_focus(name_editor::text_id())));
    press_return(&mut e, &ctx);
    assert_eq!(
        e.exports.name_editor.as_ref().map(|n| &n.text),
        Some(&typed)
    );
    assert_eq!(e.exports.draft.custom_tokens(), None);
    e.exports.name_editor = Some(name_editor::NameEditor::new(MODEL));
    press_return(&mut e, &ctx);
    assert!(e.exports.name_editor.is_none());
    assert_eq!(e.exports.draft.custom_tokens(), Some(MODEL));
    assert!(e.exports.dialog && e.exports.queued.is_empty());
    Ok(())
}

/// A line break never gets into the template: pasted or typed, it is
/// dropped before the text takes it, so what is typed next lands at the
/// cursor.
#[test]
fn line_breaks_never_get_into_the_template() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    open_dialog(&mut e, settings(&dir.path().join("out")));
    let mut typing = name_editor::NameEditor::new("abcd");
    typing.text = "abcd".into();
    e.exports.name_editor = Some(typing);
    frame(&mut e, &ctx, vec![]);
    ctx.memory_mut(|m| m.request_focus(name_editor::text_id()));
    frame(&mut e, &ctx, vec![]);
    let mut state = egui::TextEdit::load_state(&ctx, name_editor::text_id()).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(2),
        )));
    state.store(&ctx, name_editor::text_id());
    frame(&mut e, &ctx, vec![egui::Event::Paste("X\r\nY\n".into())]);
    frame(&mut e, &ctx, vec![egui::Event::Text("Z".into())]);
    frame(&mut e, &ctx, vec![egui::Event::Text("V\nW".into())]);
    frame(&mut e, &ctx, vec![egui::Event::Text("U".into())]);
    assert_eq!(
        e.exports.name_editor.as_ref().map(|n| n.text.as_str()),
        Some("abXYZVWUcd")
    );
    // An input method's text, while composed and once committed.
    let mut state = egui::TextEdit::load_state(&ctx, name_editor::text_id()).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(2),
        )));
    state.store(&ctx, name_editor::text_id());
    let composed = |e: &Editor| e.exports.name_editor.as_ref().map(|n| n.text.clone());
    frame(
        &mut e,
        &ctx,
        vec![egui::Event::Ime(egui::ImeEvent::Preedit {
            text: "日\n本".into(),
            active_range_chars: Some(2..3),
        })],
    );
    assert_eq!(composed(&e).as_deref(), Some("ab日本XYZVWUcd"));
    frame(
        &mut e,
        &ctx,
        vec![egui::Event::Ime(egui::ImeEvent::Commit("日\r\n本".into()))],
    );
    assert_eq!(composed(&e).as_deref(), Some("ab日本XYZVWUcd"));
    Ok(())
}

/// Escape closes one window at a time: the Filename Template Editor first,
/// dropping its template, also while its text is being typed in, and then
/// the dialog.
#[test]
fn escape_closes_the_template_editor_then_the_dialog() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    e.library.as_mut().unwrap().select(Some(ids[0]));
    open_dialog(&mut e, settings(&dir.path().join("out")));
    frame(&mut e, &ctx, vec![]);
    e.exports.name_editor = Some(name_editor::NameEditor::new(MODEL));
    frame(&mut e, &ctx, vec![]);
    ctx.memory_mut(|m| m.request_focus(name_editor::text_id()));
    frame(&mut e, &ctx, vec![]);
    assert!(ctx.memory(|m| m.has_focus(name_editor::text_id())));
    press(&mut e, &ctx, egui::Key::Escape);
    assert!(e.exports.name_editor.is_none());
    assert!(e.exports.dialog);
    assert_eq!(e.exports.draft.custom_tokens(), None);
    press(&mut e, &ctx, egui::Key::Escape);
    assert!(!e.exports.dialog);
    // Escape as the editor opens from the dialog never closes the dialog
    // under it.
    open_dialog(&mut e, settings(&dir.path().join("out")));
    frame(&mut e, &ctx, vec![]);
    e.exports.name_editor = Some(name_editor::NameEditor::new(MODEL));
    press(&mut e, &ctx, egui::Key::Escape);
    assert!(e.exports.dialog);
    press(&mut e, &ctx, egui::Key::Escape);
    assert!(e.exports.name_editor.is_none());
    assert!(e.exports.dialog && e.exports.queued.is_empty());
    Ok(())
}

/// A file that can't be read fails the read (Export says it goes on without
/// it); what the others gave is used.
#[test]
fn a_file_that_cant_be_read_fails_the_read_and_keeps_the_others() -> anyhow::Result<()> {
    let (dir, mut e, ids, ctx) = editor()?;
    e.exports
        .read
        .set_reader(Arc::new(|photo: &name_read::ToRead| {
            if photo.path.ends_with("b.dng") {
                return Err("b.dng can't be read".into());
            }
            Ok(FileFields {
                camera: crate::export_settings::CameraNames::Read {
                    make: None,
                    model: Some("X100".into()),
                },
                ..Default::default()
            })
        }));
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    library.select_range_to(ids[1]);
    let mut s = settings(&dir.path().join("out"));
    s.choose_tokens(MODEL);
    open_dialog(&mut e, s);
    frame(&mut e, &ctx, vec![]);
    until(&mut e, &ctx, |e| {
        matches!(e.exports.read.status(), Status::Failed(_))
    });
    assert_eq!(
        e.exports.read.status(),
        Status::Failed("b.dng can't be read".into())
    );
    let fields: Vec<_> = e
        .exports
        .scope
        .photos
        .iter()
        .map(|p| p.fields.camera.clone())
        .collect();
    assert_eq!(
        fields,
        [
            crate::export_settings::CameraNames::Read {
                make: None,
                model: Some("X100".into())
            },
            crate::export_settings::CameraNames::NotRead,
        ]
    );
    press_return(&mut e, &ctx);
    wait(&mut e, &ctx);
    assert_eq!(listing(&dir.path().join("out")), ["-b.tif", "X100-a.tif"]);
    Ok(())
}
