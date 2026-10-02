use super::*;

fn gray(width: u32, height: u32, v: f32) -> Rendered {
    Rendered {
        width,
        height,
        pixels: vec![[v; 3]; (width * height) as usize],
    }
}
/// A graphic preset over a `w`×`h` PNG (red, with a transparent left half)
/// saved in a fresh folder.
fn graphic(w: u32, h: u32, sixteen: bool) -> Result<(tempfile::TempDir, Watermark)> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("logo.png");
    if sixteen {
        let img = image::ImageBuffer::from_fn(w, h, |x, _| {
            image::Rgba([65535u16, 0, 0, if x < w / 2 { 0 } else { 65535 }])
        });
        img.save(&source)?;
    } else {
        let img = image::ImageBuffer::from_fn(w, h, |x, _| {
            image::Rgba([255u8, 0, 0, if x < w / 2 { 0 } else { 255 }])
        });
        img.save(&source)?;
    }
    let store = dir.path().join("watermarks");
    let saved = save_in(
        &store,
        &Watermark {
            name: "Logo".into(),
            style: Style::Graphic,
            inset: [0., 0.],
            ..Default::default()
        },
        Some(&source),
    )?;
    Ok((dir, saved))
}

#[test]
fn each_anchor_puts_the_mark_on_its_point_with_its_inset() -> Result<()> {
    let (dir, mut w) = graphic(10, 10, false)?;
    let images = dir.path().join("watermarks/images");
    w.size = Size::Proportional(0.1);
    w.inset = [0.05, 0.1];
    for (anchor, at) in [
        (Anchor(0, 0), (10, 10)),
        (Anchor(1, 1), (90, 40)),
        (Anchor(2, 2), (170, 70)),
        (Anchor(2, 0), (170, 10)),
        (Anchor(0, 2), (10, 70)),
    ] {
        w.anchor = anchor;
        let placed = w.ready_in(&images)?.place(200, 100).unwrap();
        assert_eq!((placed.width, placed.height), (20, 20), "{anchor:?}");
        assert_eq!((placed.x, placed.y), at, "{anchor:?}");
    }
    Ok(())
}

#[test]
fn fit_and_fill_size_the_mark_to_the_photo() -> Result<()> {
    let (dir, mut w) = graphic(40, 10, false)?;
    let images = dir.path().join("watermarks/images");
    w.inset = [0.1, 0.1];
    w.size = Size::Fit;
    let p = w.ready_in(&images)?.place(200, 100).unwrap();
    assert_eq!((p.width, p.height), (160, 40));
    // Fill: as large as fits both ways; a tall photo limits the width.
    let (dir, mut w) = graphic(10, 40, false)?;
    let images = dir.path().join("watermarks/images");
    w.inset = [0., 0.];
    w.size = Size::Fill;
    let p = w.ready_in(&images)?.place(200, 100).unwrap();
    assert_eq!((p.width, p.height), (25, 100));
    Ok(())
}

#[test]
fn rotation_turns_the_mark_in_quarter_turns() -> Result<()> {
    let (dir, mut w) = graphic(20, 10, false)?;
    let images = dir.path().join("watermarks/images");
    w.size = Size::Proportional(0.1);
    w.rotation = 1;
    let p = w.ready_in(&images)?.place(100, 100).unwrap();
    // Turned upright: 10 wide (a tenth of the photo), 20 tall.
    assert_eq!((p.width, p.height), (10, 20));
    // The transparent left half is now at the bottom.
    assert_eq!(p.rgba[(p.height - 1) * p.width][3], 0.);
    assert_eq!(p.rgba[0][3], 1.);
    Ok(())
}

