//! Disk cache of developed camera images, so reopening a photo, or opening one that
//! was prefetched, skips decoding. An entry holds the image exactly as `raw::Raw::develop`
//! produced it, plus the pixels highlight recovery changed (usually under 1%), so the
//! recovered image is restored without recomputing it.
//!
//! Entries are keyed by the RAW file's identity (size, modification time and a hash of
//! its first 64 KB), the demosaic setting and the running executable's size and
//! modification time, so a rebuilt or updated app never reads another build's output.
//! The directory is capped; the least recently used entries are removed first. Only
//! derived pixels are stored: RAW files, sidecars and catalogs are never touched.
use crate::raw::{CameraImage, Metadata};
use anyhow::{Context, Result, ensure};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

const MAGIC: &[u8; 8] = b"RMKDEC\0\x01";
/// Default size cap of the cache directory.
const LIMIT: u64 = 4 << 30;

/// The per-user cache directory: ~/Library/Caches/RAWmakase on macOS,
/// %LOCALAPPDATA%\RAWmakase\Cache on Windows and $XDG_CACHE_HOME/rawmakase (default
/// ~/.cache/rawmakase) elsewhere. RAWMAKASE_CACHE_DIR overrides all of them.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("RAWMAKASE_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    if cfg!(windows)
        && let Some(local) = std::env::var_os("LOCALAPPDATA")
    {
        return PathBuf::from(local).join("RAWmakase").join("Cache");
    }
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    if cfg!(target_os = "macos") {
        return home.join("Library/Caches/RAWmakase");
    }
    std::env::var_os("XDG_CACHE_HOME")
        .map(|p| PathBuf::from(p).join("rawmakase"))
        .unwrap_or_else(|| home.join(".cache/rawmakase"))
}

