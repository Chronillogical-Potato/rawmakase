//! Rendering hints that a DNG carries for its raw image, as Lightroom uses them:
//! the embedded camera profile, BaselineExposure, the default crop, and the
//! FixVignetteRadial (OpcodeList2) and WarpRectilinear (OpcodeList3) lens corrections.
use crate::{
    camera_profiles::CameraProfile,
    lens::{LensCorrection, Radial},
    tiff::{Entry, Tiff},
};
use std::{collections::BTreeMap, fs::File, path::Path};

#[derive(Default)]
pub struct Dng {
    pub baseline_exposure: Option<f32>,
    /// Left, top, width, height, relative to the active area.
    pub crop: Option<[u32; 4]>,
    pub profile: Option<CameraProfile>,
    /// The file's colour matrix, XYZ to camera, when it has one but no profile
    /// to read it from. LibRaw reports no matrix for a DNG at all, so without
    /// this a DNG carrying only colour matrices has no described colour.
    pub color_matrix: Option<[[f32; 3]; 3]>,
    pub lens: Option<LensCorrection>,
}

/// IFD0 tags that make up an embedded camera profile, in DCP form.
const PROFILE_TAGS: &[u16] = &[
    50708, // UniqueCameraModel
    50721, 50722, // ColorMatrix1/2
    50778, 50779, // CalibrationIlluminant1/2
    50932, // ProfileCalibrationSignature
    50936, // ProfileName
    50937, 50938, 50939, // ProfileHueSatMap dims, data 1/2
    50940, // ProfileToneCurve
    50941, // ProfileEmbedPolicy
    50942, // ProfileCopyright
    50964, 50965, // ForwardMatrix1/2
    50981, 50982, // ProfileLookTable dims, data
    51107, 51108, // ProfileHueSatMapEncoding, ProfileLookTableEncoding
    51109, // BaselineExposureOffset
];

/// `None` unless the file is a DNG.
pub fn read(path: &Path) -> Option<Dng> {
    let mut t = Tiff::open(File::open(path).ok()?, 0)?;
    let ifd0 = t.ifd(t.first)?;
    ifd0.get(&50706)?; // DNGVersion
    let tags = profile_tags(&mut t, &ifd0);
    // A profile needs a forward matrix; one written as colour matrices alone
    // still describes the camera's colour, so keep that when it is all there is.
    let profile = tags
        .as_deref()
        .and_then(|b| crate::camera_profiles::from_bytes(b).ok());
    let color_matrix = if profile.is_some() {
        None
    } else {
        tags.as_deref()
            .and_then(|b| crate::camera_profiles::color_matrix_only(b).ok().flatten())
    };
    let mut dng = Dng {
        baseline_exposure: ifd0
            .get(&50730)
            .and_then(|e| t.numbers(e))
            .and_then(|v| v.first().copied())
            .filter(|v| v.is_finite() && v.abs() <= 5.),
        profile,
        color_matrix,
        ..Default::default()
    };
    // The full-resolution raw image is the SubIFD (or IFD0) with NewSubfileType 0.
    let mut candidates = vec![ifd0];
    if let Some(subs) = candidates[0].get(&330).and_then(|e| t.offsets(e)) {
        for offset in subs.into_iter().take(16) {
            if let Some(ifd) = t.ifd(offset) {
                candidates.push(ifd);
            }
        }
    }
    // Strips or tiles, and NewSubfileType 0 (the main image) when recorded.
    let raw = candidates.into_iter().find(|ifd| {
        (ifd.contains_key(&273) || ifd.contains_key(&324))
            && ifd
                .get(&254)
                .is_none_or(|e| t.numbers(e).is_some_and(|v| v.first() == Some(&0.)))
    })?;
    let pair = |t: &mut Tiff, tag| -> Option<[u32; 2]> {
        let v = t.numbers(raw.get(&tag)?)?;
        (v.len() == 2 && v.iter().all(|x| x.is_finite() && *x >= 0.))
            .then(|| [v[0].round() as u32, v[1].round() as u32])
    };
    if let (Some(origin), Some(size)) = (pair(&mut t, 50719), pair(&mut t, 50720)) {
        dng.crop = Some([origin[0], origin[1], size[0], size[1]]);
    }
    let vignetting = raw
        .get(&51009)
        .and_then(|e| t.raw(e))
        .and_then(|b| opcode(&b, 3))
        .and_then(|p| vignette(&p));
    let warp = raw
        .get(&51022)
        .and_then(|e| t.raw(e))
        .and_then(|b| opcode(&b, 1))
        .and_then(|p| warp(&p));
    let (distortion, chromatic) = warp.unwrap_or((None, None));
    let lens = LensCorrection {
        source: "DNG opcodes".into(),
        default_on: true,
        vignetting,
        distortion,
        chromatic,
    };
    dng.lens = (!lens.is_empty() && lens.validate()).then_some(lens);
    Some(dng)
}

