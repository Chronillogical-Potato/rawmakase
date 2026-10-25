//! Maker lens-correction tables, read directly from the RAW container.
//!
//! - Fujifilm RAF: FujiIFD tags 0xF00B (distortion), 0xF00F (lateral CA) and
//!   0xF010 (vignetting), signed rationals. The first value is a pixel scale; knots
//!   are fractions of the half diagonal, distortion is in percent, CA a radius
//!   fraction, vignetting the remaining illumination in percent.
//! - Sony ARW: raw SubIFD tags 0x7032 (vignetting), 0x7035 (CA) and 0x7037
//!   (distortion), signed shorts led by the number of values and padded with zeros
//!   to a fixed length (ZV-1: 11 distortion knots in 16 slots). Knots are spread
//!   evenly from the centre to the corner. Distortion is in 2^-14, CA in 2^-21
//!   units; vignetting v restores 2^(2^(v/8192 - 1) - 0.5).
//! - Olympus and OM System ORF: the maker note's ImageProcessing directory (0x2040),
//!   tags 0x150A (distortion: k1, k2, k3 and a scale s, floats) and 0x150C (lateral
//!   CA: three coefficients for red, then three for blue, floats). With radius 1 at
//!   the half diagonal of the camera's default crop, an output radius r samples
//!   green at s·r·(1 + k1 (s·r)² + k2 (s·r)⁴ + k3 (s·r)⁶), and red and blue at that
//!   radius times 1 + c0 + c1 ρ² + c2 ρ⁴, ρ being green's source radius.
//! - Panasonic RW2: IFD0 tag 0x0119, sixteen signed shorts (ExifTool's
//!   DistortionInfo). Value 7's low four bits are 1 when the correction applies,
//!   value 12 is the radius unit N in pixels, and values 8, 4 and 11 over 2^15 are
//!   a, b and c of the camera's own correction, source radius s to output radius
//!   s·(1 + a s² + b s⁴ + c s⁶). Value 5 scales it: the output radius r samples the
//!   source radius that this maps to r·(1 + v5/2^15).
//! - Panasonic RW2 lateral CA: IFD0 tag 0x011B, 32 signed shorts. Value 14 is 256 when
//!   the table holds data (0 for lenses without). Values 17, 16, 4 and 11 are four
//!   source radii in pixels, the last about the half diagonal; values 12, 23, 8 and 26
//!   (red) and 27, 10, 20 and 29 (blue) are the channel's displacement from green at
//!   those radii in −1/1000 px, so red and blue sample green's source radius plus it.
//!   The other values, many of which change from shot to shot, are not used.
//!
//! The Fujifilm vignetting agrees with the FixVignetteRadial opcode Adobe writes into
//! DNGs of the same file to within 1%, and the Olympus and Panasonic distortion and
//! CA with the WarpRectilinear opcodes (and, for newer bodies, the embedded lens
//! profiles) Adobe DNG Converter 18.7 writes, to within a pixel (see
//! docs/lens-corrections.md).
use crate::camera_data::Metadata;
use crate::optics::{LensCorrection, Radial};
use crate::tiff::{Entry, Tiff, kind};
use std::{fs::File, io::Read, path::Path};

/// Returns `None` for unsupported files or when the camera stored no corrections.
/// `m` is the decoded image the radii are fractions of: its size and default crop.
pub fn read(path: &Path, m: &Metadata) -> Option<LensCorrection> {
    let mut f = File::open(path).ok()?;
    let mut head = [0u8; 108];
    f.read_exact(&mut head).ok()?;
    let c = if head.starts_with(b"FUJIFILMCCD-RAW") {
        let base = u32::from_be_bytes(head[100..104].try_into().ok()?) as u64;
        fuji(&mut Tiff::open(f, base)?)
    } else if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
        sony(&mut Tiff::open(f, 0)?)
    } else if [b"IIRO", b"IIRS", b"MMOR"]
        .iter()
        .any(|h| head.starts_with(*h))
    {
        olympus(path, &mut Tiff::open(f, 0)?, m)
    } else if head.starts_with(b"IIU\0") {
        panasonic(&mut Tiff::open(f, 0)?, m)
    } else {
        None
    }?;
    (!c.is_empty() && c.validate()).then_some(c)
}

const FUJI_VIGNETTE_STRENGTH: f32 = 0.85;

fn fuji(t: &mut Tiff) -> Option<LensCorrection> {
    let ifd0 = t.ifd(t.first)?;
    let fuji = t.ifd(t.offset(ifd0.get(&0xf000)?)?)?;
    let mut numbers = |tag| fuji.get(&tag).and_then(|e| t.numbers(e));
    let distortion = numbers(0xf00b);
    let chromatic = numbers(0xf00f);
    let vignetting = numbers(0xf010);
    // [scale, n knots, n values]
    let pairs = |v: &[f32], f: &dyn Fn(f32) -> f32| {
        let n = v.len().checked_sub(1)? / 2;
        (n >= 2 && v.len() == 2 * n + 1).then(|| {
            Radial::new(
                v[1..=n].to_vec(),
                v[n + 1..].iter().map(|x| f(*x)).collect(),
            )
        })?
    };
    let vignetting = vignetting
        // Lightroom renders Fujifilm's table about 15% weaker in log gain: corners of
        // three X100F photos match Camera Raw at exponent 0.85 (+0.08 EV at 1).
        .and_then(|v| pairs(&v, &|p| (100. / p).powf(FUJI_VIGNETTE_STRENGTH)))
        .filter(|r| r.values.iter().any(|v| (v - 1.).abs() > 1e-4));
    let distortion = distortion
        .and_then(|v| pairs(&v, &|p| 1. + p / 100.))
        .filter(|r| r.values.iter().any(|v| (v - 1.).abs() > 1e-6));
    // [scale, n knots, n red, n blue]
    let chromatic = chromatic.and_then(|v| {
        let n = v.len().checked_sub(1)? / 3;
        if n < 2 || v.len() != 3 * n + 1 {
            return None;
        }
        let knots = v[1..=n].to_vec();
        let red = Radial::new(
            knots.clone(),
            v[n + 1..=2 * n].iter().map(|x| 1. + x).collect(),
        )?;
        let blue = Radial::new(knots, v[2 * n + 1..].iter().map(|x| 1. + x).collect())?;
        [&red, &blue]
            .iter()
            .any(|r| r.values.iter().any(|v| (v - 1.).abs() > 1e-7))
            .then_some([red, blue])
    });
    Some(LensCorrection {
        source: "Fujifilm built-in".into(),
        default_on: true,
        vignetting,
        distortion,
        chromatic,
        gain_map: None,
    })
}

