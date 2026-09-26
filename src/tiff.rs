//! Minimal bounded TIFF directory reading for RAW containers (ARW, DNG, the TIFF
//! inside RAF). Reads only the directories and tags it is asked for.
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
};

pub(crate) struct Entry {
    pub kind: u16,
    pub count: u32,
    pub value: [u8; 4],
}
/// Minimal bounded TIFF directory reader. Offsets are relative to `base`.
pub(crate) struct Tiff {
    f: File,
    base: u64,
    pub little: bool,
    pub first: u64,
}
impl Tiff {
    pub(crate) fn open(mut f: File, base: u64) -> Option<Self> {
        let mut h = [0u8; 8];
        f.seek(SeekFrom::Start(base)).ok()?;
        f.read_exact(&mut h).ok()?;
        let little = match &h[..4] {
            b"II*\0" => true,
            b"MM\0*" => false,
            _ => return None,
        };
        let mut t = Self {
            f,
            base,
            little,
            first: 0,
        };
        t.first = t.u32(&h[4..8]) as u64;
        Some(t)
    }
    pub(crate) fn u16(&self, b: &[u8]) -> u16 {
        let a = [b[0], b[1]];
        if self.little {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        }
    }
    pub(crate) fn u32(&self, b: &[u8]) -> u32 {
        let a = [b[0], b[1], b[2], b[3]];
        if self.little {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        }
    }
    pub(crate) fn bytes(&mut self, offset: u64, len: usize) -> Option<Vec<u8>> {
        if len > 1 << 20 {
            return None;
        }
        let mut b = vec![0; len];
        self.f
            .seek(SeekFrom::Start(self.base.checked_add(offset)?))
            .ok()?;
        self.f.read_exact(&mut b).ok()?;
        Some(b)
    }
    pub(crate) fn ifd(&mut self, offset: u64) -> Option<BTreeMap<u16, Entry>> {
        let count = self.bytes(offset, 2)?;
        let n = self.u16(&count) as usize;
        if n == 0 || n > 1000 {
            return None;
        }
        let b = self.bytes(offset + 2, n * 12)?;
        Some(
            b.as_chunks::<12>()
                .0
                .iter()
                .map(|e| {
                    (
                        self.u16(&e[0..2]),
                        Entry {
                            kind: self.u16(&e[2..4]),
                            count: self.u32(&e[4..8]),
                            value: [e[8], e[9], e[10], e[11]],
                        },
                    )
                })
                .collect(),
        )
    }
    /// The first value of a LONG or IFD entry, used as a directory offset.
    pub(crate) fn offset(&self, e: &Entry) -> Option<u64> {
        matches!(e.kind, 4 | 13).then(|| self.u32(&e.value) as u64)
    }
    pub(crate) fn numbers(&mut self, e: &Entry) -> Option<Vec<f32>> {
        let size = match e.kind {
            3 | 8 => 2,
            4 | 9 => 4,
            5 | 10 => 8,
            _ => return None,
        };
        let len = (e.count as usize).checked_mul(size)?;
        let b = if len <= 4 {
            e.value[..len].to_vec()
        } else {
            let at = self.u32(&e.value) as u64;
            self.bytes(at, len)?
        };
        let v: Vec<f32> = b
            .chunks_exact(size)
            .map(|c| match e.kind {
                3 => self.u16(c) as f32,
                8 => self.u16(c) as i16 as f32,
                4 => self.u32(c) as f32,
                9 => self.u32(c) as i32 as f32,
                5 => self.u32(&c[..4]) as f32 / self.u32(&c[4..]) as f32,
                _ => self.u32(&c[..4]) as i32 as f32 / self.u32(&c[4..]) as i32 as f32,
            })
            .collect();
        v.iter().all(|x| x.is_finite()).then_some(v)
    }
}

impl Tiff {
    /// An entry's raw value bytes in file byte order.
    pub(crate) fn raw(&mut self, e: &Entry) -> Option<Vec<u8>> {
        let size = match e.kind {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        };
        let len = (e.count as usize).checked_mul(size)?;
        if len <= 4 {
            Some(e.value[..len].to_vec())
        } else {
            let at = self.u32(&e.value) as u64;
            self.bytes(at, len)
        }
    }
    /// All values of a LONG or IFD entry, e.g. the SubIFDs array.
    pub(crate) fn offsets(&mut self, e: &Entry) -> Option<Vec<u64>> {
        if !matches!(e.kind, 4 | 13) {
            return None;
        }
        let b = self.raw(e)?;
        Some(b.chunks_exact(4).map(|c| self.u32(c) as u64).collect())
    }
}
