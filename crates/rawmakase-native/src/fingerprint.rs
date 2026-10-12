//! Decode fingerprints: what opening and unpacking a RAW file yields, as plain
//! values, so a LibRaw update or a change to opening photos shows up as a changed
//! field (`rawmakase fingerprint`, `scripts/corpus/dataset-fingerprints.py`).
//! The file is opened by [`crate::photo::open`] and unpacked as development
//! unpacks it; nothing is demosaiced, so every value is exact on any machine.
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct Fingerprint {
    pub path: std::path::PathBuf,
    /// The LibRaw that decoded it.
    pub libraw: String,
    /// SHA-256 of the file's bytes; `None` when it cannot be read.
    pub sha256: Option<String>,
    /// The step that failed: `read`, `open` (LibRaw, or a sensor RAWmakase does
    /// not develop) or `unpack`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub opened: Option<Opened>,
    #[serde(flatten, skip_serializing_if = "Option::is_none")]
    pub sensor: Option<SensorData>,
}

/// What opening the file says, after `photo::open`'s DNG and RAF additions.
#[derive(Debug, Serialize)]
pub struct Opened {
    pub make: String,
    pub model: String,
    /// The visible area.
    pub size: [u32; 2],
    pub raw_size: [u32; 2],
    /// Left, top, width, height.
    pub crop: [u32; 4],
    /// LibRaw's orientation (`flip`).
    pub orientation: i32,
    /// As shot, relative to green.
    pub wb: [f32; 3],
    pub daylight_wb: [f32; 3],
    /// XYZ(D65) to camera.
    pub camera_matrix: [[f32; 3]; 3],
}

/// The unpacked sensor data and its levels. Per-channel values are in LibRaw's
/// order: red, green, blue, second green.
#[derive(Debug, Serialize)]
pub struct SensorData {
    /// 1 for a mosaic, 3 or 4 for small RAW files.
    pub channels: u32,
    /// Where the visible area starts in the raw buffer: left, top.
    pub origin: [u32; 2],
    pub black: [u32; 4],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub black_pattern: Option<BlackPattern>,
    pub white: u32,
    pub linear_white: [u32; 4],
    /// The visible colour pattern's rows, e.g. `RG/gB` (`g` is the second green),
    /// `GGRGGB/…` for X-Trans; empty without a mosaic.
    pub cfa: String,
    /// SHA-256 of the unpacked raw data in the visible area.
    pub data_sha256: String,
}

/// A black level that repeats over `rows × cols` pixels, on top of `black`.
#[derive(Debug, Serialize)]
pub struct BlackPattern {
    pub rows: u32,
    pub cols: u32,
    pub values: Vec<u32>,
}

/// The fingerprint of the file at `path`. A file that cannot be read, opened or
/// unpacked gives a fingerprint with its error, not an `Err`.
pub fn fingerprint(path: &Path) -> Fingerprint {
    let mut f = Fingerprint {
        path: path.to_path_buf(),
        libraw: crate::raw::version(),
        sha256: None,
        failed: None,
        error: None,
        opened: None,
        sensor: None,
    };
    let before = identity(path);
    if let Err((step, e)) = describe(path, &mut f) {
        f.failed = Some(step);
        f.error = Some(format!("{e:#}"));
    }
    // The file is read twice (hashed, then decoded): a file replaced in between
    // would pair one file's hash with another's fingerprint.
    if before.is_some() && identity(path) != before {
        f.opened = None;
        f.sensor = None;
        f.failed = Some("read");
        f.error = Some("The file changed while it was read".into());
    }
    f
}

/// What tells one file at `path` from another: size, modification time and,
/// on Unix, device and inode.
fn identity(path: &Path) -> Option<(u64, std::time::SystemTime, u64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    #[cfg(unix)]
    let (dev, ino) = {
        use std::os::unix::fs::MetadataExt;
        (m.dev(), m.ino())
    };
    #[cfg(not(unix))]
    let (dev, ino) = (0, 0);
    Some((m.len(), m.modified().ok()?, dev, ino))
}