fn sony(t: &mut Tiff) -> Option<LensCorrection> {
    let ifd0 = t.ifd(t.first)?;
    let sub = t.ifd(t.offset(ifd0.get(&0x14a)?)?)?;
    let mut numbers = |tag| sub.get(&tag).and_then(|e| t.numbers(e));
    let vignetting = numbers(0x7032);
    let chromatic = numbers(0x7035);
    let distortion = numbers(0x7037);
    let knots = |n: usize| {
        (0..n)
            .map(|i| i as f32 / (n - 1) as f32)
            .collect::<Vec<_>>()
    };
    // The count's values, from storage that may be longer: a ZV-1 stores 11 distortion
    // knots in 16 slots. Anything stored past the count must be zero padding.
    let counted = |v: Option<Vec<f32>>, per_count: usize| {
        let v = v?;
        let count = *v.first()? as usize;
        let values = v.get(1..=count)?;
        (count >= 2 * per_count
            && count.is_multiple_of(per_count)
            && v[count + 1..].iter().all(|x| *x == 0.))
        .then(|| values.to_vec())
    };
    let single = |v: Option<Vec<f32>>, f: &dyn Fn(f32) -> f32| {
        let v = counted(v, 1)?;
        Radial::new(knots(v.len()), v.iter().map(|x| f(*x)).collect())
    };
    let vignetting = single(vignetting, &|v| (2f32.powf(v / 8192. - 1.) - 0.5).exp2())
        .filter(|r| r.values.iter().any(|v| (v - 1.).abs() > 1e-4));
    let distortion = single(distortion, &|d| 1. + d / 16384.)
        .filter(|r| r.values.iter().any(|v| (v - 1.).abs() > 1e-6));
    let chromatic = counted(chromatic, 2).and_then(|v| {
        let n = v.len() / 2;
        let scale = |s: &[f32]| s.iter().map(|x| 1. + x / 2_097_152.).collect();
        Some([
            Radial::new(knots(n), scale(&v[..n]))?,
            Radial::new(knots(n), scale(&v[n..]))?,
        ])
    });
    Some(LensCorrection {
        source: LensCorrection::SONY_BUILTIN.into(),
        // Not yet compared with Lightroom, which may apply its own profile instead.
        default_on: false,
        vignetting,
        distortion,
        chromatic,
        gain_map: None,
    })
}

/// Radius samples from the centre to the decoded image's corner.
fn knots() -> Vec<f32> {
    (0..=32).map(|i| i as f32 / 32.).collect()
}

/// A maker's radial model in its own radius unit, `unit` pixels, sampled at the
/// decoded image's radii: `green` maps an output radius to green's source radius,
/// `chromatic` red's and blue's scale relative to green at green's source radius.
fn maker_model(
    m: &Metadata,
    unit: f64,
    green: &dyn Fn(f64) -> Option<f64>,
    chromatic: Option<[&dyn Fn(f64) -> f64; 2]>,
) -> (Option<Radial>, Option<[Radial; 2]>) {
    let half = f64::from(m.width).hypot(f64::from(m.height)) * 0.5;
    if !(half > 0. && unit > 0.) {
        return (None, None);
    }
    // The ratio's limit at the centre is its value just beside it.
    let x = |r: f32| (f64::from(r) * half / unit).max(1e-4);
    let sources: Option<Vec<f64>> = knots().into_iter().map(|r| green(x(r))).collect();
    let Some(sources) = sources.filter(|s| s.windows(2).all(|p| p[0] < p[1])) else {
        return (None, None);
    };
    let curve = |f: &dyn Fn(usize, f64) -> f64| {
        let values = (knots().into_iter().enumerate())
            .map(|(i, r)| f(i, x(r)) as f32)
            .collect();
        Radial::new(knots(), values)
    };
    let changes = |r: &Radial, by: f32| r.values.iter().any(|v| (v - 1.).abs() > by);
    let distortion = curve(&|i, x| sources[i] / x).filter(|r| changes(r, 1e-6));
    let chromatic = chromatic
        .and_then(|[red, blue]| {
            Some([
                curve(&|i, _| red(sources[i]))?,
                curve(&|i, _| blue(sources[i]))?,
            ])
        })
        .filter(|[a, b]| changes(a, 1e-7) || changes(b, 1e-7));
    (distortion, chromatic)
}

/// The values of a FLOAT entry.
fn floats(t: &mut Tiff, e: &Entry) -> Option<Vec<f64>> {
    if e.kind != kind::FLOAT {
        return None;
    }
    let b = t.raw(e)?;
    let v: Vec<f64> = (b.as_chunks::<4>().0.iter())
        .map(|c| f64::from(f32::from_bits(t.u32(c))))
        .collect();
    v.iter().all(|x| x.is_finite()).then_some(v)
}

