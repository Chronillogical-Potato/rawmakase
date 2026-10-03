//! XMP too large for one JPEG segment, split as the XMP specification (part 3,
//! "ExtendedXMP in JPEG") and Adobe's apps do: the standard packet keeps what
//! fits and names the rest by the MD5 of its serialization in
//! `xmpNote:HasExtendedXMP`; the rest follows in chunks of its own segments.
//! Camera Raw settings move first, as Adobe moves them, then the largest
//! properties until the standard packet fits.
use anyhow::{Context, Result, ensure};
use md5::{Digest, Md5};

/// What a standard XMP segment holds: 65,535 bytes less the length field and
/// the 29-byte namespace header.
pub(super) const STANDARD_MAX: usize = 65_535 - 2 - STANDARD_HEADER.len();
pub(super) const STANDARD_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
pub(super) const EXTENDED_HEADER: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
/// Data per extended segment: what is left after the header, the GUID and
/// the full length and offset.
const CHUNK: usize = 65_535 - 2 - EXTENDED_HEADER.len() - 32 - 4 - 4;
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const CRS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
const NOTE: &str = "http://ns.adobe.com/xmp/note/";

/// A property of the packet's description, as written.
struct Property {
    /// Its XML, `prefix:Name="value"` for an attribute or the element.
    xml: String,
    attribute: bool,
    camera_raw: bool,
}

/// The standard packet and, when it does not fit one segment, the GUID and
/// serialization of the extended one.
pub(super) struct Split {
    pub standard: String,
    pub extended: Option<(String, String)>,
}

/// Splits `packet` (one `rdf:Description`, as `xmp::write` makes) so its
/// standard part fits a segment.
pub(super) fn split(packet: &str) -> Result<Split> {
    if packet.len() <= STANDARD_MAX {
        return Ok(Split {
            standard: packet.into(),
            extended: None,
        });
    }
    let doc = roxmltree::Document::parse(packet).context("XMP packet is not valid XML")?;
    let description = doc
        .descendants()
        .find(|n| n.has_tag_name((RDF, "Description")))
        .context("XMP packet has no description")?;
    let mut namespaces: Vec<String> = description
        .namespaces()
        .filter_map(|ns| Some(format!("xmlns:{}=\"{}\"", ns.name()?, ns.uri())))
        .collect();
    namespaces.sort();
    namespaces.dedup();
    let prefix = |uri: &str| description.lookup_prefix(uri).unwrap_or("");
    let mut properties: Vec<Property> = description
        .attributes()
        .filter(|a| a.namespace().is_some_and(|ns| ns != RDF))
        .map(|a| Property {
            xml: format!(
                "{}:{}=\"{}\"",
                prefix(a.namespace().unwrap_or("")),
                a.name(),
                escape(a.value())
            ),
            attribute: true,
            camera_raw: a.namespace() == Some(CRS),
        })
        .collect();
    properties.extend(
        description
            .children()
            .filter(|n| n.is_element())
            .map(|n| Property {
                xml: packet[n.range()].to_string(),
                attribute: false,
                camera_raw: n.tag_name().namespace() == Some(CRS),
            }),
    );
    // Then the largest, until the rest fits.
    let mut order: Vec<usize> = (0..properties.len()).collect();
    order.retain(|i| !properties[*i].camera_raw);
    order.sort_by_key(|i| std::cmp::Reverse(properties[*i].xml.len()));
    // All Camera Raw settings move together, as Adobe moves them.
    let mut moved: Vec<bool> = properties.iter().map(|p| p.camera_raw).collect();
    let mut placeholder = "0".repeat(32);
    for i in order.iter().copied().chain([usize::MAX]) {
        let kept = description_xml(&namespaces, &properties, &moved, false, Some(&placeholder));
        if wrap(&kept).len() <= STANDARD_MAX {
            break;
        }
        ensure!(
            i != usize::MAX,
            "XMP is too large for a JPEG even when split"
        );
        moved[i] = true;
    }
    let extended = xmpmeta(&description_xml(
        &namespaces,
        &properties,
        &moved,
        true,
        None,
    ));
    let guid: String = Md5::digest(extended.as_bytes())
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    placeholder = guid.clone();
    let standard = wrap(&description_xml(
        &namespaces,
        &properties,
        &moved,
        false,
        Some(&placeholder),
    ));
    Ok(Split {
        standard,
        extended: Some((guid, extended)),
    })
}

/// The extended segments of `extended`: each its header, GUID, full length,
/// offset and chunk.
pub(super) fn segments(guid: &str, extended: &str) -> Vec<Vec<u8>> {
    let data = extended.as_bytes();
    data.chunks(CHUNK)
        .enumerate()
        .map(|(i, chunk)| {
            let mut segment = Vec::with_capacity(EXTENDED_HEADER.len() + 40 + chunk.len());
            segment.extend_from_slice(EXTENDED_HEADER);
            segment.extend_from_slice(guid.as_bytes());
            segment.extend((data.len() as u32).to_be_bytes());
            segment.extend(((i * CHUNK) as u32).to_be_bytes());
            segment.extend_from_slice(chunk);
            segment
        })
        .collect()
}

/// One description holding the properties moved (`moved_part`) or kept,
/// with the standard part's pointer to the extended one.
fn description_xml(
    namespaces: &[String],
    properties: &[Property],
    moved: &[bool],
    moved_part: bool,
    guid: Option<&str>,
) -> String {
    let mut attributes = Vec::new();
    let mut elements = Vec::new();
    for (p, m) in properties.iter().zip(moved) {
        if *m == moved_part {
            if p.attribute {
                attributes.push(p.xml.as_str());
            } else {
                elements.push(p.xml.as_str());
            }
        }
    }
    let mut out = String::from("  <rdf:Description rdf:about=\"\"");
    for ns in namespaces {
        out.push_str("\n    ");
        out.push_str(ns);
    }
    if guid.is_some() {
        out.push_str(&format!("\n    xmlns:xmpNote=\"{NOTE}\""));
    }
    for a in attributes {
        out.push_str("\n   ");
        out.push_str(a);
    }
    if let Some(guid) = guid {
        out.push_str(&format!("\n   xmpNote:HasExtendedXMP=\"{guid}\""));
    }
    out.push_str(">\n");
    for e in elements {
        out.push_str("   ");
        out.push_str(e);
        out.push('\n');
    }
    out.push_str("  </rdf:Description>\n");
    out
}

fn xmpmeta(description: &str) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"{RDF}\">\n{description} </rdf:RDF>\n</x:xmpmeta>"
    )
}
/// A standard packet, with the xpacket wrapper.
fn wrap(description: &str) -> String {
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n{}\n<?xpacket end=\"w\"?>",
        xmpmeta(description)
    )
}

/// An attribute value as XML writes it, whitespace included.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\t', "&#x9;")
        .replace('\n', "&#xA;")
        .replace('\r', "&#xD;")
}
