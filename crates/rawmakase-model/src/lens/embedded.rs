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
//!   source radius that this maps to r·(1 + v5/2^15). The CA the camera stores
//!   (tag 0x011B) is not read.
//!
//! The Fujifilm vignetting agrees with the FixVignetteRadial opcode Adobe writes into
//! DNGs of the same file to within 1%, and the Olympus and Panasonic distortion and
//! the Olympus CA with the WarpRectilinear opcodes (and, for newer bodies, the
//! embedded lens profiles) Adobe DNG Converter 18.7 writes, to within a pixel (see
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

fn panasonic(t: &mut Tiff, m: &Metadata) -> Option<LensCorrection> {
    let ifd0 = t.ifd(t.first)?;
    let b = t.raw(ifd0.get(&0x0119)?)?;
    let v: Vec<f64> = b
        .get(..32)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| f64::from(t.u16(c) as i16))
        .collect();
    let flags = t.u16(&b[14..16]);
    let unit = v[12];
    if flags & 0xf != 1 || unit <= 0. {
        return None;
    }
    let [a, b, c] = [v[8], v[4], v[11]].map(|x| x / 32768.);
    let scale = 1. + v[5] / 32768.;
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
    let (distortion, _) = maker_model(m, unit, &green, None);
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
        chromatic: None,
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
        let mut out = b"IIU\0\x08\0\0\0".to_vec();
        let info = info.iter().flat_map(|v| v.to_le_bytes()).collect();
        out.extend(directory(
            8,
            &[
                (0x010f, 2, 10, b"Panasonic\0".to_vec()),
                (0x0119, 7, 32, info),
            ],
        ));
        // As long as the header `read` reads.
        out.resize(128, 0);
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
