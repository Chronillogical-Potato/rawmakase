//! Raster data too large for recipe JSON, such as AI masks and removal patches,
//! stored once by content hash and referenced by that hash from the recipe. Catalog
//! photos keep them in the catalog's `bitmaps` table; photos opened directly carry them
//! in their sidecar's `bitmaps` map, as base64 of the compressed bytes.
use super::{FNV_OFFSET, fnv1a};
use anyhow::{Context, Result, ensure};
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use std::io::{Read, Write};

/// Largest decompressed bitmap accepted: 4096² samples of 4 bytes.
const MAX_BYTES: usize = 64 << 20;
const MAGIC: &[u8; 4] = b"RMBM";

/// Interleaved 8- or 16-bit samples, row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    /// Samples per pixel, 1–4.
    pub channels: u8,
    /// Bytes per sample, 1 or 2 (little-endian, e.g. f16 patches).
    pub depth: u8,
    pub data: Vec<u8>,
}
impl Bitmap {
    /// Decoded size in bytes, `None` when it overflows.
    fn byte_len(width: u32, height: u32, channels: u8, depth: u8) -> Option<usize> {
        (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(channels as usize)?
            .checked_mul(depth as usize)
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            (1..=4).contains(&self.channels)
                && (1..=2).contains(&self.depth)
                && self.width > 0
                && self.height > 0,
            "Invalid bitmap"
        );
        let len = Self::byte_len(self.width, self.height, self.channels, self.depth);
        ensure!(
            len == Some(self.data.len()) && self.data.len() <= MAX_BYTES,
            "Bitmap size mismatch"
        );
        Ok(())
    }
    /// Content hash (128-bit, hex) of the bitmap, which names it in the store.
    pub fn hash(&self) -> String {
        let header = [
            &self.width.to_le_bytes()[..],
            &self.height.to_le_bytes(),
            &[self.channels, self.depth],
        ]
        .concat();
        let fnv = |seed| fnv1a(fnv1a(seed, &header), &self.data);
        format!("{:016x}{:016x}", fnv(FNV_OFFSET), fnv(0x84222325cbf29ce4))
    }
    /// The content ID new assets are stored under: `sha256:` and the hex SHA-256 of
    /// a canonical header (format, dimensions, channels, sample depth) and the
    /// uncompressed samples. Older assets keep their [`Self::hash`] names.
    pub fn content_id(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut sha = Sha256::new();
        sha.update(b"RMBM1");
        sha.update(self.width.to_le_bytes());
        sha.update(self.height.to_le_bytes());
        sha.update([self.channels, self.depth]);
        sha.update(&self.data);
        let digest = sha.finalize();
        let mut id = String::with_capacity(7 + 64);
        id.push_str("sha256:");
        for byte in digest.iter() {
            id.push_str(&format!("{byte:02x}"));
        }
        id
    }
    /// Whether `id` names a bitmap by either scheme: a legacy 128-bit FNV hash or
    /// a [`Self::content_id`].
    pub fn is_valid_id(id: &str) -> bool {
        let hex = |s: &str, len: usize| {
            s.len() == len
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        };
        id.strip_prefix("sha256:")
            .map_or_else(|| hex(id, 32), |h| hex(h, 64))
    }
    /// Whether `self` is what `id` names.
    pub fn matches_id(&self, id: &str) -> bool {
        if id.starts_with("sha256:") {
            self.content_id() == id
        } else {
            self.hash() == id
        }
    }
    /// Header plus zlib-compressed samples.
    pub fn compress(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let mut out = Vec::with_capacity(self.data.len() / 4 + 16);
        out.extend(MAGIC);
        out.extend(self.width.to_le_bytes());
        out.extend(self.height.to_le_bytes());
        out.extend([self.channels, self.depth]);
        let mut encoder = ZlibEncoder::new(out, Compression::default());
        encoder.write_all(&self.data)?;
        Ok(encoder.finish()?)
    }
    pub fn decompress(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() >= 14 && &bytes[..4] == MAGIC,
            "Not a stored bitmap"
        );
        let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        let (width, height) = (u32_at(4), u32_at(8));
        let (channels, depth) = (bytes[12], bytes[13]);
        let expected = Self::byte_len(width, height, channels, depth)
            .filter(|len| *len <= MAX_BYTES)
            .context("Stored bitmap too large")?;
        let mut data = Vec::with_capacity(expected);
        ZlibDecoder::new(&bytes[14..])
            .take(expected as u64 + 1)
            .read_to_end(&mut data)?;
        let bitmap = Self {
            width,
            height,
            channels,
            depth,
            data,
        };
        bitmap.validate()?;
        Ok(bitmap)
    }
}
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
/// Standard base64 with padding, as sidecars kept bitmaps; only tests write them now.
#[cfg(any(test, feature = "test-support"))]
pub fn to_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}
pub fn from_base64(text: &str) -> Result<Vec<u8>> {
    let text = text.trim_end_matches('=');
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for c in text.bytes() {
        let v = ALPHABET
            .iter()
            .position(|a| *a == c)
            .context("Invalid base64")? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bitmaps_round_trip_compressed_and_as_base64() {
        let bitmap = Bitmap {
            width: 7,
            height: 5,
            channels: 1,
            depth: 1,
            data: (0..35).map(|i| (i * 7) as u8).collect(),
        };
        let bytes = bitmap.compress().unwrap();
        assert_eq!(Bitmap::decompress(&bytes).unwrap(), bitmap);
        for n in 0..5 {
            let slice = &bytes[..bytes.len() - n];
            assert_eq!(from_base64(&to_base64(slice)).unwrap(), slice);
        }
        assert_eq!(to_base64(b"Man"), "TWFu");
        assert_eq!(to_base64(b"Ma"), "TWE=");
        let mut other = bitmap.clone();
        other.data[3] ^= 1;
        assert_ne!(bitmap.hash(), other.hash());
        assert!(Bitmap::decompress(&bytes[..10]).is_err());
        let wrong = Bitmap {
            data: vec![0; 3],
            ..bitmap
        };
        assert!(wrong.compress().is_err());
    }
}
