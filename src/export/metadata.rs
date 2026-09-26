use crate::raw::Metadata;

pub(super) fn description(m: &Metadata) -> String {
    format!(
        "RAWmakase; ISO {}; exposure {} s; f/{}; focal length {} mm",
        m.iso, m.shutter, m.aperture, m.focal
    )
}
/// Minimal TIFF-format EXIF IFD; safe selected metadata, no maker notes or thumbnail offsets.
pub(super) fn exif(m: &Metadata) -> Vec<u8> {
    type Entry = (u16, u16, u32, Vec<u8>);
    fn directory(mut entries: Vec<Entry>, offset: u32) -> Vec<u8> {
        entries.sort_by_key(|e| e.0);
        let mut out = (entries.len() as u16).to_le_bytes().to_vec();
        let mut payload = Vec::new();
        let start = offset + 2 + entries.len() as u32 * 12 + 4;
        for (tag, kind, count, mut bytes) in entries {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(count.to_le_bytes());
            if bytes.len() <= 4 {
                bytes.resize(4, 0);
                out.extend(bytes);
            } else {
                out.extend((start + payload.len() as u32).to_le_bytes());
                payload.extend(bytes);
                if payload.len() % 2 != 0 {
                    payload.push(0);
                }
            }
        }
        out.extend(0u32.to_le_bytes());
        out.extend(payload);
        out
    }
    fn ascii(tag: u16, value: &str) -> Entry {
        let mut b = value.as_bytes().to_vec();
        b.push(0);
        (tag, 2, b.len() as u32, b)
    }
    fn rational(tag: u16, value: f32) -> Entry {
        let mut b = ((value.max(0.) * 1_000_000.).round() as u32)
            .to_le_bytes()
            .to_vec();
        b.extend(1_000_000u32.to_le_bytes());
        (tag, 5, 1, b)
    }
    let mut main = vec![
        ascii(0x010f, &m.make),
        ascii(0x0110, &m.model),
        ascii(0x0131, "RAWmakase 0.1"),
        ascii(0x010e, &description(m)),
        (0x0112, 3, 1, 1u16.to_le_bytes().to_vec()),
        (0x8769, 4, 1, vec![0; 4]),
    ];
    let offset = 8 + directory(main.clone(), 8).len() as u32;
    main.last_mut().unwrap().3 = offset.to_le_bytes().to_vec();
    let sub = vec![
        rational(0x829a, m.shutter),
        rational(0x829d, m.aperture),
        (
            0x8827,
            3,
            1,
            (m.iso.min(65535.) as u16).to_le_bytes().to_vec(),
        ),
        rational(0x920a, m.focal),
        (0xa001, 3, 1, 1u16.to_le_bytes().to_vec()),
    ];
    let mut out = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
    out.extend(directory(main, 8));
    out.extend(directory(sub, offset));
    out
}
