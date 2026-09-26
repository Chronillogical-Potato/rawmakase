//! Maker lens-correction tables, read directly from the RAW container.
//!
//! - Fujifilm RAF: FujiIFD tags 0xF00B (distortion), 0xF00F (lateral CA) and
//!   0xF010 (vignetting), signed rationals. The first value is a pixel scale; knots
//!   are fractions of the half diagonal, distortion is in percent, CA a radius
//!   fraction, vignetting the remaining illumination in percent.
//! - Sony ARW: raw SubIFD tags 0x7032 (vignetting), 0x7035 (CA) and 0x7037
//!   (distortion), signed shorts led by the number of knots, spread evenly from the
//!   centre to the corner. Distortion is in 2^-14, CA in 2^-21 units; vignetting v
//!   restores 2^(2^(v/8192 - 1) - 0.5).
//!
//! The Fujifilm vignetting agrees with the FixVignetteRadial opcode Adobe writes into
//! DNGs of the same file to within 1% (see docs/lens-corrections.md).
use super::{LensCorrection, Radial};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

/// Returns `None` for unsupported files or when the camera stored no corrections.
pub fn read(path: &Path) -> Option<LensCorrection> {
    let mut f = File::open(path).ok()?;
    let mut head = [0u8; 108];
    f.read_exact(&mut head).ok()?;
    let c = if head.starts_with(b"FUJIFILMCCD-RAW") {
        let base = u32::from_be_bytes(head[100..104].try_into().ok()?) as u64;
        fuji(&mut Tiff::open(f, base)?)
    } else if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
        sony(&mut Tiff::open(f, 0)?)
    } else {
        None
    }?;
    (!c.is_empty() && c.validate()).then_some(c)
}

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
        .and_then(|v| pairs(&v, &|p| 100. / p))
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
    let single = |v: Option<Vec<f32>>, f: &dyn Fn(f32) -> f32| {
        let v = v?;
        let n = *v.first()? as usize;
        (n >= 2 && v.len() == n + 1)
            .then(|| Radial::new(knots(n), v[1..].iter().map(|x| f(*x)).collect()))?
    };
    let vignetting = single(vignetting, &|v| (2f32.powf(v / 8192. - 1.) - 0.5).exp2())
        .filter(|r| r.values.iter().any(|v| (v - 1.).abs() > 1e-4));
    let distortion = single(distortion, &|d| 1. + d / 16384.)
        .filter(|r| r.values.iter().any(|v| (v - 1.).abs() > 1e-6));
    let chromatic = chromatic.and_then(|v| {
        let n = *v.first()? as usize / 2;
        if n < 2 || v.len() != 2 * n + 1 {
            return None;
        }
        let scale = |s: &[f32]| s.iter().map(|x| 1. + x / 2_097_152.).collect();
        Some([
            Radial::new(knots(n), scale(&v[1..=n]))?,
            Radial::new(knots(n), scale(&v[n + 1..]))?,
        ])
    });
    Some(LensCorrection {
        source: "Sony built-in".into(),
        // Not yet compared with Lightroom, which may apply its own profile instead.
        default_on: false,
        vignetting,
        distortion,
        chromatic,
    })
}

struct Entry {
    kind: u16,
    count: u32,
    value: [u8; 4],
}
/// Minimal bounded TIFF directory reader. Offsets are relative to `base`.
struct Tiff {
    f: File,
    base: u64,
    little: bool,
    first: u64,
}
impl Tiff {
    fn open(mut f: File, base: u64) -> Option<Self> {
        let mut h = [0u8; 8];
        f.seek(SeekFrom::Start(base)).ok()?;
        f.read_exact(&mut h).ok()?;
        let little = match &h[..4] {
            b"II*\0" => true,
            b"MM\0*" => false,
            _ => return None,
        };
        let mut t = Self {
            f,
            base,
            little,
            first: 0,
        };
        t.first = t.u32(&h[4..8]) as u64;
        Some(t)
    }
    fn u16(&self, b: &[u8]) -> u16 {
        let a = [b[0], b[1]];
        if self.little {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        }
    }
    fn u32(&self, b: &[u8]) -> u32 {
        let a = [b[0], b[1], b[2], b[3]];
        if self.little {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        }
    }
    fn bytes(&mut self, offset: u64, len: usize) -> Option<Vec<u8>> {
        if len > 1 << 20 {
            return None;
        }
        let mut b = vec![0; len];
        self.f
            .seek(SeekFrom::Start(self.base.checked_add(offset)?))
            .ok()?;
        self.f.read_exact(&mut b).ok()?;
        Some(b)
    }
    fn ifd(&mut self, offset: u64) -> Option<BTreeMap<u16, Entry>> {
        let count = self.bytes(offset, 2)?;
        let n = self.u16(&count) as usize;
        if n == 0 || n > 1000 {
            return None;
        }
        let b = self.bytes(offset + 2, n * 12)?;
        Some(
            b.chunks_exact(12)
                .map(|e| {
                    (
                        self.u16(&e[0..2]),
                        Entry {
                            kind: self.u16(&e[2..4]),
                            count: self.u32(&e[4..8]),
                            value: [e[8], e[9], e[10], e[11]],
                        },
                    )
                })
                .collect(),
        )
    }
    /// The first value of a LONG or IFD entry, used as a directory offset.
    fn offset(&self, e: &Entry) -> Option<u64> {
        matches!(e.kind, 4 | 13).then(|| self.u32(&e.value) as u64)
    }
    fn numbers(&mut self, e: &Entry) -> Option<Vec<f32>> {
        let size = match e.kind {
            3 | 8 => 2,
            4 | 9 => 4,
            5 | 10 => 8,
            _ => return None,
        };
        let len = (e.count as usize).checked_mul(size)?;
        let b = if len <= 4 {
            e.value[..len].to_vec()
        } else {
            let at = self.u32(&e.value) as u64;
            self.bytes(at, len)?
        };
        let v: Vec<f32> = b
            .chunks_exact(size)
            .map(|c| match e.kind {
                3 => self.u16(c) as f32,
                8 => self.u16(c) as i16 as f32,
                4 => self.u32(c) as f32,
                9 => self.u32(c) as i32 as f32,
                5 => self.u32(&c[..4]) as f32 / self.u32(&c[4..]) as f32,
                _ => self.u32(&c[..4]) as i32 as f32 / self.u32(&c[4..]) as i32 as f32,
            })
            .collect();
        v.iter().all(|x| x.is_finite()).then_some(v)
    }
}

#[cfg(test)]
mod tests {
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
        let c = super::read(&p).unwrap();
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
        let c = super::read(&p).unwrap();
        assert!(c.default_on);
        assert!(c.distortion.is_none(), "identity distortion is omitted");
        let v = c.vignetting.as_ref().unwrap();
        assert!((v.eval(1.) - 100. / 60.4).abs() < 1e-4);
        assert!((v.eval(0.85) - (100. / 75.8 + 100. / 68.3) / 2.).abs() < 1e-4);
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
            assert!(super::read(&p).is_none(), "{name}");
        }
    }
}
