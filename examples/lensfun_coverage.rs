//! Which Lensfun lens each photo of a list matches, by RAWmakase's matcher: for the
//! coverage figures in docs/lens-corrections.md. Reads a Lensfun `data/db` folder and
//! a tab-separated list (id, EXIF make, EXIF model, lens, focal length, aperture, and
//! optionally the frame's width and height and the 35 mm equivalent focal length) on
//! stdin; writes id, outcome, camera crop factor, the matched lenses and the
//! corrections the first one gives the photo (D, C, V).
//!
//!     cargo run --release --example lensfun_coverage -- path/to/lensfun/data/db < photos.tsv
use rawmakase::lens::lensfun::installed::read_folder;
use rawmakase::lens::lensfun::matching::{Outcome, Photo, find};
use rawmakase::optics::lensfun::{DISTANCE, Frame, Shot};
use std::io::BufRead;

fn main() -> anyhow::Result<()> {
    let dir = std::env::args().nth(1).expect("the Lensfun data/db folder");
    let db = read_folder(std::path::Path::new(&dir))?;
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 6 {
            continue;
        }
        let number = |i: usize| f.get(i).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.);
        let frame = match (number(6), number(7)) {
            (w, h) if w > 0. && h > 0. => [w, h],
            _ => [6000., 4000.],
        };
        let photo = Photo {
            make: f[1],
            model: f[2],
            lens: f[3],
            focal: number(4),
            focal_35mm: number(8),
            frame,
            sensor_crop: None,
        };
        let m = find(&db, &photo);
        let (kind, lenses) = match &m.outcome {
            Outcome::Unique(i) => ("unique", vec![*i]),
            Outcome::Ambiguous(all) => ("ambiguous", all.clone()),
            Outcome::NoMatch => ("no-match", vec![]),
            Outcome::NoLensName => ("no-lens-name", vec![]),
            Outcome::UnknownCamera => ("unknown-camera", vec![]),
        };
        let gives = lenses.first().map_or(String::new(), |i| {
            let crop = m.crop.unwrap_or(1.);
            let shot = Shot {
                focal: photo.focal,
                aperture: number(5),
                distance: DISTANCE,
            };
            let frame = Frame {
                size: frame,
                rect: [0., 0., frame[0], frame[1]],
                crop,
            };
            let e = db.lenses[*i].calibration.evaluate(&frame, &shot, "");
            e.correction.map_or("-".into(), |c| {
                let mut s = String::new();
                if c.distortion.is_some() {
                    s.push('D');
                }
                if c.chromatic.is_some() {
                    s.push('C');
                }
                if c.vignetting.is_some() {
                    s.push('V');
                }
                s
            })
        });
        let names: Vec<String> = lenses
            .iter()
            .map(|i| format!("{}|{}", db.lenses[*i].maker.id, db.lenses[*i].model.id))
            .collect();
        println!(
            "{}\t{kind}\t{}\t{gives}\t{}",
            f[0],
            m.camera.map_or(0., |c| c.crop),
            names.join("\t")
        );
    }
    Ok(())
}