pub struct DecodeCache {
    dir: PathBuf,
    limit: u64,
}
#[derive(Serialize, Deserialize, PartialEq)]
struct Header {
    key: String,
    width: u32,
    height: u32,
    fast: bool,
    scale_factor: f32,
    scale_clipped: u32,
    /// Pixels changed by highlight recovery, stored after the image.
    recovered: u64,
}
impl Default for DecodeCache {
    fn default() -> Self {
        Self::new(cache_dir().join("decoded"), LIMIT)
    }
}
impl DecodeCache {
    pub fn new(dir: PathBuf, limit: u64) -> Self {
        Self { dir, limit }
    }
    /// The key of `path` as the current build would develop it at full size.
    pub fn key(path: &Path) -> Result<String> {
        let id = crate::storage::Identity::read(path)?;
        let exe = std::env::current_exe()
            .and_then(fs::metadata)
            .map(|m| {
                let modified = m
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos());
                format!("{}-{modified}", m.len())
            })
            .unwrap_or_default();
        Ok(format!(
            "{}-{}-{}-{:?}-{exe}",
            id.prefix_hash,
            id.size,
            id.modified_ns,
            crate::raw::demosaic()
        ))
    }
    fn file(&self, key: &str) -> PathBuf {
        let hash = key.bytes().fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
        });
        self.dir.join(format!("{hash:016x}.decoded"))
    }
    pub fn contains(&self, key: &str) -> bool {
        self.file(key).is_file()
    }
    /// The cached image for `key`, with its recovered image set; `metadata` comes
    /// from opening the RAW, which is cheap. `None` on a miss or an unusable entry.
    pub fn load(&self, key: &str, metadata: &Metadata) -> Option<CameraImage> {
        let path = self.file(key);
        let image = Self::read(&path, key, metadata).ok()?;
        // Mark as recently used for eviction.
        let _ = fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(SystemTime::now()));
        Some(image)
    }
    fn read(path: &Path, key: &str, metadata: &Metadata) -> Result<CameraImage> {
        let file = fs::File::open(path)?;
        let size = file.metadata()?.len();
        let mut file = std::io::BufReader::new(file);
        let mut magic = [0; 8];
        file.read_exact(&mut magic)?;
        ensure!(&magic == MAGIC, "Not a decode cache entry");
        let mut len = [0; 4];
        file.read_exact(&mut len)?;
        let len = u32::from_le_bytes(len) as usize;
        ensure!(len < 1 << 16, "Invalid decode cache header");
        let mut header = vec![0; len];
        file.read_exact(&mut header)?;
        let header: Header = serde_json::from_slice(&header)?;
        ensure!(header.key == key, "Decode cache key collision");
        let n = header.width as u64 * header.height as u64;
        ensure!(
            n > 0 && n <= 150_000_000 && header.recovered <= n,
            "Invalid decode cache size"
        );
        let start = 12 + len as u64;
        ensure!(
            size == start + n * 12 + header.recovered * 16,
            "Decode cache entry has the wrong size"
        );
        let file = file.into_inner();
        let mut pixels = vec![[0f32; 3]; n as usize];
        read_at(&file, start, bytemuck::cast_slice_mut(&mut pixels))?;
        let mut changes = vec![Change::default(); header.recovered as usize];
        read_at(
            &file,
            start + n * 12,
            bytemuck::cast_slice_mut(&mut changes),
        )?;
        let image = CameraImage {
            recovered: Default::default(),
            width: header.width,
            height: header.height,
            pixels,
            metadata: metadata.clone(),
            fast: header.fast,
            scale_factor: header.scale_factor,
            scale_clipped: header.scale_clipped,
        };
        let mut recovered = CameraImage {
            recovered: Default::default(),
            pixels: vec![[0.; 3]; image.pixels.len()],
            metadata: image.metadata.clone(),
            width: image.width,
            height: image.height,
            fast: image.fast,
            scale_factor: image.scale_factor,
            scale_clipped: image.scale_clipped,
        };
        recovered
            .pixels
            .par_chunks_mut(1 << 20)
            .zip(image.pixels.par_chunks(1 << 20))
            .for_each(|(a, b)| a.copy_from_slice(b));
        for change in changes {
            let p = recovered
                .pixels
                .get_mut(change.index as usize)
                .context("Invalid recovered pixel")?;
            *p = change.value;
        }
        let _ = image.recovered.set(Arc::new(recovered));
        Ok(image)
    }
    /// Stores `image` and its recovered image (which must be set) under `key`, then
    /// trims the directory to the size cap. Written to a temporary file and renamed,
    /// so a concurrent reader never sees a partial entry.
    pub fn store(&self, key: &str, image: &CameraImage) -> Result<()> {
        let recovered = image.recovered.get().context("Image not recovered")?;
        ensure!(
            recovered.pixels.len() == image.pixels.len(),
            "Recovered size mismatch"
        );
        let changes: Vec<Change> = image
            .pixels
            .iter()
            .zip(&recovered.pixels)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, (_, b))| Change {
                index: i as u32,
                value: *b,
            })
            .collect();
        let header = serde_json::to_vec(&Header {
            key: key.into(),
            width: image.width,
            height: image.height,
            fast: image.fast,
            scale_factor: image.scale_factor,
            scale_clipped: image.scale_clipped,
            recovered: changes.len() as u64,
        })?;
        fs::create_dir_all(&self.dir)?;
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        {
            let mut out = std::io::BufWriter::new(file.as_file_mut());
            out.write_all(MAGIC)?;
            out.write_all(&(header.len() as u32).to_le_bytes())?;
            out.write_all(&header)?;
            out.write_all(bytemuck::cast_slice(&image.pixels))?;
            out.write_all(bytemuck::cast_slice(&changes))?;
            out.flush()?;
        }
        file.persist(self.file(key))?;
        self.trim()
    }
    /// Removes the least recently used entries until the directory fits the cap.
    fn trim(&self) -> Result<()> {
        let mut entries: Vec<(SystemTime, u64, PathBuf)> = fs::read_dir(&self.dir)?
            .filter_map(|e| {
                let e = e.ok()?;
                let path = e.path();
                (path.extension()? == "decoded").then_some(())?;
                let m = e.metadata().ok()?;
                Some((m.modified().ok()?, m.len(), path))
            })
            .collect();
        let mut total: u64 = entries.iter().map(|e| e.1).sum();
        entries.sort();
        for (_, size, path) in entries {
            if total <= self.limit {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= size;
            }
        }
        Ok(())
    }
}
/// Fills `buf` from `offset`, in parallel chunks: one reader leaves most of an SSD's
/// bandwidth unused on a 700 MB image.
fn read_at(file: &fs::File, offset: u64, buf: &mut [u8]) -> Result<()> {
    const CHUNK: usize = 16 << 20;
    buf.par_chunks_mut(CHUNK)
        .enumerate()
        .try_for_each(|(i, chunk)| -> std::io::Result<()> {
            let at = offset + (i * CHUNK) as u64;
            #[cfg(unix)]
            {
                std::os::unix::fs::FileExt::read_exact_at(file, chunk, at)
            }
            #[cfg(windows)]
            {
                let mut done = 0;
                while done < chunk.len() {
                    let n = std::os::windows::fs::FileExt::seek_read(
                        file,
                        &mut chunk[done..],
                        at + done as u64,
                    )?;
                    if n == 0 {
                        return Err(std::io::ErrorKind::UnexpectedEof.into());
                    }
                    done += n;
                }
                Ok(())
            }
        })?;
    Ok(())
}
#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Change {
    index: u32,
    value: [f32; 3],
}
// SAFETY: four 4-byte fields, `repr(C)`, no padding; every bit pattern is valid.
unsafe impl bytemuck::Zeroable for Change {}
unsafe impl bytemuck::Pod for Change {}