/// Rewrites the profile tags as a standalone DCP so the regular parser validates them.
/// Returns the bytes rather than a profile, since the matrix is readable from
/// them even when the profile itself is not.
fn profile_tags(t: &mut Tiff, ifd0: &BTreeMap<u16, Entry>) -> Option<Vec<u8>> {
    let mut entries = Vec::new();
    for tag in PROFILE_TAGS {
        if let Some(e) = ifd0.get(tag) {
            entries.push((*tag, e.kind, e.count, t.raw(e)?));
        }
    }
    let le = t.little;
    let u16b = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let u32b = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let mut out: Vec<u8> = if le { b"II".to_vec() } else { b"MM".to_vec() };
    out.extend(u16b(0x4352)); // DCP magic
    out.extend(u32b(8));
    out.extend(u16b(entries.len() as u16));
    let mut data_at = 8 + 2 + entries.len() * 12 + 4;
    let mut data = Vec::new();
    for (tag, kind, count, bytes) in &entries {
        out.extend(u16b(*tag));
        out.extend(u16b(*kind));
        out.extend(u32b(*count));
        if bytes.len() <= 4 {
            let mut v = bytes.clone();
            v.resize(4, 0);
            out.extend(v);
        } else {
            out.extend(u32b(data_at as u32));
            data.extend(bytes);
            if bytes.len() % 2 == 1 {
                data.push(0);
            }
            data_at = 8 + 2 + entries.len() * 12 + 4 + data.len();
        }
    }
    out.extend(u32b(0));
    out.extend(data);
    Some(out)
}

