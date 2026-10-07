//! XML escaping, the packet wrapper of the XMP RAWmakase writes, and the XMP
//! namespaces ([`ns`]) that readers and writers across the crate share.
pub mod ns;
use ns::RDF;

/// Text or an attribute value as `xmp::write` writes it: markup escaped,
/// tabs and line breaks kept as they are and other control characters,
/// which XML 1.0 does not allow, dropped.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// An attribute value as XML writes it, whitespace included: tabs and line
/// breaks as character references, so a parsed value (whose whitespace a
/// parser normalizes otherwise) is written back unchanged.
pub fn escape_attribute(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\t', "&#x9;")
        .replace('\n', "&#xA;")
        .replace('\r', "&#xD;")
}

/// `description`, one `rdf:Description` ending in a line break, in its
/// `x:xmpmeta` and `rdf:RDF` elements.
pub fn xmpmeta(description: &str) -> String {
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n <rdf:RDF xmlns:rdf=\"{RDF}\">\n{description} </rdf:RDF>\n</x:xmpmeta>"
    )
}

/// A packet: `xmpmeta` with the xpacket wrapper.
pub fn packet(description: &str) -> String {
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n{}\n<?xpacket end=\"w\"?>",
        xmpmeta(description)
    )
}