#[cfg(test)]
mod tests {
    use super::*;
    fn image(width: u32, height: u32, seed: f32) -> CameraImage {
        let mut im = CameraImage {
            recovered: Default::default(),
            width,
            height,
            pixels: (0..width * height)
                .map(|i| [(i as f32 * 0.1 + seed).sin().abs(), 0.5, seed])
                .collect(),
            metadata: Metadata::default(),
            fast: false,
            scale_factor: 1.5,
            scale_clipped: 7,
        };
        let mut recovered = im.clone();
        recovered.pixels[3] = [2., 3., 4.];
        im.recovered = Arc::new(recovered).into();
        im
    }
    #[test]
    fn entries_round_trip_and_stay_within_the_cap() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let bytes = 40 * 30 * 12 + 200;
        let cache = DecodeCache::new(dir.path().join("decoded"), 2 * bytes);
        let metadata = Metadata {
            make: "Test".into(),
            ..Default::default()
        };
        assert!(cache.load("a", &metadata).is_none());
        let a = image(40, 30, 0.2);
        cache.store("a", &a)?;
        assert!(cache.contains("a"));
        let loaded = cache.load("a", &metadata).unwrap();
        assert_eq!(loaded.pixels, a.pixels);
        assert_eq!(loaded.metadata.make, "Test");
        assert_eq!((loaded.scale_factor, loaded.scale_clipped), (1.5, 7));
        assert_eq!(
            loaded.recovered.get().unwrap().pixels,
            a.recovered.get().unwrap().pixels
        );
        // Another key never reads this entry, even on a file-name collision.
        assert!(DecodeCache::read(&cache.file("a"), "b", &metadata).is_err());
        // A truncated entry is a miss.
        let bytes_on_disk = fs::read(cache.file("a"))?;
        fs::write(cache.file("a"), &bytes_on_disk[..bytes_on_disk.len() - 4])?;
        assert!(cache.load("a", &metadata).is_none());
        cache.store("a", &a)?;
        std::thread::sleep(std::time::Duration::from_millis(20));
        cache.store("b", &image(40, 30, 0.4))?;
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(cache.load("a", &metadata).is_some());
        std::thread::sleep(std::time::Duration::from_millis(20));
        // The least recently used entry goes first.
        cache.store("c", &image(40, 30, 0.6))?;
        assert!(cache.contains("a") && cache.contains("c") && !cache.contains("b"));
        Ok(())
    }
}
