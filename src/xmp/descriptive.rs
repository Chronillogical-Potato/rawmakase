//! Reads descriptive metadata from an XMP packet: an XMP sidecar, a JPEG's or
//! TIFF's embedded XMP, or the XMP a Lightroom catalog keeps per photo.
//! Namespaces are matched by URI, in attribute and element forms. Lightroom's
//! and digiKam's properties are read as digiKam documents and writes them.
use crate::catalog::{Capture, LangAlt, Location, Value};
use anyhow::{Result, ensure};
use roxmltree::Node;

const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const DC: &str = "http://purl.org/dc/elements/1.1/";
const XMP: &str = "http://ns.adobe.com/xap/1.0/";
const EXIF: &str = "http://ns.adobe.com/exif/1.0/";
const PHOTOSHOP: &str = "http://ns.adobe.com/photoshop/1.0/";
const LR: &str = "http://ns.adobe.com/lightroom/1.0/";
const DIGIKAM: &str = "http://www.digikam.org/ns/1.0/";
const XML: &str = "http://www.w3.org/XML/1998/namespace";

/// What a packet holds. `None` is a field the packet does not have; an
/// explicitly empty one (an empty list or string) is `Value::Cleared`, or an
/// empty list of keywords.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Read {
    pub title: Option<Value<LangAlt>>,
    pub caption: Option<Value<LangAlt>>,
    pub copyright: Option<Value<LangAlt>>,
    pub creator: Option<Value<Vec<String>>>,
    /// Keyword paths, top first.
    pub keywords: Option<Vec<Vec<String>>>,
    pub rating: Option<i32>,
    pub label: Option<String>,
    /// Lightroom's flag: 1 picked, -1 rejected, 0 neither.
    pub flag: Option<i32>,
    pub capture: Option<Capture>,
    pub location: Option<Location>,
}
impl Read {
    /// Each field from `self`, else from `fallback`: a sidecar over a file's
    /// embedded XMP. An explicitly empty field stays empty.
    pub fn or(self, fallback: Read) -> Read {
        Read {
            title: self.title.or(fallback.title),
            caption: self.caption.or(fallback.caption),
            copyright: self.copyright.or(fallback.copyright),
            creator: self.creator.or(fallback.creator),
            keywords: self.keywords.or(fallback.keywords),
            rating: self.rating.or(fallback.rating),
            label: self.label.or(fallback.label),
            flag: self.flag.or(fallback.flag),
            capture: self.capture.or(fallback.capture),
            location: self.location.or(fallback.location),
        }
    }
}

