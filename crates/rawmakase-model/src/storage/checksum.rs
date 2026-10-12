//! What a photo's file was, to find it again once it is moved or renamed
//! outside RAWmakase: its size, a quick hash of three blocks for finding
//! candidates cheaply, and the SHA-256 of the whole file, which alone
//! verifies a match.
use super::{IDENTITY_PREFIX, Identity, Stamp};
use anyhow::Result;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// How checksums are computed; one stored with another version is read again.
pub const VERSION: i64 = 1;
/// Each of the quick hash's blocks: the start, middle and end of the file.
const BLOCK: u64 = 64 * 1024;
/// What the full hash reads between two calls of its `pace`.
pub const CHUNK: usize = 1 << 20;

/// A file's checksum, tied to the version of the file that was read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChecksum {
    pub size: u64,
    /// SHA-256 of the size and three 64 KiB blocks, hex.
    pub quick: String,
    /// SHA-256 of the whole file, hex; `None` until it is read.
    pub full: Option<String>,
    /// The version that was read: still current while the file's identity
    /// is the same.
    pub identity: Identity,
}

/// How a read ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    Done(T),
    /// The file changed while it was read; nothing it gave is kept.
    Changed,
    /// `pace` asked to stop.
    Cancelled,
}

impl FileChecksum {
    /// The size and quick hash of `path`, without its full hash. `pace` is
    /// given the bytes read, as [`Self::read_full`]'s is.
    pub fn read_quick(path: &Path, mut pace: impl FnMut(usize) -> bool) -> Result<Outcome<Self>> {
        let identity = Identity::read(path)?;
        let (quick, read) = quick_hash(&mut File::open(path)?, identity.size)?;
        if !pace(read + identity_reads(identity.size)) {
            return Ok(Outcome::Cancelled);
        }
        stable(path, identity, |identity| Self {
            size: identity.size,
            quick,
            full: None,
            identity,
        })
    }
    /// The whole checksum of `path`, read in [`CHUNK`]s. After each chunk,
    /// `pace` is given the bytes just read and returns whether to go on; it
    /// may wait to limit how fast the file is read.
    pub fn read_full(path: &Path, mut pace: impl FnMut(usize) -> bool) -> Result<Outcome<Self>> {
        let identity = Identity::read(path)?;
        let mut file = File::open(path)?;
        let (quick, read) = quick_hash(&mut file, identity.size)?;
        if !pace(read + identity_reads(identity.size)) {
            return Ok(Outcome::Cancelled);
        }
        file.seek(SeekFrom::Start(0))?;
        let mut sha = Sha256::new();
        let mut buffer = vec![0; CHUNK];
        loop {
            let n = read_chunk(&mut file, &mut buffer)?;
            if n == 0 {
                break;
            }
            sha.update(&buffer[..n]);
            if !pace(n) {
                return Ok(Outcome::Cancelled);
            }
        }
        let full = hex(&sha.finalize());
        stable(path, identity, |identity| Self {
            size: identity.size,
            quick,
            full: Some(full),
            identity,
        })
    }
    /// Whether this was read from the version of the file `identity` names.
    pub fn is_current(&self, identity: &Identity) -> bool {
        self.identity == *identity
    }
}

/// `make(identity)` if the file at `path` is still the version `identity`
/// names: the same size, modification time and first bytes.
fn stable<T>(
    path: &Path,
    identity: Identity,
    make: impl FnOnce(Identity) -> T,
) -> Result<Outcome<T>> {
    let after = Stamp::read(path)?;
    if after.size != identity.size || after.modified_ns != identity.modified_ns {
        return Ok(Outcome::Changed);
    }
    if Identity::read(path)? != identity {
        return Ok(Outcome::Changed);
    }
    Ok(Outcome::Done(make(identity)))
}

/// What reading a file's identity before and after (see [`stable`]) reads.
fn identity_reads(size: u64) -> usize {
    2 * IDENTITY_PREFIX.min(size) as usize
}

/// Fills `buffer` as far as the file goes; less only at its end.
fn read_chunk(file: &mut File, buffer: &mut [u8]) -> Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match file.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(filled)
}

