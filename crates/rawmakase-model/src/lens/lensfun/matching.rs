//! Which Lensfun lens a photo was taken with.
//!
//! The camera is found by the make and model the file itself writes (not
//! RAWmakase's names for them), ignoring case and spacing, as Lensfun's own entries
//! are those EXIF strings. Its mount, with the mounts Lensfun says fit it, and its crop
//! factor decide which lenses can be on it: a lens calibrated on a camera of a
//! larger crop factor (by more than 4%) doesn't cover the photo. A fixed-lens camera
//! has its own mount and needs no lens name.
//!
//! A lens name matches a Lensfun lens when every word of it is among the lens's
//! words (makers' names left out), they state the same generation, focal lengths and
//! aperture, a third-party maker it names is the lens's, and the photo's focal length
//! is within the lens's range. The best of them is the one whose name the photo's
//! explains most of. It is suggested only when it is the one best, and the photo's
//! name says more than numbers ("24-70mm" fits too many lenses to pick one); RAWmakase
//! never applies it unasked.
use super::names::{Stated, THIRD_PARTY, maker_words, same_generation, same_text, stated, words};
use super::{Camera, Database, Lens};

/// What the matcher reads of a photo.
#[derive(Clone, Copy, Debug, Default)]
pub struct Photo<'a> {
    /// The make and model as the file writes them (EXIF Make and Model).
    pub make: &'a str,
    pub model: &'a str,
    pub lens: &'a str,
    /// Focal length (mm) and its 35 mm equivalent, 0 when unknown.
    pub focal: f32,
    pub focal_35mm: f32,
    /// The photo frame's width and height (the camera's default crop), in pixels.
    pub frame: [f32; 2],
    /// The crop mode's crop factor over the sensor's, when the camera says
    /// (`Metadata::sensor_crop`).
    pub sensor_crop: Option<f32>,
}

/// How a photo matched.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// One lens fits best.
    Unique(usize),
    /// Several fit equally, or the name is too bare to pick: indexes into the
    /// database's lenses, best first.
    Ambiguous(Vec<usize>),
    /// The camera is known but no lens fits its name.
    NoMatch,
    /// The photo records no lens name.
    NoLensName,
    /// Lensfun doesn't know the camera.
    UnknownCamera,
}

/// A photo matched against the database.
#[derive(Clone, Debug)]
pub struct Matched<'d> {
    pub camera: Option<&'d Camera>,
    /// The photo frame's crop factor: the camera's (for a readout of another aspect
    /// ratio, that readout's), else from the 35 mm equivalent focal length.
    pub crop: Option<f32>,
    pub outcome: Outcome,
}

/// The camera entry for a file's EXIF make and model; of several (Lensfun's variants
/// for other aspect ratios), the one nearest the frame's aspect ratio.
pub fn camera<'d>(
    db: &'d Database,
    make: &str,
    model: &str,
    frame: [f32; 2],
) -> Option<&'d Camera> {
    let aspect = aspect_of(frame);
    db.cameras
        .iter()
        .filter(|c| {
            same_text(&c.maker.id, make)
                && (same_text(&c.model.id, model)
                    // RAWmakase's names leave the maker out of the model.
                    || same_text(&c.model.id, &format!("{make} {model}")))
        })
        .min_by(|a, b| {
            // An entry for another aspect ratio (Lensfun's variants) is worse than
            // one without any.
            let off = |c: &Camera| match (aspect, c.aspect) {
                (Some(x), Some(y)) => (x - y).abs(),
                (Some(_), None) => 0.01,
                (None, _) => 0.,
            };
            off(a).total_cmp(&off(b))
        })
}

fn aspect_of(frame: [f32; 2]) -> Option<f32> {
    let [w, h] = frame;
    (w > 0. && h > 0.).then(|| w.max(h) / w.min(h))
}

/// The aspect ratio of `camera`'s sensor: Lensfun's, else 4:3 for a (Micro) Four
/// Thirds mount, whose sensors all are; `None` when neither tells.
pub fn native_aspect(camera: &Camera) -> Option<f32> {
    camera.aspect.or_else(|| {
        let mount = camera.mount.to_lowercase();
        (mount.contains("4/3") || mount.contains("four thirds")).then_some(4. / 3.)
    })
}