#[test]
fn png_alpha_is_honoured_and_untouched_pixels_keep_full_precision() -> Result<()> {
    let (dir, mut w) = graphic(100, 50, true)?;
    let images = dir.path().join("watermarks/images");
    w.size = Size::Fit;
    w.opacity = 0.5;
    w.anchor = Anchor(0, 0);
    // A value 16-bit TIFF holds and 8-bit can't.
    let v = 1000. / 65535.;
    let mut image = gray(100, 100, v);
    w.ready_in(&images)?.apply(&mut image);
    // Outside the mark, and under its transparent half, exactly as rendered.
    assert_eq!(image.pixels[99 * 100 + 99], [v; 3]);
    assert_eq!(image.pixels[10 * 100 + 10], [v; 3]);
    // Under the half-opaque red, the exact mix.
    let p = image.pixels[10 * 100 + 90];
    assert!((p[0] - (0.5 + v * 0.5)).abs() < 1e-6);
    assert!((p[1] - v * 0.5).abs() < 1e-6);
    Ok(())
}

#[test]
fn a_missing_image_fails_before_anything_is_drawn() {
    let w = Watermark {
        style: Style::Graphic,
        image: Some("gone.png".into()),
        ..Default::default()
    };
    let error = w.ready_in(Path::new("/nonexistent")).err().unwrap();
    assert_eq!(error.to_string(), "Watermark image not found: gone.png");
}

#[test]
fn deleting_a_preset_removes_its_own_image() -> Result<()> {
    let (dir, w) = graphic(4, 4, false)?;
    let store = dir.path().join("watermarks");
    let image = store.join("images").join(w.image.clone().unwrap());
    assert!(image.is_file());
    assert_eq!(presets_in(&store), vec![w.clone()]);
    // The source can go: the preset has its own copy.
    std::fs::remove_file(dir.path().join("logo.png"))?;
    assert!(w.ready_in(&store.join("images")).is_ok());
    delete_in(&store, &w)?;
    assert!(!image.exists());
    assert!(presets_in(&store).is_empty());
    Ok(())
}

#[test]
fn text_is_drawn_with_smooth_edges_and_a_shadow() -> Result<()> {
    let mut w = Watermark {
        text: "Ag © 2026".into(),
        size: Size::Proportional(0.5),
        color: [1., 1., 1.],
        ..Default::default()
    };
    let plain = w.ready()?.place(400, 200).unwrap();
    assert!((plain.width as f32 - 200.).abs() < 12.);
    let alphas: Vec<f32> = plain.rgba.iter().map(|p| p[3]).collect();
    assert!(alphas.iter().any(|a| *a > 0.99), "solid strokes");
    assert!(
        alphas.iter().any(|a| *a > 0.05 && *a < 0.95),
        "antialiased edges"
    );
    assert!(alphas.iter().all(|a| (0. ..=1.).contains(a)));
    // The shadow darkens around the text, not over it.
    w.shadow.enabled = true;
    w.shadow.opacity = 1.;
    let shadowed = w.ready()?.place(400, 200).unwrap();
    let covered = |p: &Placed| p.rgba.iter().filter(|c| c[3] > 0.01).count();
    assert!(covered(&shadowed) > covered(&plain));
    assert!(
        shadowed.rgba.iter().any(|c| c[3] > 0.3 && c[0] < 0.2),
        "dark shadow pixels"
    );
    Ok(())
}

#[test]
fn the_weight_follows_the_style() -> Result<()> {
    let ink = |face: &str| -> Result<f32> {
        let w = Watermark {
            text: "Heavy".into(),
            face: face.into(),
            size: Size::Proportional(0.5),
            ..Default::default()
        };
        let placed = w.ready()?.place(400, 200).unwrap();
        Ok(placed.rgba.iter().map(|p| p[3]).sum::<f32>() / (placed.width * placed.height) as f32)
    };
    assert!(ink("Bold")? > ink("Regular")? * 1.2);
    Ok(())
}

#[test]
fn renaming_a_preset_keeps_its_image() -> Result<()> {
    let (dir, w) = graphic(4, 4, false)?;
    let store = dir.path().join("watermarks");
    let renamed = rename_in(
        &store,
        &w.name,
        &Watermark {
            name: "Studio".into(),
            ..w.clone()
        },
    )?;
    let names: Vec<String> = presets_in(&store).into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["Studio"]);
    assert!(renamed.ready_in(&store.join("images")).is_ok());
    Ok(())
}
