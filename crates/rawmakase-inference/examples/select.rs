//! Command-line probe: `select MODEL INPUT.ppm OUTPUT.pgm [THREADS] [REPEAT]`.
//!
//! Reads a binary PPM (P6, 8-bit), prints the load and per-run timings and
//! writes the coverage as a binary PGM (P5). The ONNX Runtime library comes from
//! `RAWMAKASE_ORT_LIB` or the package locations.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use rawmakase_inference::{LoadOptions, RgbImage, Subject};

fn read_ppm(path: &Path) -> Result<RgbImage, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let mut fields = Vec::new();
    let mut pos = 0;
    while fields.len() < 4 {
        while bytes.get(pos).is_some_and(u8::is_ascii_whitespace) {
            pos += 1;
        }
        let start = pos;
        while bytes.get(pos).is_some_and(|b| !b.is_ascii_whitespace()) {
            pos += 1;
        }
        fields.push(String::from_utf8_lossy(&bytes[start..pos]).into_owned());
    }
    pos += 1;
    if fields[0] != "P6" || fields[3] != "255" {
        return Err("expected an 8-bit binary PPM (P6)".into());
    }
    let width: usize = fields[1].parse().map_err(|_| "bad width")?;
    let height: usize = fields[2].parse().map_err(|_| "bad height")?;
    Ok(RgbImage {
        width,
        height,
        data: bytes[pos..].to_vec(),
    })
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        return Err("usage: select MODEL INPUT.ppm OUTPUT.pgm [THREADS] [REPEAT]".into());
    }
    let threads = args.get(4).and_then(|t| t.parse().ok());
    let repeat: usize = args.get(5).and_then(|t| t.parse().ok()).unwrap_or(1);
    let image = read_ppm(Path::new(&args[2]))?;
    let started = Instant::now();
    let mut session = Subject::load_with(
        &PathBuf::from(&args[1]),
        &LoadOptions {
            runtime_library: None,
            threads,
        },
    )
    .map_err(|e| e.to_string())?;
    println!("load {:?}", started.elapsed());
    let cancel = AtomicBool::new(false);
    let mut coverage = None;
    for run in 0..repeat {
        let started = Instant::now();
        coverage = Some(session.select(&image, &cancel).map_err(|e| e.to_string())?);
        println!("select #{run} {:?}", started.elapsed());
    }
    let coverage = coverage.ok_or("no run")?;
    let mut out = format!("P5\n{} {}\n255\n", coverage.width, coverage.height).into_bytes();
    out.extend_from_slice(&coverage.data);
    std::fs::write(&args[3], out).map_err(|e| e.to_string())
}
