//! Runs the real model end to end. Ignored by default: it needs the pinned model
//! file and an ONNX Runtime library.
//!
//! ```text
//! RAWMAKASE_TEST_MODEL=/path/to/isnet-general-use.onnx \
//! RAWMAKASE_ORT_LIB=/path/to/libonnxruntime.dylib \
//!   cargo test -p rawmakase-inference --release -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rawmakase_inference::{InferenceError, RgbImage, Subject};

/// A blue-green gradient with a red disc and a yellow rectangle: two clear
/// subjects on a smooth background.
fn synthetic(width: usize, height: usize) -> RgbImage {
    let mut data = Vec::with_capacity(width * height * 3);
    let (cx, cy, r) = (
        width as f32 * 0.3,
        height as f32 * 0.52,
        height as f32 * 0.2,
    );
    for y in 0..height {
        for x in 0..width {
            let (fx, fy) = (x as f32, y as f32);
            let in_disc = (fx - cx).powi(2) + (fy - cy).powi(2) < r * r;
            let in_rect = (fx - width as f32 * 0.7).abs() < width as f32 * 0.11
                && (fy - height as f32 * 0.5).abs() < height as f32 * 0.22;
            let pixel = if in_disc {
                [230, 60, 50]
            } else if in_rect {
                [240, 220, 60]
            } else {
                [
                    (40.0 + 100.0 * fx / width as f32) as u8,
                    (90.0 + 80.0 * fy / height as f32) as u8,
                    140,
                ]
            };
            data.extend_from_slice(&pixel);
        }
    }
    RgbImage {
        width,
        height,
        data,
    }
}

fn model_path() -> Option<PathBuf> {
    std::env::var_os("RAWMAKASE_TEST_MODEL").map(PathBuf::from)
}

#[test]
#[ignore = "needs RAWMAKASE_TEST_MODEL and an ONNX Runtime library (RAWMAKASE_ORT_LIB)"]
fn selects_the_synthetic_subjects_at_the_input_aspect() {
    let Some(model) = model_path() else {
        panic!("set RAWMAKASE_TEST_MODEL to the model file");
    };
    let started = Instant::now();
    let mut session = Subject::load(&model).expect("load the runtime and the model");
    eprintln!("cold load: {:?}", started.elapsed());

    let (width, height) = (1536, 1024);
    let image = synthetic(width, height);
    let cancel = AtomicBool::new(false);
    for run in 0..3 {
        let started = Instant::now();
        let coverage = session.select(&image, &cancel).expect("select");
        eprintln!("select #{run}: {:?}", started.elapsed());
        assert_eq!((coverage.width, coverage.height), (width, height));
        let at = |fx: f32, fy: f32| {
            coverage.data[(fy * height as f32) as usize * width + (fx * width as f32) as usize]
        };
        assert!(
            at(0.3, 0.52) > 200,
            "disc centre is subject: {}",
            at(0.3, 0.52)
        );
        assert!(
            at(0.7, 0.5) > 200,
            "rectangle centre is subject: {}",
            at(0.7, 0.5)
        );
        assert!(
            at(0.03, 0.03) < 40,
            "top-left corner is background: {}",
            at(0.03, 0.03)
        );
        assert!(
            at(0.97, 0.97) < 40,
            "bottom-right corner is background: {}",
            at(0.97, 0.97)
        );
        let mean = coverage.data.iter().map(|&v| f64::from(v)).sum::<f64>()
            / coverage.data.len() as f64
            / 255.0;
        assert!(
            (0.05..0.5).contains(&mean),
            "plausible subject area, got {mean}"
        );
    }
}

#[test]
#[ignore = "needs RAWMAKASE_TEST_MODEL and an ONNX Runtime library (RAWMAKASE_ORT_LIB)"]
fn a_raised_flag_cancels_before_and_during_a_run() {
    let Some(model) = model_path() else {
        panic!("set RAWMAKASE_TEST_MODEL to the model file");
    };
    let mut session = Subject::load(&model).expect("load the runtime and the model");
    let image = synthetic(768, 512);

    let raised = AtomicBool::new(true);
    assert_eq!(
        session.select(&image, &raised),
        Err(InferenceError::Cancelled)
    );

    let cancel = AtomicBool::new(false);
    let started = Instant::now();
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(Duration::from_millis(250));
            cancel.store(true, Ordering::Relaxed);
        });
        session.select(&image, &cancel)
    });
    eprintln!("cancelled after {:?}", started.elapsed());
    assert_eq!(result, Err(InferenceError::Cancelled));

    // The session stays usable after a terminated run.
    let cancel = AtomicBool::new(false);
    assert!(session.select(&image, &cancel).is_ok());
}
