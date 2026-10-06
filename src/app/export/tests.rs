use super::*;
use crate::export_settings::{Destination, Format};

/// An editor on a catalog of two copies of the synthetic chart DNG and a JPEG.
fn editor() -> anyhow::Result<(tempfile::TempDir, Editor, Vec<i64>, egui::Context)> {
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
    let ids = library.photos.iter().map(|p| p.id).collect();
    e.library = Some(Box::new(library));
    e.library_mode = true;
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
    let scope = e.export_scope().expect("photos to export");
    assert_eq!(
        scope
            .photos
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["a.dng", "b.dng"]
    );
    assert_eq!(scope.left_out.len(), 1, "{:?}", scope.left_out);
    assert_eq!(scope.left_out[0].0, "c.jpg");
    let out = dir.path().join("out");
    export(&mut e, scope.clone(), settings(&out))?;
    wait(&mut e, &ctx);
    assert_eq!(listing(&out), ["a.tif", "b.tif"]);
    assert_eq!(e.status, "Exported 2 of 3 · 1 not exported");
    // Kept in the top bar until read: the JPEG, and why.
    let summary = e.exports.summary.clone().expect("a report");
    assert_eq!(summary.problems.len(), 1);
    assert!(summary.problems[0].1.contains("can't be exported"));

    // Again into the same folder: Ask asks once for both files.
    export(&mut e, scope, settings(&out))?;
    let pending = e.exports.pending.take().expect("the question");
    assert_eq!(pending.conflicts, [out.join("a.tif"), out.join("b.tif")]);
    e.plan_export(pending, Some(Existing::Unique));
    wait(&mut e, &ctx);
    assert_eq!(listing(&out), ["a-2.tif", "a.tif", "b-2.tif", "b.tif"]);
    // That export went well, but the first one's report is unread: it stays, and
    // the two add up until it is dismissed.
    let summary = e.exports.summary.clone().expect("the unread report");
    // The JPEG was left out of both.
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
    e.document.set_image(Arc::new(crate::raw::CameraImage {
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
    e.library_mode = false;
    let library = e.library.as_mut().unwrap();
    library.select(Some(ids[0]));
    assert!(e.export_scope().is_none());
    assert!(
        e.status.starts_with("Export waits for the open photo"),
        "{}",
        e.status
    );
    // A Library selection without it goes ahead.
    e.library_mode = true;
    e.library.as_mut().unwrap().select(Some(ids[1]));
    let scope = e.export_scope().expect("the other photo");
    assert_eq!(scope.photos.len(), 1);
    assert!(!scope.photos[0].open);
    // Once final, the open photo is exported as shown.
    e.document.pending_lightroom = None;
    e.library_mode = false;
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
    e.document.set_image(Arc::new(crate::raw::CameraImage {
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
