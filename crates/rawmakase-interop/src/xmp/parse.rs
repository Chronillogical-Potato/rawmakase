use super::Preset;
use crate::color::curve::ToneCurve;
use crate::xml::ns::{CRS, PHOTOSHOP, RDF, XML};
use anyhow::{Context, Result, ensure};
use std::{collections::BTreeMap, path::Path};
/// A preset's name and description, not settings.
const LABELS: [&str; 5] = ["Name", "Group", "ShortName", "SortName", "Description"];
/// Lightroom 18.5's empty point color selection: 19 values of -1.
fn is_empty_point_colors(node: roxmltree::Node<'_, '_>) -> bool {
    let values: Vec<_> = node
        .descendants()
        .filter(|n| n.has_tag_name((RDF, "li")))
        .filter_map(|n| n.text())
        .flat_map(|s| s.split(','))
        .collect();
    values.len() == 19 && values.iter().all(|v| v.trim().parse::<f32>() == Ok(-1.))
}
fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    let child = node.children().find(|n| n.has_tag_name((CRS, name)))?;
    let item = child
        .descendants()
        .find(|n| n.has_tag_name((RDF, "li")) && n.attribute((XML, "lang")) == Some("x-default"))
        .or_else(|| child.descendants().find(|n| n.has_tag_name((RDF, "li"))));
    Some(
        item.and_then(|n| n.text())
            .or_else(|| child.text())
            .unwrap_or("")
            .trim()
            .to_string(),
    )
}
pub fn parse(path: &Path, text: &str) -> Result<Preset> {
    ensure!(text.len() < 8_000_000, "XMP file too large");
    // A specific malformed export in this collection repeats the Group close tag.
    // Repair only consecutive duplicate Group closes; never rewrite the source file.
    let mut normalized = String::with_capacity(text.len());
    let mut previous = "";
    let mut repaired = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "</crs:Group>" && previous == trimmed {
            repaired = true;
            continue;
        }
        normalized.push_str(line);
        normalized.push('\n');
        previous = trimmed;
    }
    let doc = roxmltree::Document::parse(&normalized)?;
    let description = doc
        .descendants()
        .find(|n| {
            n.has_tag_name((RDF, "Description"))
                && (n.attributes().any(|a| a.namespace() == Some(CRS))
                    || n.children().any(|c| c.tag_name().namespace() == Some(CRS)))
                && n.parent().is_some_and(|p| p.has_tag_name((RDF, "RDF")))
        })
        .context("No top-level XMP description")?;
    let sidecar = description
        .attribute((PHOTOSHOP, "SidecarForExtension"))
        .is_some();
    let mut settings: BTreeMap<_, _> = description
        .attributes()
        .filter(|a| a.namespace() == Some(CRS))
        .map(|a| (a.name().to_string(), a.value().to_string()))
        .collect();
    let mut curves = BTreeMap::new();
    let mut blockers = Vec::new();
    let mut look = String::new();
    let mut local = BTreeMap::new();
    for node in description
        .children()
        .filter(|n| n.is_element() && n.tag_name().namespace() == Some(CRS))
    {
        let name = node.tag_name().name();
        if !node.children().any(|n| n.is_element())
            && !LABELS.contains(&name)
            && !matches!(name, "Look" | "PointColors" | "ColorVariance")
            && let Some(text) = node.text()
        {
            settings.insert(name.to_string(), text.trim().to_string());
            continue;
        }
        if [
            "ToneCurve",
            "ToneCurveRed",
            "ToneCurveGreen",
            "ToneCurveBlue",
            "ToneCurvePV2012",
            "ToneCurvePV2012Red",
            "ToneCurvePV2012Green",
            "ToneCurvePV2012Blue",
        ]
        .contains(&name)
        {
            let mut points = Vec::new();
            for item in node.descendants().filter(|n| n.has_tag_name((RDF, "li"))) {
                let v = item
                    .text()
                    .context("Empty curve point")?
                    .split(',')
                    .map(|s| s.trim().parse::<f32>())
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                ensure!(v.len() == 2, "Invalid curve point");
                points.push([v[0] / 255., v[1] / 255.]);
            }
            let curve = ToneCurve {
                points,
                smooth: true,
                natural: true,
            };
            curve.validate()?;
            curves.insert(name.to_string(), curve);
        } else if super::local::KEYS.contains(&name) {
            let value = super::local::Node::from_xml(node);
            if !value.is_empty() {
                local.insert(name.to_string(), value);
            }
        } else if name == "Look" {
            let d = node
                .descendants()
                .find(|n| n.has_tag_name((RDF, "Description")))
                .unwrap_or(node);
            look = d
                .attribute((CRS, "Name"))
                .map(str::to_string)
                .or_else(|| child_text(d, "Name"))
                .unwrap_or_default();
            if let Some(amount) = d.attribute((CRS, "Amount")) {
                let amount = super::look::LookAmount::parse(amount)?;
                settings.insert(super::look::SETTING.into(), amount.0.to_string());
            }
            if let Some(uuid) = d.attribute((CRS, "UUID")).filter(|_| !look.is_empty()) {
                settings.insert("RAWmakaseLookUUID".into(), uuid.into());
            }
        } else if name == "Preset" && sidecar {
            // In a photo sidecar, the nested preset-amount record is provenance.
            // The resolved top-level edits (including curves) remain authoritative.
        } else if matches!(name, "PointColors" | "ColorVariance") {
            // Lightroom writes an empty selection as 19 values of -1 (and a variance
            // of -50); `apply` reads both as no swatches.
            let items: Vec<_> = node
                .descendants()
                .filter(|n| n.has_tag_name((RDF, "li")))
                .map(|n| n.text().unwrap_or("").trim().to_string())
                .collect();
            let empty = name == "PointColors" && is_empty_point_colors(node);
            let value = if empty {
                String::new()
            } else {
                items.join("; ")
            };
            settings.insert(name.to_string(), value);
        } else if !LABELS.contains(&name) {
            blockers.push(format!("Unsupported structured setting: {name}"));
        }
    }
    let name = child_text(description, "Name")
        .filter(|s| !s.is_empty())
        .or_else(|| settings.get("Name").cloned())
        .unwrap_or_else(|| {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        });
    let fallback = path
        .parent()
        .and_then(|p| p.file_name())
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let group = child_text(description, "Group")
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback);
    let id = path.to_string_lossy().to_string();
    let mut notes = Vec::new();
    if repaired {
        notes.push("Recovered duplicated XML Group closing tag; original file unchanged".into());
    }
    // A packet or preset from a RAWmakase that predates `RAWmakaseMarkers` keeps the
    // operators measured only since, besides any it names.
    let creator_tool = description
        .attribute((crate::xml::ns::XMP, "CreatorTool"))
        .map(str::to_string)
        .or_else(|| {
            description
                .children()
                .find(|n| n.has_tag_name((crate::xml::ns::XMP, "CreatorTool")))
                .and_then(|n| n.text())
                .map(|t| t.trim().to_string())
        });
    add_implied_original(&mut settings, creator_tool.as_deref());
    let preset = Preset {
        photo_settings: sidecar,
        id,
        name,
        group,
        path: path.to_path_buf(),
        settings,
        curves,
        look,
        blockers,
        notes,
        local,
        builtin: false,
    };
    ensure!(
        !preset.settings.is_empty() || !preset.curves.is_empty(),
        "No camera settings"
    );
    Ok(preset)
}

