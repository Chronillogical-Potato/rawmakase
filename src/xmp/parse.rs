use super::Preset;
use crate::develop::curve::ToneCurve;
use anyhow::{Context, Result, ensure};
use std::{collections::BTreeMap, path::Path};
pub(super) const CRS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
pub(super) const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    let child = node.children().find(|n| n.has_tag_name((CRS, name)))?;
    let item = child
        .descendants()
        .find(|n| {
            n.has_tag_name((RDF, "li"))
                && n.attribute(("http://www.w3.org/XML/1998/namespace", "lang"))
                    == Some("x-default")
        })
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
    let mut settings: BTreeMap<_, _> = description
        .attributes()
        .filter(|a| a.namespace() == Some(CRS))
        .map(|a| (a.name().to_string(), a.value().to_string()))
        .collect();
    let mut curves = BTreeMap::new();
    let mut blockers = Vec::new();
    let mut look = String::new();
    for node in description
        .children()
        .filter(|n| n.is_element() && n.tag_name().namespace() == Some(CRS))
    {
        let name = node.tag_name().name();
        if !node.children().any(|n| n.is_element())
            && ![
                "Name",
                "Group",
                "ShortName",
                "SortName",
                "Description",
                "Look",
                "PointColors",
                "ColorVariance",
            ]
            .contains(&name)
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
                ensure!(
                    amount.parse::<f32>()? == 1.,
                    "Profile Amount other than 100% is not supported"
                );
            }
            if let Some(uuid) = d.attribute((CRS, "UUID")) {
                settings.insert("RAWmakaseLookUUID".into(), uuid.into());
            }
        } else if name == "Preset"
            && description
                .attribute(("http://ns.adobe.com/photoshop/1.0/", "SidecarForExtension"))
                .is_some()
        {
            // In a photo sidecar, the nested preset-amount record is provenance.
            // The resolved top-level edits (including curves) remain authoritative.
        } else if matches!(name, "PointColors" | "ColorVariance") && {
            // Lightroom 18.5 writes these exact empty-selection sentinels.
            let values: Vec<_> = node
                .descendants()
                .filter(|n| n.has_tag_name((RDF, "li")))
                .filter_map(|n| n.text())
                .flat_map(|s| s.split(','))
                .map(|v| v.trim().parse::<f32>())
                .collect();
            if name == "PointColors" {
                values.len() == 19 && values.iter().all(|v| matches!(v, Ok(n) if *n == -1.))
            } else {
                values.len() == 1
                    && matches!(values[0], Ok(n) if n == -50.)
                    && description
                        .children()
                        .find(|n| n.has_tag_name((CRS, "PointColors")))
                        .is_some_and(|n| {
                            let values: Vec<_> = n
                                .descendants()
                                .filter(|n| n.has_tag_name((RDF, "li")))
                                .filter_map(|n| n.text())
                                .flat_map(|s| s.split(','))
                                .collect();
                            values.len() == 19
                                && values.iter().all(|v| v.trim().parse::<f32>() == Ok(-1.))
                        })
            }
        } {
            // No selected point color to adjust.
        } else if !["Name", "Group", "ShortName", "SortName", "Description"].contains(&name) {
            let active = node.descendants().any(|n| {
                n.has_tag_name((RDF, "li")) && n.text().is_some_and(|s| !s.trim().is_empty())
            });
            if active || !matches!(name, "PointColors" | "ColorVariance") {
                blockers.push(format!("Unsupported structured setting: {name}"));
            }
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
    let preset = Preset {
        photo_settings: description
            .attribute(("http://ns.adobe.com/photoshop/1.0/", "SidecarForExtension"))
            .is_some(),
        id,
        name,
        group,
        path: path.to_path_buf(),
        settings,
        curves,
        look,
        blockers,
        notes,
    };
    ensure!(
        !preset.settings.is_empty() || !preset.curves.is_empty(),
        "No camera settings"
    );
    Ok(preset)
}