/// Parameters of the first opcode with `id` in a big-endian DNG opcode list.
fn opcode(b: &[u8], id: u32) -> Option<Vec<u8>> {
    let be = |o: usize| Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?));
    let mut o = 4;
    for _ in 0..be(0)?.min(64) {
        let (code, size) = (be(o)?, be(o + 12)? as usize);
        let params = b.get(o + 16..o + 16 + size)?;
        if code == id {
            return Some(params.to_vec());
        }
        o += 16 + size;
    }
    None
}
fn doubles(p: &[u8]) -> Vec<f64> {
    p.as_chunks::<8>()
        .0
        .iter()
        .map(|c| f64::from_be_bytes(*c))
        .collect()
}
/// Radius samples; the opcode radius, like ours, is 1 at the farthest corner from
/// the optical centre, which must be near the image centre.
fn knots() -> Vec<f32> {
    (0..=32).map(|i| i as f32 / 32.).collect()
}
fn centred(cx: f64, cy: f64) -> bool {
    (cx - 0.5).abs() < 0.02 && (cy - 0.5).abs() < 0.02
}
/// FixVignetteRadial: gain 1 + k0 r² + k1 r⁴ + k2 r⁶ + k3 r⁸ + k4 r¹⁰.
fn vignette(p: &[u8]) -> Option<Radial> {
    let v = doubles(p);
    if v.len() != 7 || !centred(v[5], v[6]) {
        return None;
    }
    let values = knots()
        .iter()
        .map(|r| {
            let r2 = f64::from(*r * *r);
            (1. + (0..5).map(|i| v[i] * r2.powi(i as i32 + 1)).sum::<f64>()) as f32
        })
        .collect();
    let r = Radial {
        knots: knots(),
        values,
    };
    r.values.iter().any(|g| (g - 1.).abs() > 1e-4).then_some(r)
}
/// WarpRectilinear: source radius ratio kr0 + kr1 r² + kr2 r⁴ + kr3 r⁶ per plane.
/// Tangential terms are ignored.
fn warp(p: &[u8]) -> Option<(Option<Radial>, Option<[Radial; 2]>)> {
    let planes = u32::from_be_bytes(p.get(..4)?.try_into().ok()?) as usize;
    let v = doubles(p.get(4..)?);
    if !matches!(planes, 1 | 3)
        || v.len() != planes * 6 + 2
        || !centred(v[planes * 6], v[planes * 6 + 1])
    {
        return None;
    }
    let ratio = |plane: usize, r: f32| {
        let k = &v[plane * 6..plane * 6 + 4];
        let r2 = f64::from(r * r);
        (k[0] + k[1] * r2 + k[2] * r2 * r2 + k[3] * r2 * r2 * r2) as f32
    };
    let green = if planes == 3 { 1 } else { 0 };
    let curve = |f: &dyn Fn(f32) -> f32| Radial {
        knots: knots(),
        values: knots().into_iter().map(f).collect(),
    };
    let distortion = curve(&|r| ratio(green, r));
    let chromatic = (planes == 3).then(|| {
        [
            curve(&|r| ratio(0, r) / ratio(1, r)),
            curve(&|r| ratio(2, r) / ratio(1, r)),
        ]
    });
    let changes = |r: &Radial| r.values.iter().any(|v| (v - 1.).abs() > 1e-6);
    Some((
        Some(distortion).filter(changes),
        chromatic.filter(|[a, b]| changes(a) || changes(b)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn list(id: u32, params: &[u8]) -> Vec<u8> {
        let mut b = 1u32.to_be_bytes().to_vec();
        for v in [id, 0x0103_0000, 0, params.len() as u32] {
            b.extend(v.to_be_bytes());
        }
        b.extend(params);
        b
    }
    fn be(v: &[f64]) -> Vec<u8> {
        v.iter().flat_map(|x| x.to_be_bytes()).collect()
    }
    #[test]
    fn decodes_vignette_and_warp_opcodes() {
        // DSCF7853's FixVignetteRadial from Lightroom's DNG.
        let p = be(&[0.2599, 0.3424, 0.0630, 0.0346, -0.0300, 0.5, 0.5]);
        let v = vignette(&opcode(&list(3, &p), 3).unwrap()).unwrap();
        assert!((v.eval(1.) - 1.67).abs() < 0.01);
        assert!((v.eval(0.5) - 1.087).abs() < 0.002);
        assert!(opcode(&list(3, &p), 1).is_none());
        let mut w = 3u32.to_be_bytes().to_vec();
        w.extend(be(&[1.0002, 0.0005, 0., 0., 0., 0.]));
        w.extend(be(&[1., 0., 0., 0., 0., 0.]));
        w.extend(be(&[1., 0., 0., 0., 0., 0.]));
        w.extend(be(&[0.5, 0.5]));
        let (distortion, chromatic) = warp(&w).unwrap();
        assert!(distortion.is_none());
        let [red, blue] = chromatic.unwrap();
        assert!((red.eval(1.) - 1.0007).abs() < 1e-5);
        assert_eq!(blue.eval(1.), 1.);
        // Off-centre corrections are not representable and are skipped.
        assert!(vignette(&be(&[0.2, 0., 0., 0., 0., 0.3, 0.5])).is_none());
    }
    #[test]
    fn non_dng_files_are_ignored() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), b"II*\0\x08\0\0\0\0\0\0\0").unwrap();
        assert!(read(f.path()).is_none());
    }
}