/// A property of any top-level description, attribute or element.
enum Property<'a, 'i> {
    Attribute(&'a str),
    Element(Node<'a, 'i>),
}

struct Packet<'a, 'i> {
    descriptions: Vec<Node<'a, 'i>>,
}
impl<'a, 'i> Packet<'a, 'i> {
    fn get(&self, ns: &str, name: &str) -> Option<Property<'a, 'i>> {
        for d in &self.descriptions {
            if let Some(v) = d.attribute((ns, name)) {
                return Some(Property::Attribute(v));
            }
            if let Some(e) = d.children().find(|c| c.has_tag_name((ns, name))) {
                return Some(Property::Element(e));
            }
        }
        None
    }
    /// A simple value's text.
    fn text(&self, ns: &str, name: &str) -> Option<String> {
        match self.get(ns, name)? {
            Property::Attribute(v) => Some(v.trim().to_string()),
            Property::Element(e) => Some(
                e.attribute((RDF, "resource"))
                    .or_else(|| e.text())
                    .unwrap_or("")
                    .trim()
                    .to_string(),
            ),
        }
    }
    /// An array's items (Alt, Bag or Seq), with their languages; a simple
    /// value as one item.
    fn items(&self, ns: &str, name: &str) -> Option<Vec<(Option<String>, String)>> {
        Some(match self.get(ns, name)? {
            Property::Attribute(v) => one(v),
            Property::Element(e) => {
                let array = e.children().find(|c| {
                    c.is_element()
                        && c.tag_name().namespace() == Some(RDF)
                        && matches!(c.tag_name().name(), "Alt" | "Bag" | "Seq")
                });
                match array {
                    Some(array) => array
                        .children()
                        .filter(|c| c.has_tag_name((RDF, "li")))
                        .map(|li| {
                            (
                                li.attribute((XML, "lang")).map(String::from),
                                li.text().unwrap_or("").trim().to_string(),
                            )
                        })
                        .filter(|(_, t)| !t.is_empty())
                        .collect(),
                    None => one(e.text().unwrap_or("")),
                }
            }
        })
    }
    fn lang_alt(&self, ns: &str, name: &str) -> Option<Value<LangAlt>> {
        let items = self.items(ns, name)?;
        Some(if items.is_empty() {
            Value::Cleared
        } else {
            Value::Set(LangAlt(
                items
                    .into_iter()
                    .map(|(lang, text)| (lang.unwrap_or_else(|| "x-default".into()), text))
                    .collect(),
            ))
        })
    }
    fn list(&self, ns: &str, name: &str) -> Option<Vec<String>> {
        Some(self.items(ns, name)?.into_iter().map(|(_, t)| t).collect())
    }
}
fn one(text: &str) -> Vec<(Option<String>, String)> {
    let text = text.trim();
    if text.is_empty() {
        Vec::new()
    } else {
        vec![(None, text.to_string())]
    }
}

/// Reads `text`, an XMP packet or sidecar.
pub fn read(text: &str) -> Result<Read> {
    ensure!(text.len() < 16_000_000, "XMP is too large");
    let doc = roxmltree::Document::parse(text)?;
    let descriptions: Vec<Node> = doc
        .descendants()
        .filter(|n| {
            n.has_tag_name((RDF, "Description"))
                && n.parent().is_some_and(|p| p.has_tag_name((RDF, "RDF")))
        })
        .collect();
    ensure!(!descriptions.is_empty(), "No XMP description");
    let p = Packet { descriptions };
    Ok(Read {
        title: p.lang_alt(DC, "title"),
        caption: p.lang_alt(DC, "description"),
        copyright: p.lang_alt(DC, "rights"),
        creator: p.list(DC, "creator").map(|names| {
            if names.is_empty() {
                Value::Cleared
            } else {
                Value::Set(names)
            }
        }),
        keywords: keywords(&p),
        // An empty rating is no rating, not a missing one.
        rating: p.text(XMP, "Rating").and_then(|r| {
            if r.is_empty() {
                Some(0)
            } else {
                r.parse::<f64>()
                    .ok()
                    .map(|r| (r.round() as i32).clamp(0, 5))
            }
        }),
        label: p.text(XMP, "Label").or_else(|| {
            p.text(DIGIKAM, "ColorLabel")
                .and_then(|c| digikam_label(&c))
        }),
        flag: p
            .text(DIGIKAM, "PickLabel")
            .and_then(|f| match f.as_str() {
                "1" => Some(-1),
                "3" => Some(1),
                "0" | "2" => Some(0),
                _ => None,
            })
            .or_else(|| {
                // Adobe writes -1 for a rejected photo.
                (p.text(XMP, "Rating").as_deref() == Some("-1")).then_some(-1)
            }),
        capture: [
            (EXIF, "DateTimeOriginal"),
            (PHOTOSHOP, "DateCreated"),
            (XMP, "CreateDate"),
        ]
        .into_iter()
        .find_map(|(ns, name)| capture(&p.text(ns, name)?)),
        location: location(&p),
    })
}

/// Keywords, by Lightroom's paths where present, else digiKam's; then the
/// flat dc:subject names no path holds, as top-level keywords (a tool that
/// writes only dc:subject may have added them since). With neither path
/// property, dc:subject is the flat keywords.
fn keywords(p: &Packet) -> Option<Vec<Vec<String>>> {
    let split = |items: Vec<String>, separator: char| -> Vec<Vec<String>> {
        items
            .into_iter()
            .map(|i| {
                i.split(separator)
                    .map(|n| n.trim().to_string())
                    .filter(|n| !n.is_empty())
                    .collect::<Vec<_>>()
            })
            // A hierarchy deeper than the catalog reads back is left out.
            .filter(|path| !path.is_empty() && path.len() < 256)
            .collect()
    };
    let hierarchical = p
        .list(LR, "hierarchicalSubject")
        .map(|i| split(i, '|'))
        .or_else(|| p.list(DIGIKAM, "TagsList").map(|i| split(i, '/')));
    let subject = p.list(DC, "subject");
    match (hierarchical, subject) {
        (None, None) => None,
        (None, Some(flat)) => Some(flat.into_iter().map(|n| vec![n]).collect()),
        (Some(paths), subject) => {
            let mut paths = paths;
            for name in subject.unwrap_or_default() {
                if !paths.iter().flatten().any(|n| *n == name) {
                    paths.push(vec![name]);
                }
            }
            Some(paths)
        }
    }
}

/// digiKam's color labels by number, as Lightroom names its own.
fn digikam_label(number: &str) -> Option<String> {
    Some(
        match number {
            "0" => "",
            "1" => "Red",
            "2" => "Orange",
            "3" => "Yellow",
            "4" => "Green",
            "5" => "Blue",
            "6" => "Purple",
            "7" => "Gray",
            "8" => "Black",
            "9" => "White",
            _ => return None,
        }
        .to_string(),
    )
}

/// An XMP date ("2024-05-01T12:30:15.12+02:00") with its time; a date alone
/// is not a capture time.
fn capture(text: &str) -> Option<Capture> {
    let text = text.trim();
    let (date, rest) = text.split_once('T')?;
    let date_ok = date.len() == 10
        && date.bytes().enumerate().all(|(i, b)| {
            if i == 4 || i == 7 {
                b == b'-'
            } else {
                b.is_ascii_digit()
            }
        });
    if !date_ok {
        return None;
    }
    let (time, offset) = match rest.find(['Z', '+', '-']) {
        Some(at) => (&rest[..at], Some(&rest[at..])),
        None => (rest, None),
    };
    let (time, subsec) = match time.split_once('.') {
        Some((t, s)) => (t, Some(s)),
        None => (time, None),
    };
    let time = match time.len() {
        5 => format!("{time}:00"),
        8 => time.to_string(),
        _ => return None,
    };
    if !time.bytes().enumerate().all(|(i, b)| {
        if i == 2 || i == 5 {
            b == b':'
        } else {
            b.is_ascii_digit()
        }
    }) {
        return None;
    }
    let offset = match offset {
        Some("Z") => Some("+00:00".to_string()),
        Some(o) if o.len() == 6 && o.as_bytes()[3] == b':' => Some(o.to_string()),
        Some(_) => return None,
        None => None,
    };
    // A real day and time, not only their shape.
    let n = |s: &str| s.parse::<u32>().ok();
    let (year, month, day) = (n(&date[..4])?, n(&date[5..7])?, n(&date[8..10])?);
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    let (hour, minute, second) = (n(&time[..2])?, n(&time[3..5])?, n(&time[6..8])?);
    if day == 0 || day > days || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    if let Some(o) = &offset
        && (n(&o[1..3])? * 60 + n(&o[4..6])? > 14 * 60 || n(&o[4..6])? > 59)
    {
        return None;
    }
    let captured = format!("{date}T{time}");
    // A date the catalog can sort by: never 0000-00-00.
    crate::export::exif::lightroom_time(&captured, None)?;
    Some(Capture {
        captured,
        subsec: subsec
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .map(String::from),
        offset,
    })
}

/// exif:GPSLatitude and GPSLongitude ("52,13.782N" or "52,13,46.92N"), with
/// exif:GPSAltitude ("1005/10") above or below sea level by GPSAltitudeRef.
fn location(p: &Packet) -> Option<Location> {
    let coordinate = |text: String| -> Option<f64> {
        let text = text.trim();
        let hemisphere = text.chars().last()?;
        let sign = match hemisphere.to_ascii_uppercase() {
            'N' | 'E' => 1.,
            'S' | 'W' => -1.,
            _ => return None,
        };
        let parts: Vec<f64> = text[..text.len() - 1]
            .split(',')
            .map(|n| n.trim().parse().ok())
            .collect::<Option<_>>()?;
        let value = match parts[..] {
            [d, m] => d + m / 60.,
            [d, m, s] => d + m / 60. + s / 3600.,
            _ => return None,
        };
        Some(sign * value)
    };
    let (lat, lon) = (p.text(EXIF, "GPSLatitude")?, p.text(EXIF, "GPSLongitude")?);
    // Both there and empty: the location was removed.
    if lat.is_empty() && lon.is_empty() {
        return Some(Location::Cleared);
    }
    let (lat, lon) = (coordinate(lat)?, coordinate(lon)?);
    if !(-90.0..=90.).contains(&lat) || !(-180.0..=180.).contains(&lon) {
        return None;
    }
    let alt = p.text(EXIF, "GPSAltitude").and_then(|a| {
        let value = match a.split_once('/') {
            Some((n, d)) => n.trim().parse::<f64>().ok()? / d.trim().parse::<f64>().ok()?,
            None => a.trim().parse().ok()?,
        };
        let below = p.text(EXIF, "GPSAltitudeRef").as_deref() == Some("1");
        value
            .is_finite()
            .then_some(if below { -value } else { value })
    });
    Some(Location::At { lat, lon, alt })
}

#[cfg(test)]
mod tests;