/// The crop factor of a photo frame of `frame` pixels from a camera of `crop` whose
/// sensor has the aspect ratio `aspect`: a readout of another aspect ratio (a 1:1 or
/// 16:9 mode that records only that) is the largest such rectangle on the sensor, a
/// smaller diagonal. Without the sensor's aspect ratio the frame is taken as the
/// whole sensor, as Lensfun takes the image it is given.
pub fn frame_crop(crop: f32, aspect: Option<f32>, frame: [f32; 2]) -> f32 {
    let Some(native) = aspect.map(|a| a.max(1.)) else {
        return crop;
    };
    let Some(shot) = aspect_of(frame) else {
        return crop;
    };
    if (shot / native - 1.).abs() < 0.02 {
        return crop;
    }
    // Long and short edges of the sensor, for a unit diagonal.
    let d = (native * native + 1.).sqrt();
    let (long, short) = (native / d, 1. / d);
    let (l, s) = if shot >= native {
        (long, long / shot)
    } else {
        (short * shot, short)
    };
    crop / l.hypot(s)
}

/// The crop factor the 35 mm equivalent focal length gives, for a camera Lensfun
/// doesn't know.
pub fn exif_crop(photo: &Photo) -> Option<f32> {
    (photo.focal > 0. && photo.focal_35mm > 0.)
        .then(|| photo.focal_35mm / photo.focal)
        .filter(|c| (0.1..=20.).contains(c))
}

/// Whether `lens` can be on `camera`: a mount in common with the camera's own and
/// those Lensfun says fit it.
pub fn fits_mount(db: &Database, camera: &Camera, lens: &Lens) -> bool {
    let compat = db
        .mount(&camera.mount)
        .map(|m| m.compat.as_slice())
        .unwrap_or(&[]);
    lens.mounts.iter().any(|m| {
        m.eq_ignore_ascii_case(&camera.mount) || compat.iter().any(|c| c.eq_ignore_ascii_case(m))
    })
}

/// The lenses a photo may choose from by hand: those that fit its camera's mount and
/// that cover its frame, or, for a camera Lensfun doesn't know, every lens covering a
/// frame of the crop factor the photo's EXIF gives. Sorted by maker and model.
pub fn choices(db: &Database, photo: &Photo) -> (Option<f32>, Vec<usize>) {
    let camera = camera(db, photo.make, photo.model, photo.frame);
    let crop = crop_for(camera, photo);
    let mut out: Vec<usize> = db
        .lenses
        .iter()
        .enumerate()
        .filter(|(_, l)| camera.is_none_or(|c| fits_mount(db, c, l)))
        .filter(|(_, l)| crop.is_some_and(|crop| l.calibration.covers(crop)))
        .map(|(i, _)| i)
        .collect();
    out.sort_by(|a, b| {
        let (a, b) = (&db.lenses[*a], &db.lenses[*b]);
        (a.maker.shown.to_lowercase(), a.model.shown.to_lowercase())
            .cmp(&(b.maker.shown.to_lowercase(), b.model.shown.to_lowercase()))
            .then(a.calibration.crop.total_cmp(&b.calibration.crop))
    });
    (crop, out)
}

fn crop_for(camera: Option<&Camera>, photo: &Photo) -> Option<f32> {
    match camera {
        Some(c) => Some(photo_crop(c.crop, native_aspect(c), photo)),
        None => exif_crop(photo),
    }
}

/// The crop factor of `photo`'s frame from a camera of `crop` with a sensor of
/// `aspect` (`frame_crop`), in a crop mode the camera records (Canon's APS-C mode)
/// that much larger; else, where the 35 mm equivalent focal length disagrees by more
/// than rounding (10%) with both the frame's and the sensor's crop factors, the crop
/// factor that gives (Nikon's DX and Sony's APS-C modes write it so).
pub fn photo_crop(crop: f32, aspect: Option<f32>, photo: &Photo) -> f32 {
    let frame = frame_crop(crop, aspect, photo.frame);
    if let Some(k) = photo.sensor_crop.filter(|k| k.is_finite() && *k >= 1.) {
        return frame * k;
    }
    // A 35 mm equivalent of the whole sensor (as many cameras record it for a
    // readout) says nothing of a crop mode.
    match exif_crop(photo) {
        Some(exif) if (exif / frame - 1.).abs() > 0.1 && (exif / crop - 1.).abs() > 0.1 => exif,
        _ => frame,
    }
}