/// Names in `RAWmakaseOriginal` the operators a packet or preset from an earlier
/// RAWmakase kept without naming them, among those whose settings it carries: the
/// ones its marker format could not name, or, without `RAWmakaseMarkers`, the ones
/// measured after its release. Presets record no release; theirs is taken as the last
/// one without the marker.
fn add_implied_original(settings: &mut BTreeMap<String, String>, creator_tool: Option<&str>) {
    let unnamed = if let Some(markers) = settings.get("RAWmakaseMarkers") {
        // An unreadable format is taken as the current one.
        let markers = markers.trim().parse().unwrap_or(super::write::MARKERS);
        super::write::unnamed_by_markers(markers)
    } else {
        let version = match creator_tool {
            Some(tool) => super::write::rawmakase_version(tool),
            None if settings.contains_key("RAWmakasePreset") => Some((0, 1, 15)),
            None => None,
        };
        let Some(version) = version else {
            return;
        };
        super::write::implied_original(version)
    };
    let implied: Vec<_> = unnamed
        .into_iter()
        .filter(|name| {
            super::write::operator_keys(name)
                .iter()
                .any(|key| settings.keys().any(|k| key.matches(k)))
        })
        .collect();
    if implied.is_empty() {
        return;
    }
    let kept = settings.entry("RAWmakaseOriginal".to_string()).or_default();
    for name in implied {
        if !kept.split(',').any(|n| n.trim() == name) {
            if !kept.is_empty() {
                kept.push(',');
            }
            kept.push_str(name);
        }
    }
}
