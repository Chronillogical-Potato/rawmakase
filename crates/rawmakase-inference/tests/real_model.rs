//! Runs the real model. Ignored by default: set `RAWMAKASE_TEST_MODEL` to the folder
//! holding the model's files and `RAWMAKASE_ORT_LIB` to an ONNX Runtime library.
use rawmakase_inference::{InferenceError, Point, Prompt, RgbImage, Subject};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

fn model() -> PathBuf {
    PathBuf::from(std::env::var("RAWMAKASE_TEST_MODEL").expect("RAWMAKASE_TEST_MODEL"))
}
/// A bright disc on a dark, slightly textured ground.
fn scene() -> RgbImage {
    let (w, h) = (320usize, 240usize);
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        for x in 0..w {
            let inside = ((x as f32 - 200.).powi(2) + (y as f32 - 120.).powi(2)).sqrt() < 60.;
            let v = if inside {
                [230, 60, 50]
            } else {
                [30 + (x % 7) as u8, 40, 60]
            };
            data.extend(v);
        }
    }
    RgbImage {
        width: w,
        height: h,
        data,
    }
}

#[test]
#[ignore = "needs the model and an ONNX Runtime"]
fn a_click_selects_the_object_under_it_and_a_click_elsewhere_does_not() {
    let cancel = AtomicBool::new(false);
    let mut session = Subject::load(&model()).unwrap();
    let image = scene();
    let embedding = session.embed(&image, &cancel).unwrap();
    let click = |x: f32, y: f32| Prompt {
        points: vec![Point {
            x,
            y,
            positive: true,
        }],
        bounds: None,
    };
    let on = session
        .segment(&embedding, &click(200. / 320., 0.5), &cancel)
        .unwrap();
    assert_eq!((on.width, on.height), (320, 240));
    let at = |c: &rawmakase_inference::Coverage, x: usize, y: usize| c.data[y * c.width + x];
    assert!(at(&on, 200, 120) > 200, "{}", at(&on, 200, 120));
    assert!(at(&on, 20, 20) < 40, "{}", at(&on, 20, 20));
    // A box does the same.
    let boxed = Prompt {
        points: vec![],
        bounds: Some([130. / 320., 50. / 240., 270. / 320., 190. / 240.]),
    };
    let by_box = session.segment(&embedding, &boxed, &cancel).unwrap();
    assert!(at(&by_box, 200, 120) > 200 && at(&by_box, 20, 20) < 40);
    // A raised flag never yields a result.
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        session
            .segment(&embedding, &click(0.5, 0.5), &cancel)
            .unwrap_err(),
        InferenceError::Cancelled
    );
    assert_eq!(
        session.embed(&image, &cancel).err(),
        Some(InferenceError::Cancelled)
    );
}

#[test]
#[ignore = "needs the model and an ONNX Runtime"]
fn the_subject_is_found_without_a_click_and_the_object_in_front_is_not_sky() {
    let cancel = AtomicBool::new(false);
    let mut session = Subject::load(&model()).unwrap();
    let image = scene();
    let analysis = session.analyze(&image, &cancel).unwrap();
    let subject = analysis.subject().unwrap();
    let at = |c: &rawmakase_inference::Coverage, x: usize, y: usize| c.data[y * c.width + x];
    assert!(at(&subject, 200, 120) > 200, "{}", at(&subject, 200, 120));
    assert!(at(&subject, 20, 20) < 40, "{}", at(&subject, 20, 20));
    // The smooth blue backdrop may count as sky; the object in front of it never does.
    let sky = analysis.sky().unwrap();
    assert!(at(&sky, 200, 120) < 40, "{}", at(&sky, 200, 120));
    // Clicks work on the same analysis.
    let click = Prompt {
        points: vec![Point {
            x: 200. / 320.,
            y: 0.5,
            positive: true,
        }],
        bounds: None,
    };
    assert!(
        at(
            &session
                .segment(analysis.embedding(), &click, &cancel)
                .unwrap(),
            200,
            120
        ) > 200
    );
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        session.analyze(&image, &cancel).err(),
        Some(InferenceError::Cancelled)
    );
}