/// The lens `photo` was taken with, as far as the database tells.
pub fn find<'d>(db: &'d Database, photo: &Photo) -> Matched<'d> {
    let camera = camera(db, photo.make, photo.model, photo.frame);
    let crop = crop_for(camera, photo);
    let Some(cam) = camera else {
        return Matched {
            camera,
            crop,
            outcome: Outcome::UnknownCamera,
        };
    };
    let crop_value = crop.unwrap_or(cam.crop);
    let fitting = db
        .lenses
        .iter()
        .enumerate()
        .filter(|(_, l)| fits_mount(db, cam, l) && l.calibration.covers(crop_value));
    if cam.fixed_lens() {
        let own: Vec<usize> = fitting
            .filter(|(_, l)| l.mounts.iter().any(|m| m.eq_ignore_ascii_case(&cam.mount)))
            .map(|(i, _)| i)
            .collect();
        let outcome = match own.as_slice() {
            [] => Outcome::NoMatch,
            [one] => Outcome::Unique(*one),
            _ => Outcome::Ambiguous(own),
        };
        return Matched {
            camera,
            crop,
            outcome,
        };
    }
    if photo.lens.trim().is_empty() {
        return Matched {
            camera,
            crop,
            outcome: Outcome::NoLensName,
        };
    }
    let mut scored: Vec<(usize, f32)> = fitting
        .filter_map(|(i, l)| score(cam, l, photo).map(|s| (i, s)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let outcome = match scored.as_slice() {
        [] => Outcome::NoMatch,
        [(best, s), rest @ ..]
            if !bare(photo.lens, cam) && rest.first().is_none_or(|r| r.1 < s - 1e-6) =>
        {
            Outcome::Unique(*best)
        }
        all => Outcome::Ambiguous(all.iter().map(|(i, _)| *i).collect()),
    };
    Matched {
        camera,
        crop,
        outcome,
    }
}

/// A name of numbers alone ("24-70mm", "17-50mm f/2.8"), makers' names aside
/// ("Canon 50mm f/1.8" says no more than "50mm f/1.8").
fn bare(name: &str, camera: &Camera) -> bool {
    let camera_maker = maker_words(&camera.maker.id);
    words(name)
        .iter()
        .filter(|w| {
            !camera_maker.contains(w)
                && !THIRD_PARTY.contains(&w.as_str())
                && !MAKERS.contains(&w.as_str())
        })
        .all(|w| w == "mm" || w.chars().all(|c| c.is_ascii_digit() || c == '.'))
}
/// Camera makers' names, as lens names may carry them.
const MAKERS: [&str; 16] = [
    "canon",
    "nikon",
    "sony",
    "pentax",
    "olympus",
    "panasonic",
    "lumix",
    "leica",
    "fujifilm",
    "fuji",
    "samsung",
    "minolta",
    "konica",
    "ricoh",
    "hasselblad",
    "om",
];

/// How much of `lens`'s name `photo`'s lens name explains (0–1), or `None` when it
/// can't be that lens.
pub fn score(camera: &Camera, lens: &Lens, photo: &Photo) -> Option<f32> {
    let lens_maker = maker_words(&lens.maker.id);
    let camera_maker = maker_words(&camera.maker.id);
    let lens_words: Vec<String> = words(&lens.model.id)
        .into_iter()
        .filter(|w| !lens_maker.contains(w))
        .collect();
    let exif: Vec<String> = words(photo.lens)
        .into_iter()
        .filter(|w| !lens_maker.contains(w) && !camera_maker.contains(w))
        .collect();
    if exif.is_empty() {
        return None;
    }
    // A third-party maker the photo names is the lens's.
    if exif.iter().any(|w| THIRD_PARTY.contains(&w.as_str())) {
        return None;
    }
    // Every word of the photo's name, as many times as it has it.
    let mut left = lens_words.clone();
    for w in &exif {
        let i = left.iter().position(|l| l == w)?;
        left.swap_remove(i);
    }
    if !same_generation(&exif, &lens_words) {
        return None;
    }
    let (theirs, ours): (Stated, Stated) = (stated(&lens.model.id), stated(photo.lens));
    if !theirs.agree(&ours) {
        return None;
    }
    let [lo, hi] = lens.calibration.focal;
    if photo.focal > 0. && lo > 0. && !(lo * 0.97 - 0.5..=hi * 1.03 + 0.5).contains(&photo.focal) {
        return None;
    }
    Some(exif.len() as f32 / lens_words.len().max(1) as f32)
}
