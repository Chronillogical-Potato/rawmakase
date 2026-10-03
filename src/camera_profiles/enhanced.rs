//! XMP look profiles backed by Adobe DNG SDK-format HSV big tables.
//! Assets are read from the user's installation, never bundled with RAWmakase.
use super::{CameraProfile, Table};
use crate::{
    color_math::{srgb_decode, srgb_encode},
    develop::curve::{CurveLut, ToneCurve},
    xmp::ns::{CRS, RDF, XML},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};
const ALPHABET: &[u8] =
    b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?`'|()[]{}@%$#";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Enhanced {
    pub uuid: String,
    pub base_name: String,
    #[serde(default)]
    pub highlights: f32,
    #[serde(default)]
    pub shadows: f32,
    #[serde(default)]
    pub clarity: f32,
    #[serde(default)]
    pub monochrome: bool,
    pub(super) table: Table,
    pub(super) curve: Vec<f32>,
}
impl Enhanced {
    #[cfg(test)]
    pub(super) fn for_test(table: Table) -> Self {
        Self {
            uuid: "0".repeat(32),
            base_name: "Test".into(),
            highlights: 0.,
            shadows: 0.,
            clarity: 0.,
            monochrome: false,
            table,
            curve: (0..=4096)
                .map(|i| {
                    let x = i as f32 / 4096.;
                    x * x * (3. - 2. * x)
                })
                .collect(),
        }
    }
    pub(super) fn validate(&self) -> Result<()> {
        ensure!(
            self.uuid.len() == 32 && self.uuid.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid look UUID"
        );
        ensure!(!self.base_name.is_empty(), "Missing look base profile");
        ensure!(
            [self.highlights, self.shadows, self.clarity]
                .iter()
                .all(|v| v.is_finite() && (-1. ..=1.).contains(v)),
            "Invalid profile tone adjustment"
        );
        self.table.validate()?;
        ensure!(
            self.curve.len() == 4097
                && self
                    .curve
                    .iter()
                    .all(|v| v.is_finite() && (0. ..=1.).contains(v)),
            "Invalid look curve"
        );
        Ok(())
    }
    pub(super) fn apply_table(&self, rgb: [f32; 3]) -> [f32; 3] {
        self.table.apply(rgb.map(|v| v.clamp(0., 1.)), None, 0.)
    }
    pub(super) fn apply_curve(&self, rgb: [f32; 3]) -> [f32; 3] {
        let p = rgb.map(|v| srgb_encode(v.clamp(0., 1.)));
        let eval = |v: f32| {
            let x = v.clamp(0., 1.) * 4096.;
            let i = (x as usize).min(4095);
            self.curve[i] + (self.curve[i + 1] - self.curve[i]) * (x - i as f32)
        };
        let lo = p.into_iter().fold(f32::INFINITY, f32::min);
        let hi = p.into_iter().fold(0., f32::max);
        let a = eval(lo);
        let b = eval(hi);
        if hi - lo > 1e-8 {
            p.map(|v| srgb_decode(a + (b - a) * (v - lo) / (hi - lo)))
        } else {
            [srgb_decode(a); 3]
        }
    }
}
fn decode_table(text: &str) -> Result<Table> {
    ensure!(text.len() <= 16_000_000, "Look table too large");
    let digits: Vec<_> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    ensure!(digits.len() % 5 != 1, "Truncated base85 table");
    let mut compressed = Vec::with_capacity(digits.len() * 4 / 5);
    for chunk in digits.chunks(5) {
        let mut value = 0u64;
        let mut power = 1u64;
        for b in chunk {
            value += ALPHABET
                .iter()
                .position(|c| c == b)
                .context("Invalid base85 table")? as u64
                * power;
            power *= 85;
        }
        ensure!(value <= u32::MAX as u64, "Base85 overflow");
        compressed.extend_from_slice(&(value as u32).to_le_bytes()[..chunk.len() - 1]);
    }
    ensure!(compressed.len() >= 5, "Truncated compressed table");
    let expected = u32::from_le_bytes(compressed[..4].try_into()?) as usize;
    ensure!(expected <= 12_000_048, "Expanded look table too large");
    let mut data = Vec::new();
    flate2::read::ZlibDecoder::new(&compressed[4..])
        .take(expected as u64 + 1)
        .read_to_end(&mut data)?;
    ensure!(
        data.len() == expected && data.len() >= 24,
        "Invalid expanded table size"
    );
    let word = |i| u32::from_le_bytes(data[i..i + 4].try_into().unwrap());
    ensure!(
        word(0) == 0 && matches!(word(4), 1 | 2),
        "Unsupported big table type/version (requires HSV look table)"
    );
    let dims = [word(8) as usize, word(12) as usize, word(16) as usize];
    ensure!(
        dims.iter().all(|v| (1..=256).contains(v)),
        "Invalid table dimensions"
    );
    let count = dims.iter().product::<usize>();
    ensure!(count <= 1_000_000, "Look table too large");
    let end = 20 + count * 12;
    let tail = if word(4) == 2 { 20 } else { 4 };
    ensure!(
        data.len() == end + tail || data.len() == end + tail + 4,
        "Invalid table payload size"
    );
    ensure!(word(end) <= 1, "Unsupported table encoding");
    if word(4) == 2 {
        let min = f64::from_le_bytes(data[end + 4..end + 12].try_into()?);
        let max = f64::from_le_bytes(data[end + 12..end + 20].try_into()?);
        ensure!(
            min.is_finite() && max.is_finite() && (0. ..=1.).contains(&min) && max >= 1.,
            "Invalid table amount bounds"
        );
    }
    if data.len() == end + tail + 4 {
        ensure!(word(end + tail) == 0, "Unsupported look table flags");
    }
    let table = Table {
        dims,
        srgb: word(end) == 1,
        data: data[20..end]
            .as_chunks::<12>()
            .0
            .iter()
            .map(|p| {
                std::array::from_fn(|c| f32::from_le_bytes(p[c * 4..c * 4 + 4].try_into().unwrap()))
            })
            .collect(),
    };
    table.validate()?;
    Ok(table)
}
pub(super) fn compose(path: &Path, base: &CameraProfile) -> Result<CameraProfile> {
    ensure!(
        std::fs::metadata(path)?.len() <= 16_000_000,
        "XMP profile too large"
    );
    compose_text(&std::fs::read_to_string(path)?, base)
}
pub(super) fn compose_text(text: &str, base: &CameraProfile) -> Result<CameraProfile> {
    let doc = roxmltree::Document::parse(text)?;
    let d = doc
        .descendants()
        .find(|n| {
            n.has_tag_name((RDF, "Description"))
                && n.parent().is_some_and(|p| p.has_tag_name((RDF, "RDF")))
        })
        .context("Missing profile description")?;
    let attr = |name| d.attribute((CRS, name)).unwrap_or("");
    ensure!(attr("PresetType") == "Look", "XMP is not a look profile");
    ensure!(
        attr("CameraProfile") == base.name && base.enhanced.is_none(),
        "Missing base camera profile {}",
        attr("CameraProfile")
    );
    let restriction = attr("CameraModelRestriction");
    ensure!(
        restriction.is_empty() || restriction.eq_ignore_ascii_case(&base.camera),
        "Look belongs to {restriction}"
    );
    // Fail closed: do not silently discard profile-internal develop controls or RGB LUTs.
    const META: &[&str] = &[
        "PresetType",
        "Cluster",
        "UUID",
        "SupportsAmount",
        "SupportsColor",
        "SupportsMonochrome",
        "SupportsHighDynamicRange",
        "SupportsNormalDynamicRange",
        "SupportsSceneReferred",
        "SupportsOutputReferred",
        "CameraModelRestriction",
        "Copyright",
        "ContactInfo",
        "Version",
        "ProcessVersion",
        "ConvertToGrayscale",
        "CameraProfile",
        "LookTable",
        "HasSettings",
        "Highlights2012",
        "Shadows2012",
        "Clarity2012",
    ];
    for a in d.attributes().filter(|a| a.namespace() == Some(CRS)) {
        ensure!(
            META.contains(&a.name()) || a.name().starts_with("Table_"),
            "Unsupported profile setting {}",
            a.name()
        );
    }
    let monochrome = match attr("ConvertToGrayscale").to_ascii_lowercase().as_str() {
        "" | "false" => false,
        "true" => true,
        _ => anyhow::bail!("Invalid profile monochrome flag"),
    };
    let adjustment = |key| -> Result<f32> {
        let value = attr(key);
        Ok(if value.is_empty() {
            0.
        } else {
            value.parse::<f32>()? / 100.
        })
    };
    let name_node = d
        .children()
        .find(|n| n.has_tag_name((CRS, "Name")))
        .context("Missing profile name")?;
    let name = name_node
        .descendants()
        .find(|n| n.has_tag_name((RDF, "li")) && n.attribute((XML, "lang")) == Some("x-default"))
        .and_then(|n| n.text())
        .context("Missing profile name")?;
    let table_id = attr("LookTable");
    ensure!(
        table_id.len() == 32 && table_id.bytes().all(|b| b.is_ascii_hexdigit()),
        "Missing HSV look table"
    );
    let table_key = format!("Table_{table_id}");
    let table = decode_table(attr(&table_key))?;
    let mut curve = ToneCurve::default();
    for n in d
        .children()
        .filter(|n| n.is_element() && n.tag_name().namespace() == Some(CRS))
    {
        let key = n.tag_name().name();
        if key.starts_with("ToneCurvePV2012") {
            let points = n
                .descendants()
                .filter(|v| v.has_tag_name((RDF, "li")))
                .map(|v| -> Result<[f32; 2]> {
                    let (x, y) = v
                        .text()
                        .context("Empty curve point")?
                        .split_once(',')
                        .context("Invalid curve point")?;
                    Ok([
                        x.trim().parse::<f32>()? / 255.,
                        y.trim().parse::<f32>()? / 255.,
                    ])
                })
                .collect::<Result<Vec<_>>>()?;
            let parsed = ToneCurve {
                points,
                ..Default::default()
            };
            parsed.validate()?;
            if key == "ToneCurvePV2012" {
                curve = parsed;
            } else {
                ensure!(
                    [
                        "ToneCurvePV2012Red",
                        "ToneCurvePV2012Green",
                        "ToneCurvePV2012Blue"
                    ]
                    .contains(&key)
                        && parsed.points == [[0., 0.], [1., 1.]],
                    "Unsupported profile channel curve {key}"
                );
            }
        } else {
            ensure!(
                ["Name", "ShortName", "SortName", "Group", "Description"].contains(&key),
                "Unsupported profile element {key}"
            );
        }
    }
    let lut = CurveLut::new(&curve);
    let mut p = base.clone();
    p.name = name.into();
    p.copyright = format!("{}; {}", base.copyright, attr("Copyright"));
    p.enhanced = Some(Enhanced {
        uuid: attr("UUID").into(),
        base_name: base.name.clone(),
        highlights: adjustment("Highlights2012")?,
        shadows: adjustment("Shadows2012")?,
        clarity: adjustment("Clarity2012")?,
        monochrome,
        table,
        curve: (0..=4096).map(|i| lut.evaluate(i as f32 / 4096.)).collect(),
    });
    p.validate()?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn fixture() -> String {
        let mut bytes = Vec::new();
        for v in [0u32, 1, 1, 2, 2] {
            bytes.extend(v.to_le_bytes());
        }
        for _ in 0..4 {
            for v in [120f32, 1., 1.] {
                bytes.extend(v.to_le_bytes());
            }
        }
        bytes.extend(0u32.to_le_bytes());
        let mut compressed = (bytes.len() as u32).to_le_bytes().to_vec();
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&bytes).unwrap();
        compressed.extend(z.finish().unwrap());
        let mut text = String::new();
        for chunk in compressed.chunks(4) {
            let mut word = [0; 4];
            word[..chunk.len()].copy_from_slice(chunk);
            let mut value = u32::from_le_bytes(word);
            for _ in 0..chunk.len() + 1 {
                text.push(ALPHABET[(value % 85) as usize] as char);
                value /= 85;
            }
        }
        text
    }
    fn profile_xml(table: &str) -> String {
        format!(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="{RDF}"><r:Description xmlns:c="{CRS}" c:PresetType="Look" c:UUID="0123456789ABCDEF0123456789ABCDEF" c:CameraProfile="Test base" c:LookTable="0123456789ABCDEF0123456789ABCDEF" c:Table_0123456789ABCDEF0123456789ABCDEF="{table}"><c:Name><r:Alt><r:li xml:lang="x-default">Test look</r:li></r:Alt></c:Name></r:Description></r:RDF></x:xmpmeta>"#
        )
    }
    #[test]
    fn sdk_base85_zlib_table_has_expected_hue_rotation() -> Result<()> {
        let t = decode_table(&fixture())?;
        let green = t.apply([0.5, 0., 0.], None, 0.);
        assert!((green[0]).abs() < 1e-6 && (green[1] - 0.5).abs() < 1e-6 && green[2].abs() < 1e-6);
        for s in ["", "x", "!!!!!", "zzzzzzzzzz", "\"\"\"\"\""] {
            assert!(decode_table(s).is_err());
        }
        Ok(())
    }
    #[test]
    #[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
    fn enhanced_profile_roundtrip_keeps_camera_and_old_profiles_unchanged() -> Result<()> {
        let mut base = CameraProfile::camera_matrix_default(&crate::raw::Metadata {
            make: "Fujifilm".into(),
            model: "X100F".into(),
            cam_xyz: [
                [1.1434, -0.4948, -0.121],
                [-0.3746, 1.2042, 0.1903],
                [-0.0666, 0.1479, 0.5235],
            ],
            ..Default::default()
        })
        .unwrap();
        base.name = "Test base".into();
        let xml = profile_xml(&fixture());
        let composed = compose_text(&xml, &base)?;
        assert_eq!(composed.camera, base.camera);
        assert!(base.enhanced.is_none());
        assert_eq!(composed.name, "Test look");
        let copy: CameraProfile = serde_json::from_slice(&serde_json::to_vec(&composed)?)?;
        assert_eq!(copy, composed);
        copy.validate()?;
        let old: CameraProfile = serde_json::from_slice(&serde_json::to_vec(&base)?)?;
        assert!(old.enhanced.is_none());
        assert!(
            compose_text(
                &xml.replace("c:PresetType=", "c:UnknownOperator=\"10\" c:PresetType="),
                &base
            )
            .is_err()
        );
        assert!(compose_text(&xml.replace("Test base", "Another camera profile"), &base).is_err());
        let look = copy.enhanced.as_ref().unwrap();
        for p in [[0.; 3], [0.18, 0.1, 0.3], [1.; 3]] {
            let result = look.apply_curve(p);
            assert!(result.into_iter().zip(p).all(|(a, b)| (a - b).abs() < 1e-5));
        }
        Ok(())
    }
}