fn describe(path: &Path, f: &mut Fingerprint) -> Result<(), (&'static str, anyhow::Error)> {
    use sha2::Digest;
    use std::io::Read;
    let read = |e: std::io::Error| ("read", e.into());
    let mut hash = sha2::Sha256::new();
    let (mut file, mut buf) = (std::fs::File::open(path).map_err(read)?, vec![0; 1 << 20]);
    loop {
        match file.read(&mut buf).map_err(read)? {
            0 => break,
            n => hash.update(&buf[..n]),
        }
    }
    f.sha256 = Some(hex(&hash.finalize()));
    let raw = crate::photo::open(path).map_err(|e| ("open", e))?;
    let m = &raw.metadata;
    f.opened = Some(Opened {
        make: m.make.clone(),
        model: m.model.clone(),
        size: [m.width, m.height],
        raw_size: [m.raw_width, m.raw_height],
        crop: [m.crop_left, m.crop_top, m.crop_width, m.crop_height],
        orientation: m.flip,
        wb: m.wb,
        daylight_wb: m.daylight_wb,
        camera_matrix: m.cam_xyz,
    });
    let s = raw.sensor().map_err(|e| ("unpack", e))?;
    let (rows, cols, values) = s.black_pattern;
    f.sensor = Some(SensorData {
        channels: s.channels,
        origin: s.origin,
        black: s.black,
        black_pattern: (rows > 0 && cols > 0).then_some(BlackPattern { rows, cols, values }),
        white: s.white,
        linear_white: s.linear_white,
        cfa: cfa(&s.pattern),
        data_sha256: hex(&s.data_sha256),
    });
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The pattern's rows as letters, reduced to the period that repeats.
fn cfa(pattern: &[Vec<u8>]) -> String {
    let period = |n: usize, same: &dyn Fn(usize, usize) -> bool| {
        (1..=n)
            .find(|&p| n.is_multiple_of(p) && (p..n).all(|i| same(i, i % p)))
            .unwrap_or(n)
    };
    let rows = period(pattern.len(), &|a, b| pattern[a] == pattern[b]);
    let cols = pattern.first().map_or(0, |row| {
        period(row.len(), &|a, b| pattern.iter().all(|r| r[a] == r[b]))
    });
    pattern[..rows]
        .iter()
        .map(|row| {
            row[..cols]
                .iter()
                .map(|&c| match c {
                    0 => 'R',
                    1 => 'G',
                    3 => 'g',
                    2 => 'B',
                    _ => '?',
                })
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    #[test]
    fn chart_dng_fingerprint_is_pinned() {
        let chart = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/corpus/charts/synthetic-d65.dng"
        );
        let mut json = serde_json::to_value(super::fingerprint(chart.as_ref())).unwrap();
        // LibRaw computes white balance in floating point, which may round
        // differently with another compiler; the rest is exact everywhere.
        let fields = json.as_object_mut().unwrap();
        assert_eq!(fields.remove("path").unwrap(), chart);
        assert_eq!(fields.remove("libraw").unwrap(), crate::raw::version());
        let wb = fields.remove("wb").unwrap();
        let daylight = fields.remove("daylight_wb").unwrap();
        for (got, want) in [
            (wb, [1.8181818, 1., 1.3888888]),
            (daylight, [1.817948, 1., 1.3889403]),
        ] {
            let got: [f32; 3] = serde_json::from_value(got).unwrap();
            assert!(
                got.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-5),
                "{got:?}"
            );
        }
        // As the generator (tests/color/dng.rs) writes it: RGGB, black 0, white
        // 65535, no DefaultCrop (LibRaw's 65535 marks none) and a camera LibRaw
        // has no matrix for, whose colour is in the embedded profile.
        let want = serde_json::json!({
            "sha256": "3b23ed9df6b4e9ce0f504f8e01ad9b3c3d105d11bf177dac95f40a99f70a0407",
            "make": "RAWmakase",
            "model": "Synthetic Chart",
            "size": [970, 742],
            "raw_size": [970, 742],
            "crop": [65535, 65535, 0, 0],
            "orientation": 0,
            "camera_matrix": [[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]],
            "channels": 1,
            "origin": [0, 0],
            "black": [0, 0, 0, 0],
            "black_pattern": {"rows": 1, "cols": 1, "values": [0]},
            "white": 65535,
            "linear_white": [0, 0, 0, 0],
            "cfa": "RG/gB",
            "data_sha256": "d7f816bb902c0e23d349d53c86a0925200019bd37cf18bca732dfca370ca89e5",
        });
        assert_eq!(json, want);
    }

    #[test]
    fn unreadable_and_non_raw_files_are_errors_not_failures() {
        let dir = tempfile::tempdir().unwrap();
        let missing = super::fingerprint(&dir.path().join("missing.dng"));
        assert!(missing.sha256.is_none() && missing.failed == Some("read"));
        let text = dir.path().join("text.dng");
        std::fs::write(&text, b"not a raw file").unwrap();
        let f = super::fingerprint(&text);
        assert_eq!(
            f.sha256.as_deref(),
            Some("75d3499f372b9001c41b9ca9d7c9c5398477905e49e949746947ea7942b2f859")
        );
        assert!(f.failed == Some("open") && f.opened.is_none() && f.sensor.is_none());
    }

    #[test]
    fn cfa_patterns_reduce_to_their_period() {
        let bayer: Vec<Vec<u8>> = (0..16)
            .map(|r| if r % 2 == 0 { vec![0, 1] } else { vec![3, 2] })
            .collect();
        assert_eq!(super::cfa(&bayer), "RG/gB");
        let xtrans = ["GGRGGB", "GGBGGR", "BRGRBG", "GGBGGR", "GGRGGB", "RBGBRG"];
        let pattern: Vec<Vec<u8>> = xtrans
            .iter()
            .map(|row| {
                row.bytes()
                    .map(|c| b"RGB".iter().position(|&x| x == c).unwrap() as u8)
                    .collect()
            })
            .collect();
        assert_eq!(super::cfa(&pattern), xtrans.join("/"));
        // A period of 4 rows inside LibRaw's 16 by 16 (Leaf) pattern.
        let leaf: Vec<Vec<u8>> = (0..16).map(|r| vec![(r % 4) as u8 % 3; 16]).collect();
        assert_eq!(super::cfa(&leaf), "R/G/B/R");
        assert_eq!(super::cfa(&[]), "");
    }
}