fn olympus(path: &Path, t: &mut Tiff, m: &Metadata) -> Option<LensCorrection> {
    let ifd0 = t.ifd(t.first)?;
    let exif = t.ifd(t.offset(ifd0.get(&0x8769)?)?)?;
    let note = exif.get(&0x927c)?;
    let at = t.u32(&note.value) as u64;
    // Offsets in the note count from its start, after its own byte order mark.
    let head = t.bytes(at, 16)?;
    let (start, source) = if head.starts_with(b"OLYMPUS\0") {
        (8, "Olympus built-in")
    } else if head.starts_with(b"OM SYSTEM\0\0\0") {
        (12, "OM System built-in")
    } else {
        return None;
    };
    let little = match &head[start..start + 2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let mut n = Tiff::headless(File::open(path).ok()?, at, little);
    let main = n.ifd(start as u64 + 4)?;
    let e = main.get(&0x2040)?;
    let processing = match e.kind {
        // Older notes store the directory inline.
        kind::UNDEFINED => n.u32(&e.value) as u64,
        _ => n.offset(e)?,
    };
    let ip = n.ifd(processing)?;
    let d = floats(&mut n, ip.get(&0x150a)?).filter(|d| d.len() == 4)?;
    let ca: Option<[f64; 6]> = ip
        .get(&0x150c)
        .and_then(|e| floats(&mut n, e))
        .and_then(|c| c.try_into().ok());
    let s = if d[3] > 0. { d[3] } else { 1. };
    let k = [d[0], d[1], d[2]];
    let green = |r: f64| {
        let x = s * r;
        let x2 = x * x;
        Some(x * (1. + x2 * (k[0] + x2 * (k[1] + x2 * k[2]))))
    };
    // Red's and blue's scale at green's source radius ρ.
    let scale = |at: usize, rho: f64| {
        ca.map_or(1., |c| {
            1. + c[at] + rho * rho * (c[at + 1] + rho * rho * c[at + 2])
        })
    };
    let (red, blue) = (|rho| scale(0, rho), |rho| scale(3, rho));
    let chromatic = ca.is_some().then_some([&red as &dyn Fn(f64) -> f64, &blue]);
    let (w, h) = match (m.crop_width, m.crop_height) {
        (0, _) | (_, 0) => (m.width, m.height),
        size => size,
    };
    let unit = f64::from(w).hypot(f64::from(h)) * 0.5;
    let (distortion, chromatic) = maker_model(m, unit, &green, chromatic);
    Some(LensCorrection {
        source: source.into(),
        // Adobe applies it as the photo's lens profile: written into its DNGs as
        // their warp, or for newer bodies as the "Camera Settings" profile.
        default_on: true,
        vignetting: None,
        distortion,
        chromatic,
        gain_map: None,
    })
}

/// The values of an IFD0 entry of `n` signed shorts.
fn shorts<const N: usize>(t: &mut Tiff, e: &Entry) -> Option<[f64; N]> {
    let b = t.raw(e)?;
    let v: Vec<f64> = (b.get(..2 * N)?.as_chunks::<2>().0.iter())
        .map(|c| f64::from(t.u16(c) as i16))
        .collect();
    v.try_into().ok()
}

/// Panasonic's lateral CA (IFD0 0x011B) as red's and blue's scale relative to green,
/// each a function of green's source radius in pixels. `half` is the decoded image's
/// half diagonal.
fn panasonic_ca(v: [f64; 32], half: f64) -> Option<[impl Fn(f64) -> f64; 2]> {
    let knots = [v[17], v[16], v[4], v[11]];
    if v[14] != 256. || knots[0] <= 0. || !knots.is_sorted_by(|a, b| a < b) || half <= 0. {
        return None;
    }
    let at = |i: [usize; 4]| i.map(|i| -v[i] / 1000.);
    let (red, blue) = (at([12, 23, 8, 26]), at([27, 10, 20, 29]));
    if red.iter().chain(&blue).all(|d| *d == 0.) {
        return None;
    }
    // Displacement in pixels at a source radius: straight lines between the knots,
    // from none at the centre, held past the last knot.
    let lines = |d: [f64; 4], g: f64| {
        let i = knots.iter().position(|k| g < *k).unwrap_or(4);
        let (k0, d0) = if i == 0 {
            (0., 0.)
        } else {
            (knots[i - 1], d[i - 1])
        };
        match knots.get(i) {
            Some(k1) => d0 + (d[i] - d0) * (g - k0) / (k1 - k0),
            None => d0,
        }
    };
    // Adobe applies a smooth fit of those lines in its warp's form, 1 + c0 + c1 s² +
    // c2 s⁴ + c3 s⁶ with s = g / half. Least squares with residuals weighted by s, out
    // to the half diagonal, matches the CA in its DNGs of 309 photos from 111 bodies
    // to within 0.15 px (median 0.015 px); the lines themselves differ by up to 0.86 px.
    let fit = |d: [f64; 4]| {
        let mut normal = [[0.; 5]; 4];
        for i in 1..=256 {
            let s = f64::from(i) / 256.;
            let s2 = s * s;
            let row = [s, s * s2, s * s2 * s2, s * s2 * s2 * s2];
            let y = lines(d, s * half) / half;
            for (j, n) in normal.iter_mut().enumerate() {
                for k in 0..4 {
                    n[k] += row[j] * row[k];
                }
                n[4] += row[j] * y;
            }
        }
        let c = solve4(normal)?;
        Some(move |g: f64| {
            let s2 = (g / half).powi(2);
            1. + c[0] + s2 * (c[1] + s2 * (c[2] + s2 * c[3]))
        })
    };
    Some([fit(red)?, fit(blue)?])
}

/// Solves the 4×4 system in the augmented matrix `m` by Gaussian elimination with
/// partial pivoting, or `None` when it is singular.
fn solve4(mut m: [[f64; 5]; 4]) -> Option<[f64; 4]> {
    for col in 0..4 {
        let pivot = (col..4).max_by(|a, b| m[*a][col].abs().total_cmp(&m[*b][col].abs()))?;
        m.swap(col, pivot);
        if m[col][col].abs() < 1e-300 {
            return None;
        }
        let top = m[col];
        for row in &mut m[col + 1..] {
            let f = row[col] / top[col];
            for (x, t) in row.iter_mut().zip(top).skip(col) {
                *x -= f * t;
            }
        }
    }
    let mut x = [0.; 4];
    for row in (0..4).rev() {
        let rest: f64 = (row + 1..4).map(|k| m[row][k] * x[k]).sum();
        x[row] = (m[row][4] - rest) / m[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

fn panasonic(t: &mut Tiff, m: &Metadata) -> Option<LensCorrection> {
    let ifd0 = t.ifd(t.first)?;
    let info = ifd0.get(&0x0119).and_then(|e| shorts::<16>(t, e));
    let ca = ifd0.get(&0x011b).and_then(|e| shorts::<32>(t, e));
    let half = f64::from(m.width).hypot(f64::from(m.height)) * 0.5;
    let ca = ca.and_then(|v| panasonic_ca(v, half));
    // Value 7's low four bits switch the distortion correction on.
    let info = info.filter(|v| (v[7] as i16 as u16) & 0xf == 1 && v[12] > 0.);
    // Radii in the distortion model's unit, or in pixels without it.
    let unit = info.map_or(1., |v| v[12]);
    let [a, b, c] = info.map_or([0.; 3], |v| [v[8], v[4], v[11]].map(|x| x / 32768.));
    let scale = info.map_or(1., |v| 1. + v[5] / 32768.);
    // The camera's correction maps the source radius to the output radius; its
    // inverse, by Newton's method, is green's source radius for an output radius.
    let forward = |s: f64| {
        let s2 = s * s;
        s * (1. + s2 * (a + s2 * (b + s2 * c)))
    };
    let slope = |s: f64| {
        let s2 = s * s;
        1. + s2 * (3. * a + s2 * (5. * b + s2 * 7. * c))
    };
    let green = |r: f64| {
        let target = r * scale;
        let mut s = target;
        for _ in 0..50 {
            let d = slope(s);
            if d <= 0. {
                return None;
            }
            s -= (forward(s) - target) / d;
        }
        ((forward(s) - target).abs() < 1e-9 && s > 0.).then_some(s)
    };
    // The CA applies at green's source radius, in the sensor's pixels.
    let [red, blue] = match &ca {
        Some([red, blue]) => [Some(red), Some(blue)],
        None => [None, None],
    };
    let red = |rho: f64| red.map_or(1., |f| f(rho * unit));
    let blue = |rho: f64| blue.map_or(1., |f| f(rho * unit));
    let chromatic = ca.is_some().then_some([&red as &dyn Fn(f64) -> f64, &blue]);
    if info.is_none() && chromatic.is_none() {
        return None;
    }
    let (distortion, chromatic) = maker_model(m, unit, &green, chromatic);
    let make = ifd0
        .get(&0x010f)
        .and_then(|e| t.raw(e))
        .map(|b| String::from_utf8_lossy(&b).to_ascii_lowercase())
        .unwrap_or_default();
    let source = if make.starts_with("leica") {
        "Leica built-in"
    } else {
        "Panasonic built-in"
    };
    Some(LensCorrection {
        source: source.into(),
        default_on: true,
        vignetting: None,
        distortion,
        chromatic,
        gain_map: None,
    })
}

#[cfg(test)]
mod tests {
    use crate::camera_data::Metadata;
    use crate::optics::LensCorrection;
    use std::path::Path;
    /// The correction of a file whose radii need no image size (Fujifilm, Sony).
    fn read(p: &Path) -> Option<LensCorrection> {
        super::read(p, &Metadata::default())
    }
    /// (tag, type, count, data)
    type Entry = (u16, u16, u32, Vec<u8>);
    /// Little-endian TIFF with the given directories, each pointed to by the previous one.
    fn tiff(dirs: &[Vec<Entry>]) -> Vec<u8> {
        // Directories are laid out in order; entry data follows each directory.
        let mut out = b"II*\0\x08\0\0\0".to_vec();
        for (d, entries) in dirs.iter().enumerate() {
            let start = out.len();
            let data_start = start + 2 + entries.len() * 12 + 4;
            let mut data: Vec<u8> = Vec::new();
            out.extend((entries.len() as u16).to_le_bytes());
            for (tag, kind, count, bytes) in entries {
                out.extend(tag.to_le_bytes());
                out.extend(kind.to_le_bytes());
                out.extend(count.to_le_bytes());
                if bytes.len() <= 4 {
                    let mut v = bytes.clone();
                    v.resize(4, 0);
                    out.extend(v);
                } else {
                    out.extend(((data_start + data.len()) as u32).to_le_bytes());
                    data.extend(bytes);
                }
            }
            out.extend(0u32.to_le_bytes());
            out.extend(data);
            // A pointer entry value of 0 is patched to the next directory.
            if d + 1 < dirs.len() {
                let next = out.len() as u32;
                let value = start + 2 + 8;
                out[value..value + 4].copy_from_slice(&next.to_le_bytes());
            }
        }
        out
    }
    fn shorts(v: &[i16]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_le_bytes()).collect()
    }
    fn rationals(v: &[f64]) -> Vec<u8> {
        v.iter()
            .flat_map(|x| {
                let n = (*x * 1_000_000.).round() as i32;
                [n.to_le_bytes(), 1_000_000i32.to_le_bytes()].concat()
            })
            .collect()
    }
    #[test]
    fn reads_sony_subifd_tables() {
        let mut vig = vec![16i16];
        vig.extend((0..16).map(|i| i * 800));
        let mut dist = vec![16i16];
        dist.extend((0..16).map(|i| i * 10));
        let mut ca = vec![32i16];
        ca.extend(std::iter::repeat_n(-384, 16));
        ca.extend(std::iter::repeat_n(0, 16));
        let bytes = tiff(&[
            vec![(0x14a, 4, 1, vec![0; 4])],
            vec![
                (0x7032, 8, 17, shorts(&vig)),
                (0x7035, 8, 33, shorts(&ca)),
                (0x7037, 8, 17, shorts(&dist)),
            ],
        ]);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.arw");
        std::fs::write(&p, bytes).unwrap();
        let c = read(&p).unwrap();
        assert!(!c.default_on);
        let v = c.vignetting.as_ref().unwrap();
        assert!((v.eval(0.) - 1.).abs() < 1e-6);
        let corner = 2f32.powf(2f32.powf(12000. / 8192. - 1.) - 0.5);
        assert!((v.eval(1.) - corner).abs() < 1e-5);
        assert!((c.distortion.as_ref().unwrap().eval(1.) - (1. + 150. / 16384.)).abs() < 1e-6);
        let [red, blue] = c.chromatic.as_ref().unwrap();
        assert!((red.eval(0.5) - (1. - 384. / 2_097_152.)).abs() < 1e-7);
        assert_eq!(blue.eval(0.5), 1.);
    }
    fn sony_file(
        dir: &std::path::Path,
        vig: &[i16],
        ca: &[i16],
        dist: &[i16],
    ) -> std::path::PathBuf {
        let entry = |tag, v: &[i16]| (tag, 8, v.len() as u32, shorts(v));
        let bytes = tiff(&[
            vec![(0x14a, 4, 1, vec![0; 4])],
            vec![entry(0x7032, vig), entry(0x7035, ca), entry(0x7037, dist)],
        ]);
        let p = dir.join("a.arw");
        std::fs::write(&p, bytes).unwrap();
        p
    }
    /// A ZV-1 stores fewer knots than its fixed 16 (CA: 32) slots, padded with zeros (#403).
    const ZV1_VIG: [i16; 17] = [
        16, 0, 0, 0, 15, 63, 193, 413, 715, 1107, 1576, 2126, 2749, 3443, 4523, 5509, 6639,
    ];
    const ZV1_CA: [i16; 33] = [
        22, 0, 256, 384, 128, 128, 256, 384, 640, 896, 1152, 1408, 0, 1280, 1664, 1664, 1280, 896,
        512, 256, 128, 0, 256, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    const ZV1_DIST: [i16; 17] = [
        11, 1265, 1218, 1083, 860, 562, 209, -179, -580, -972, -1353, -1725, 0, 0, 0, 0, 0,
    ];
    #[test]
    fn reads_sony_tables_padded_to_fixed_slots() {
        let d = tempfile::tempdir().unwrap();
        let c = read(&sony_file(d.path(), &ZV1_VIG, &ZV1_CA, &ZV1_DIST)).unwrap();
        let dist = c.distortion.as_ref().unwrap();
        assert_eq!(dist.knots.len(), 11);
        assert_eq!(dist.knots[1], 0.1);
        assert!((dist.eval(0.) - (1. + 1265. / 16384.)).abs() < 1e-6);
        assert!((dist.eval(1.) - (1. - 1725. / 16384.)).abs() < 1e-6);
        let [red, blue] = c.chromatic.as_ref().unwrap();
        assert_eq!(red.knots.len(), 11);
        assert!((red.eval(1.) - (1. + 1408. / 2_097_152.)).abs() < 1e-7);
        assert!((blue.eval(0.1) - (1. + 1280. / 2_097_152.)).abs() < 1e-7);
        assert!((blue.eval(1.) - (1. + 256. / 2_097_152.)).abs() < 1e-7);
        assert_eq!(c.vignetting.as_ref().unwrap().knots.len(), 16);
    }
    #[test]
    fn rejects_malformed_sony_tables() {
        fn with<const N: usize>(mut v: [i16; N], i: usize, x: i16) -> [i16; N] {
            v[i] = x;
            v
        }
        let d = tempfile::tempdir().unwrap();
        for (name, dist, ca) in [
            (
                "data past the count",
                with(ZV1_DIST, 16, 1),
                with(ZV1_CA, 32, 1),
            ),
            (
                "count past the stored values",
                with(ZV1_DIST, 0, 17),
                with(ZV1_CA, 0, 34),
            ),
            ("a single knot", with(ZV1_DIST, 0, 1), with(ZV1_CA, 0, 2)),
        ] {
            let c = read(&sony_file(d.path(), &ZV1_VIG, &ca, &dist)).unwrap();
            assert!(c.distortion.is_none() && c.chromatic.is_none(), "{name}");
        }
        let odd = with(ZV1_CA, 0, 21);
        let c = read(&sony_file(d.path(), &ZV1_VIG, &odd, &ZV1_DIST)).unwrap();
        assert!(c.chromatic.is_none(), "odd CA count");
    }
    #[test]
    fn reads_fujifilm_raf_tables() {
        let knots: Vec<f64> = (0..=10).map(|i| i as f64 / 10.).collect();
        let mut vig = vec![327.7];
        vig.extend(&knots);
        vig.extend([
            100., 99.5, 98.8, 97.5, 95.4, 92.2, 87.7, 82.4, 75.8, 68.3, 60.4,
        ]);
        let mut dist = vec![327.7];
        dist.extend(&knots);
        dist.extend(std::iter::repeat_n(0., 11));
        let fuji = tiff(&[
            vec![(0xf000, 13, 1, vec![0; 4])],
            vec![
                (0xf00b, 10, 23, rationals(&dist)),
                (0xf010, 10, 23, rationals(&vig)),
            ],
        ]);
        let mut raf = b"FUJIFILMCCD-RAW 0201FF383501".to_vec();
        raf.resize(128, 0);
        raf[100..104].copy_from_slice(&128u32.to_be_bytes());
        raf.extend(fuji);
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.raf");
        std::fs::write(&p, raf).unwrap();
        let c = read(&p).unwrap();
        assert!(c.default_on);
        assert!(c.distortion.is_none(), "identity distortion is omitted");
        let v = c.vignetting.as_ref().unwrap();
        let gain = |p: f32| (100. / p).powf(super::FUJI_VIGNETTE_STRENGTH);
        assert!((v.eval(1.) - gain(60.4)).abs() < 1e-4);
        assert!((v.eval(0.85) - (gain(75.8) + gain(68.3)) / 2.).abs() < 1e-4);
    }
    /// Little-endian directory of `entries` (tag, type, count, data) at `at`, as a
    /// file's or maker note's offsets count; the data follows it.
    fn directory(at: usize, entries: &[Entry]) -> Vec<u8> {
        let mut data_at = at + 2 + entries.len() * 12 + 4;
        let (mut out, mut data): (Vec<u8>, Vec<u8>) =
            ((entries.len() as u16).to_le_bytes().to_vec(), vec![]);
        for (tag, kind, count, bytes) in entries {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(count.to_le_bytes());
            if bytes.len() <= 4 {
                out.extend(bytes.iter().chain(&[0; 4]).take(4));
            } else {
                out.extend((data_at as u32).to_le_bytes());
                data.extend(bytes);
                data_at += bytes.len();
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(data);
        out
    }
    /// An ORF whose maker note's ImageProcessing directory holds `processing`.
    fn orf(processing: &[Entry]) -> Vec<u8> {
        orf_with(b"OLYMPUS\0II\x03\0", processing)
    }
    /// An ORF whose maker note starts with `header`, ending in a byte order mark and
    /// two version bytes.
    fn orf_with(header: &[u8], processing: &[Entry]) -> Vec<u8> {
        let main = header.len();
        let mut note = header.to_vec();
        let at = (main + 18) as u32;
        note.extend(directory(
            main,
            &[(0x2040, 13, 1, at.to_le_bytes().to_vec())],
        ));
        note.extend(directory(at as usize, processing));
        let mut out = b"IIRO\x08\0\0\0".to_vec();
        out.extend(directory(
            8,
            &[(0x8769, 4, 1, 26u32.to_le_bytes().to_vec())],
        ));
        out.extend(directory(
            26,
            &[(0x927c, 7, note.len() as u32, note.clone())],
        ));
        out
    }
    fn hex(s: &str) -> Vec<u8> {
        (s.split_whitespace())
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect()
    }
    /// Source radius in pixels at output radius `px` from Adobe's WarpRectilinear
    /// coefficients, whose radius 1 is `unit` pixels.
    fn adobe(k: [f64; 4], unit: f64, px: f64) -> f64 {
        let r2 = (px / unit).powi(2);
        px * (k[0] + r2 * (k[1] + r2 * (k[2] + r2 * k[3])))
    }
    /// Decoded image size and default crop.
    fn frame(size: [u32; 2], crop: [u32; 4]) -> Metadata {
        let [crop_left, crop_top, crop_width, crop_height] = crop;
        Metadata {
            width: size[0],
            height: size[1],
            crop_left,
            crop_top,
            crop_width,
            crop_height,
            ..Default::default()
        }
    }
    /// Largest difference in pixels, out to the default crop's corner, between the
    /// source radii of `c` and Adobe's warp.
    fn largest_difference(
        c: &LensCorrection,
        m: &Metadata,
        adobe: &dyn Fn(usize, f64) -> f64,
    ) -> f64 {
        let half = f64::from(m.width).hypot(f64::from(m.height)) / 2.;
        let corner = f64::from(m.crop_width).hypot(f64::from(m.crop_height)) / 2.;
        (0..=200)
            .flat_map(|i| {
                let px = corner * f64::from(i) / 200.;
                let scale = c.radial_scale((px / half) as f32);
                (0..3).map(move |ch| (px * f64::from(scale[ch]) - adobe(ch, px)).abs())
            })
            .fold(0., f64::max)
    }
    /// PA080192.ORF (raw.pixls.us, E-M10 Mark III, M.17mm F1.8): its tags, against the
    /// warp Adobe DNG Converter 18.7 writes for it (radius 1 at the 4640 × 3472 image's
    /// corner farthest from its centre, (2312, 1736)).
    #[test]
    fn reads_olympus_distortion_and_chromatic_aberration_as_adobe_does() {
        let distortion = hex("97 f4 9a bd 66 06 cb bb b3 e4 b7 3b 00 00 80 3f");
        let ca = hex("00 98 2d 3a 00 20 94 b9 00 80 b9 b9 00 58 20 ba 00 48 80 3a 00 a0 64 39");
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.orf");
        std::fs::write(&p, orf(&[(0x150a, 11, 4, distortion), (0x150c, 11, 6, ca)])).unwrap();
        let m = frame([4640, 3472], [8, 8, 4608, 3456]);
        let c = super::read(&p, &m).unwrap();
        assert!(c.default_on && c.vignetting.is_none());
        assert_eq!(c.source, "Olympus built-in");
        let warp = [
            [
                1.000663065655637,
                -0.07727914192099739,
                -0.006663217130000866,
                0.005985386689871972,
            ],
            [
                1.000000000000001,
                -0.0769286992183596,
                -0.006405051999955553,
                0.0058986243274408824,
            ],
            [
                0.999387503381258,
                -0.07587444557829748,
                -0.006451880311619311,
                0.005871129977034196,
            ],
        ];
        let unit = 2328f64.hypot(1736.);
        let off = largest_difference(&c, &m, &|ch, px| adobe(warp[ch], unit, px));
        assert!(off < 0.1, "{off} px from Adobe's warp");
        // Without the correction the corner is 220 px out.
        assert!(adobe(warp[1], unit, 2880.) < 2880. - 200.);
    }
    /// OM System bodies start their note with a longer header.
    #[test]
    fn reads_om_system_notes() {
        let distortion = [-0.02f32, 0.01, -0.003, 1.]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.orf");
        let file = orf_with(b"OM SYSTEM\0\0\0II\x04\0", &[(0x150a, 11, 4, distortion)]);
        std::fs::write(&p, file).unwrap();
        let m = frame([5240, 3912], [8, 8, 5184, 3888]);
        let c = super::read(&p, &m).unwrap();
        assert_eq!(c.source, "OM System built-in");
        assert!(c.chromatic.is_none());
        // Radius 1 of the model is the default crop's half diagonal, 3240 px.
        let half = 5240f32.hypot(3912.) / 2.;
        let at = |px: f32| c.distortion.as_ref().unwrap().eval(px / half);
        let model = |x: f32| 1. + x * x * (-0.02 + x * x * (0.01 - 0.003 * x * x));
        assert!((at(3240.) - model(1.)).abs() < 1e-5);
        assert!((at(1620.) - model(0.5)).abs() < 1e-5);
    }
    #[test]
    fn olympus_note_without_corrections_has_none() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.orf");
        let zero = |n: usize| [0, 0, 0, 0].repeat(n);
        let one = [&zero(3)[..], &1f32.to_le_bytes()].concat();
        let m = frame([4640, 3472], [8, 8, 4608, 3456]);
        std::fs::write(&p, orf(&[(0x150a, 11, 4, one), (0x150c, 11, 6, zero(6))])).unwrap();
        assert!(super::read(&p, &m).is_none());
        std::fs::write(&p, orf(&[(0x0100, 3, 2, vec![0; 4])])).unwrap();
        assert!(super::read(&p, &m).is_none());
    }
    /// An RW2 with DistortionInfo `info`.
    fn rw2(info: &[i16]) -> Vec<u8> {
        rw2_with(Some(info), None)
    }
    /// An RW2 with DistortionInfo `info` and the lateral CA table `ca` (0x011B).
    fn rw2_with(info: Option<&[i16]>, ca: Option<&[i16]>) -> Vec<u8> {
        let mut out = b"IIU\0\x08\0\0\0".to_vec();
        let mut entries = vec![(0x010f, 2, 10, b"Panasonic\0".to_vec())];
        for (tag, v) in [(0x0119, info), (0x011b, ca)] {
            if let Some(v) = v {
                entries.push((tag, 7, 2 * v.len() as u32, shorts(v)));
            }
        }
        out.extend(directory(8, &entries));
        // At least as long as the header `read` reads.
        out.resize(out.len().max(128), 0);
        out
    }
    /// gx9_3_2.RW2 (raw.pixls.us, DC-GX9, LUMIX G 25/F1.7): its DistortionInfo, against
    /// the green plane of the warp Adobe DNG Converter 18.7 writes for it (radius 1 at
    /// the 5208 × 3904 image's corner farthest from (2600, 1952)).
    const GX9: [i16; 16] = [
        -24389, -25169, 545, 0, 652, 0, 211, 1, 447, 600, 434, -38, 3240, 1100, -30428, 8502,
    ];
    #[test]
    fn reads_panasonic_distortion_as_adobe_does() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        std::fs::write(&p, rw2(&GX9)).unwrap();
        let m = frame([5200, 3904], [8, 8, 5184, 3888]);
        let c = super::read(&p, &m).unwrap();
        assert!(c.default_on && c.chromatic.is_none() && c.vignetting.is_none());
        assert_eq!(c.source, "Panasonic built-in");
        let green = [
            1.0000012404328427,
            -0.013760663120671701,
            -0.02015800015496616,
            0.004736213335729822,
        ];
        let unit = 2608f64.hypot(1952.);
        let off = largest_difference(&c, &m, &|_, px| adobe(green, unit, px));
        assert!(off < 0.1, "{off} px from Adobe's warp");
    }
    /// The DMC-GM5's DistortionInfo layout: a scale in value 5 (P1080764.RW2,
    /// raw.pixls.us, 3:2 of a 4:3 sensor), against the green plane of Adobe's warp.
    #[test]
    fn reads_panasonic_distortion_scale() {
        let gm5 = [
            25348, 2828, 62, 619, 0, -32, 0, -4095, 51, 628, 516, -19, 2870, 1128, -17663, -1541,
        ];
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        std::fs::write(&p, rw2(&gm5)).unwrap();
        let m = frame([4608, 3080], [8, 8, 4592, 3064]);
        let c = super::read(&p, &m).unwrap();
        let green = [
            0.9990234634717745,
            -0.0014475342840505334,
            9.97146343806321e-06,
            0.00045846619652323284,
        ];
        let unit = 2304f64.hypot(1540.);
        let off = largest_difference(&c, &m, &|_, px| adobe(green, unit, px));
        assert!(off < 0.1, "{off} px from Adobe's warp");
    }
    /// Largest difference in pixels, out to the default crop's corner, between red's
    /// and blue's displacement from green in `c` and in Adobe's warp, and Adobe's
    /// largest displacement.
    fn largest_ca_difference(
        c: &LensCorrection,
        m: &Metadata,
        adobe: &dyn Fn(usize, f64) -> f64,
    ) -> (f64, f64) {
        let half = f64::from(m.width).hypot(f64::from(m.height)) / 2.;
        let corner = f64::from(m.crop_width).hypot(f64::from(m.crop_height)) / 2.;
        (0..=200)
            .flat_map(|i| {
                let px = corner * f64::from(i) / 200.;
                let scale = c.radial_scale((px / half) as f32);
                [0, 2].map(|ch| {
                    let ours = px * f64::from(scale[ch] - scale[1]);
                    let theirs = adobe(ch, px) - adobe(1, px);
                    ((ours - theirs).abs(), theirs.abs())
                })
            })
            .fold((0., 0.), |(a, b), (x, y)| (a.max(x), b.max(y)))
    }
    /// Raw.pixls.us photos: their DistortionInfo and lateral CA tags, the decoded
    /// image's size and default crop, and the red, green and blue planes of the warp
    /// Adobe DNG Converter 18.7 writes for them, whose radius 1 is the farthest corner
    /// of the DNG's image from its optical centre.
    struct PanasonicCa {
        info: [i16; 16],
        ca: [i16; 32],
        frame: ([u32; 2], [u32; 4]),
        warp: [[f64; 4]; 3],
        /// The corner's offsets from the centre, in pixels.
        corner: [f64; 2],
    }
    /// gx9_3_2.RW2 (DC-GX9, LUMIX G 25/F1.7).
    const GX9_CA: PanasonicCa = PanasonicCa {
        info: GX9,
        ca: [
            -30167, -1805, 16569, 17920, 2730, 1024, 128, 21439, -246, 2184, -170, 3276, -473,
            18937, 256, 21168, 2184, 1092, 18608, 16640, -738, 0, -22016, -464, 2760, -464, 643,
            189, 17408, -2007, 17139, 15825,
        ],
        frame: ([5200, 3904], [8, 8, 5184, 3888]),
        warp: [
            [
                1.0004682282802129,
                -0.01451169102033667,
                -0.019598127970462897,
                0.004331191680153168,
            ],
            [
                1.0000012404328427,
                -0.013760663120671701,
                -0.02015800015496616,
                0.004736213335729822,
            ],
            [
                0.9998048878471808,
                -0.01311285159727582,
                -0.0202450377845782,
                0.004896631126171336,
            ],
        ],
        corner: [2608., 1952.],
    };
    #[test]
    fn reads_panasonic_chromatic_aberration_as_adobe_does() {
        // P1000403.RW2 (DMC-G80, SIGMA 60mm F2.8 DN): distortion correction off.
        let g80 = PanasonicCa {
            info: [
                8820, 0, 0, 0, 0, 0, 0, 0, 0, 0, 516, 0, 2870, 0, -12432, -15201,
            ],
            ca: [
                -2493, -15813, 1160, 12032, 2407, 256, 0, 14334, -633, 1408, 748, 2888, -230, -19,
                256, 1536, 1926, 963, 1376, 8320, 953, 96, 22912, -485, 10182, -485, -806, 361,
                512, 1175, -843, -17923,
            ],
            frame: ([4608, 3464], [8, 8, 4592, 3448]),
            warp: [
                [
                    1.0002378144655706,
                    2.9725538808200724e-05,
                    9.90484904583866e-06,
                    1.8099946004213052e-06,
                ],
                [1., 0., 0., 0.],
                [
                    0.9996276377798108,
                    -4.8152284437108506e-05,
                    3.178458018560093e-05,
                    -1.8260554076832136e-05,
                ],
            ],
            corner: [2304., 1732.],
        };
        // P1000416.RW2 (DC-TZ202): strong distortion, CA knots at 2/7, 4/7 and 6/7 of
        // the radius, and up to 3.9 px of CA. Adobe's polynomial departs from the
        // straight lines between the knots by up to 0.85 px here.
        let tz202 = PanasonicCa {
            info: [
                24343, 1775, 408, 0, 1460, 0, 631, 1, 6781, 189, 508, 587, 3288, 689, 24841, -25795,
            ],
            ca: [
                -18496, 12507, 4424, -25296, 2808, 384, -2560, 13819, 234, 832, -963, 3276, -586,
                -26642, 256, 8520, 1872, 936, -32280, 4416, -3385, 2112, 0, -859, -14384, -347,
                3085, -325, -22528, -8437, -18839, 17586,
            ],
            frame: ([5488, 3664], [8, 8, 5472, 3648]),
            warp: [
                [
                    0.9998364018762427,
                    -0.2003806248746034,
                    0.05753545636529678,
                    -0.00963193389561301,
                ],
                [
                    0.9991955093872854,
                    -0.1997130260581652,
                    0.057697840880385876,
                    -0.009660068321890813,
                ],
                [
                    0.9995507528793499,
                    -0.19942467462750102,
                    0.05834188670640916,
                    -0.009773256243380768,
                ],
            ],
            corner: [2744., 1832.],
        };
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        for (name, case) in [("GX9", &GX9_CA), ("G80", &g80), ("TZ202", &tz202)] {
            std::fs::write(&p, rw2_with(Some(&case.info), Some(&case.ca))).unwrap();
            let m = frame(case.frame.0, case.frame.1);
            let c = super::read(&p, &m).unwrap();
            assert!(c.chromatic.is_some(), "{name}");
            let unit = case.corner[0].hypot(case.corner[1]);
            let adobe = |ch: usize, px| adobe(case.warp[ch], unit, px);
            let (off, ca) = largest_ca_difference(&c, &m, &adobe);
            assert!(ca > 1., "{name}: Adobe corrects {ca} px");
            assert!(off < 0.1, "{name}: {off} px from Adobe's CA");
        }
        // The G80's lens needs no distortion correction, only the CA.
        std::fs::write(&p, rw2_with(Some(&g80.info), Some(&g80.ca))).unwrap();
        let c = super::read(&p, &frame(g80.frame.0, g80.frame.1)).unwrap();
        assert!(c.distortion.is_none());
    }
    /// Newer bodies, whose DNGs carry the distortion in an embedded profile, keep the CA
    /// in the warp, on the sensor's radius: P1002296.RW2 (DC-GH7, LEICA DG 12-35/F2.8 at
    /// 12mm).
    #[test]
    fn reads_panasonic_chromatic_aberration_on_the_sensor_radius() {
        let info = [
            18216, -26703, 241, 0, -399, 0, 124, 1, 4495, 436, 354, -1062, 3611, 936, -23617,
            -17367,
        ];
        let ca = [
            27551, 1983, 18, 15368, 3032, 32, -5888, 6654, -1268, 338, -546, 3638, -725, -1, 256,
            2064, 2426, 1213, 18128, 0, -1086, 384, -3584, -1268, -4230, -1268, -676, -159, -17152,
            -2148, -15798, -17015,
        ];
        let warp = [
            [
                1.000588699784562,
                -2.5632476810422133e-05,
                -0.00029661560319560465,
                -8.09482177484e-05,
            ],
            [1., 0., 0., 0.],
            [
                1.0001421369447667,
                1.0062062321125609e-05,
                0.0005116995869975938,
                -7.363913965363622e-05,
            ],
        ];
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        std::fs::write(&p, rw2_with(Some(&info), Some(&ca))).unwrap();
        let m = frame([5792, 4352], [8, 8, 5776, 4336]);
        let c = super::read(&p, &m).unwrap();
        assert!(c.distortion.is_some());
        let half = 5792f64.hypot(4352.) / 2.;
        // Adobe's warp applies at the sensor radius green samples after distortion.
        let sensor = |px: f64| px * f64::from(c.radial_scale((px / half) as f32)[1]);
        let unit = 2896f64.hypot(2176.);
        let (off, most) =
            largest_ca_difference(&c, &m, &|ch, px| adobe(warp[ch], unit, sensor(px)));
        assert!(most > 1., "Adobe corrects {most} px");
        assert!(off < 0.1, "{off} px from Adobe's CA");
    }
    /// Lenses without CA data store an empty table; malformed tables are not read.
    #[test]
    fn panasonic_chromatic_tables_without_data_are_not_read() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        let m = frame(GX9_CA.frame.0, GX9_CA.frame.1);
        let mut empty = [0; 32];
        // A DMC-G6's: only the values that change with each shot are set.
        [empty[0], empty[1], empty[2], empty[7], empty[13], empty[31]] =
            [-5508, 3276, 4624, -26724, 24178, 852];
        let mut unordered = GX9_CA.ca;
        unordered.swap(16, 17);
        let mut unknown = GX9_CA.ca;
        unknown[14] = 512;
        for (name, ca) in [
            ("empty", &empty[..]),
            ("unordered", &unordered[..]),
            ("unknown layout", &unknown[..]),
            ("short", &GX9_CA.ca[..31]),
        ] {
            std::fs::write(&p, rw2_with(Some(&GX9), Some(ca))).unwrap();
            let c = super::read(&p, &m).unwrap();
            assert!(c.chromatic.is_none() && c.distortion.is_some(), "{name}");
            std::fs::write(&p, rw2_with(None, Some(ca))).unwrap();
            assert!(super::read(&p, &m).is_none(), "{name}");
        }
        // Without DistortionInfo the CA is still read.
        std::fs::write(&p, rw2_with(None, Some(&GX9_CA.ca))).unwrap();
        let c = super::read(&p, &m).unwrap();
        assert!(c.chromatic.is_some() && c.distortion.is_none());
    }
    /// A model whose correction folds back on itself has no inverse to sample.
    #[test]
    fn panasonic_model_without_inverse_is_not_read() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        let mut folding = GX9;
        folding[11] = -32768;
        std::fs::write(&p, rw2(&folding)).unwrap();
        let m = frame([5200, 3904], [8, 8, 5184, 3888]);
        assert!(super::read(&p, &m).is_none());
    }
    #[test]
    fn panasonic_correction_switched_off_is_not_read() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.rw2");
        let mut off = GX9;
        off[7] = 0;
        std::fs::write(&p, rw2(&off)).unwrap();
        let m = frame([5200, 3904], [8, 8, 5184, 3888]);
        assert!(super::read(&p, &m).is_none());
    }
    #[test]
    fn unsupported_or_truncated_files_have_no_correction() {
        let d = tempfile::tempdir().unwrap();
        for (name, bytes) in [
            ("empty", &b""[..]),
            ("text", &[b'x'; 200][..]),
            ("tiff", b"II*\0\x08\0\0\0\0\0"),
        ] {
            let p = d.path().join(name);
            std::fs::write(&p, bytes).unwrap();
            assert!(read(&p).is_none(), "{name}");
        }
    }
}