/// SHA-256 of the size and of the blocks at the start, middle and end of a
/// file of `size` bytes, and the bytes read. They overlap in a file smaller
/// than three blocks; one no larger than a block is its only block.
fn quick_hash(file: &mut File, size: u64) -> Result<(String, usize)> {
    let mut sha = Sha256::new();
    sha.update(size.to_le_bytes());
    let block = BLOCK.min(size);
    let mut buffer = vec![0; block as usize];
    let mut starts = vec![0, size / 2 - (size / 2).min(block / 2), size - block];
    starts.dedup();
    for &start in &starts {
        file.seek(SeekFrom::Start(start))?;
        anyhow::ensure!(
            read_chunk(file, &mut buffer)? == buffer.len(),
            "The file is shorter than it was"
        );
        sha.update(&buffer);
    }
    Ok((hex(&sha.finalize()), buffer.len() * starts.len()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.dng");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }
    fn done<T>(read: Outcome<T>) -> T {
        match read {
            Outcome::Done(value) => value,
            _ => panic!("not read"),
        }
    }

    #[test]
    fn the_full_hash_is_the_files_sha256_and_the_quick_hash_agrees() {
        let bytes: Vec<u8> = (0..3 * CHUNK + 17).map(|i| (i * 31 % 251) as u8).collect();
        let (_dir, path) = file(&bytes);
        let full = done(FileChecksum::read_full(&path, |_| true).unwrap());
        assert_eq!(
            full.full.as_deref(),
            Some(hex(&Sha256::digest(&bytes)).as_str())
        );
        assert_eq!(full.size, bytes.len() as u64);
        let quick = done(FileChecksum::read_quick(&path, |_| true).unwrap());
        assert_eq!((&quick.quick, quick.full), (&full.quick, None));
        assert!(full.is_current(&Identity::read(&path).unwrap()));
    }

    #[test]
    fn a_change_outside_the_sampled_blocks_changes_only_the_full_hash() {
        let mut bytes = vec![7u8; 4 * BLOCK as usize];
        let (_dir, a) = file(&bytes);
        // Between the start and middle blocks, and the same size.
        bytes[BLOCK as usize + 10] = 8;
        let (_dir2, b) = file(&bytes);
        let (a, b) = (
            done(FileChecksum::read_full(&a, |_| true).unwrap()),
            done(FileChecksum::read_full(&b, |_| true).unwrap()),
        );
        assert_eq!(a.quick, b.quick);
        assert_ne!(a.full, b.full);
    }

    #[test]
    fn quick_hashes_match_reference_values() {
        // Worked out apart from this code (Python's hashlib): SHA-256 of the
        // size as 8 bytes little-endian, then the blocks.
        let (_dir, small) = file(b"tiny synthetic raw");
        assert_eq!(
            done(FileChecksum::read_quick(&small, |_| true).unwrap()).quick,
            "9beee9b9ee4616893c8914866ad8fff1fcce147ef391f86deec352f36bab2deb"
        );
        let bytes: Vec<u8> = (0..300_000).map(|i| (i * 31 % 251) as u8).collect();
        let (_dir, large) = file(&bytes);
        assert_eq!(
            done(FileChecksum::read_quick(&large, |_| true).unwrap()).quick,
            "ff49b88ab02bd87d11a736b5d2c6b4869523ac70622fe32caeeaa833c77b5031"
        );
    }

    #[test]
    fn the_quick_hash_reports_what_it_read() {
        let (_dir, small) = file(b"tiny synthetic raw");
        let mut read = 0;
        done(
            FileChecksum::read_quick(&small, |n| {
                read += n;
                true
            })
            .unwrap(),
        );
        // The quick block, and the identity read before and after.
        assert_eq!(read, 3 * 18);
        let (_dir, large) = file(&vec![0u8; 300_000]);
        let mut read = 0;
        done(
            FileChecksum::read_quick(&large, |n| {
                read += n;
                true
            })
            .unwrap(),
        );
        assert_eq!(read, 5 * BLOCK as usize);
    }

    #[test]
    fn small_and_empty_files_have_checksums() {
        for bytes in [&b""[..], b"x", b"tiny synthetic raw"] {
            let (_dir, path) = file(bytes);
            let full = done(FileChecksum::read_full(&path, |_| true).unwrap());
            assert_eq!(full.full.unwrap(), hex(&Sha256::digest(bytes)));
        }
    }

    #[test]
    fn pace_sees_every_chunk_and_can_stop_the_read() {
        let (_dir, path) = file(&vec![1u8; 2 * CHUNK + 5]);
        let mut seen = Vec::new();
        done(
            FileChecksum::read_full(&path, |n| {
                seen.push(n);
                true
            })
            .unwrap(),
        );
        assert_eq!(seen, [5 * BLOCK as usize, CHUNK, CHUNK, 5]);
        let mut chunks = 0;
        let read = FileChecksum::read_full(&path, |_| {
            chunks += 1;
            false
        })
        .unwrap();
        assert_eq!((read, chunks), (Outcome::Cancelled, 1));
        assert!(matches!(
            FileChecksum::read_quick(&path, |_| false).unwrap(),
            Outcome::Cancelled
        ));
    }

    #[test]
    fn a_file_changed_during_the_read_gives_nothing() {
        let (_dir, path) = file(&vec![1u8; 2 * CHUNK]);
        let mut first = true;
        let read = FileChecksum::read_full(&path, |_| {
            // After the quick hash: same size, other content and time.
            if std::mem::take(&mut first) {
                return true;
            }
            std::fs::write(&path, vec![2u8; 2 * CHUNK]).unwrap();
            let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
            File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(later)
                .unwrap();
            true
        })
        .unwrap();
        assert_eq!(read, Outcome::Changed);
    }
}
