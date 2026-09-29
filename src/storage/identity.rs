//! What identifies a photo's file: its size, modification time and first bytes.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{fs, fs::File, io::Read, path::Path, time::UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identity {
    pub size: u64,
    pub modified_ns: u128,
    pub prefix_hash: String,
}
impl Identity {
    pub fn read(path: &Path) -> Result<Self> {
        let m = fs::metadata(path)?;
        let mut bytes = Vec::new();
        File::open(path)?.take(65536).read_to_end(&mut bytes)?;
        let hash = bytes.iter().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ u64::from(*b)).wrapping_mul(0x100000001b3)
        });
        Ok(Self {
            size: m.len(),
            modified_ns: m.modified()?.duration_since(UNIX_EPOCH)?.as_nanos(),
            prefix_hash: format!("{hash:016x}"),
        })
    }
    pub(super) fn key(&self) -> String {
        format!("{}-{}-{}", self.prefix_hash, self.size, self.modified_ns)
    }
}
