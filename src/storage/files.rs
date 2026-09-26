use anyhow::Result;
use serde::Serialize;
use std::{
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

/// Per-user data: ~/Library/Application Support/RAWmakase on macOS,
/// %APPDATA%\RAWmakase on Windows, and $XDG_DATA_HOME/rawmakase (default
/// ~/.local/share/rawmakase) elsewhere. RAWMAKASE_DATA_DIR overrides all of them.
pub fn data_dir() -> PathBuf {
    std::env::var_os("RAWMAKASE_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            // Windows has no HOME; without this the data would land in the
            // current directory.
            cfg!(windows)
                .then(|| std::env::var_os("APPDATA"))
                .flatten()
                .map(|p| PathBuf::from(p).join("RAWmakase"))
        })
        .or_else(|| {
            if cfg!(target_os = "macos") || cfg!(windows) {
                None
            } else {
                std::env::var_os("XDG_DATA_HOME").map(|p| PathBuf::from(p).join("rawmakase"))
            }
        })
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(
                if cfg!(target_os = "macos") {
                    "Library/Application Support/RAWmakase"
                } else {
                    ".local/share/rawmakase"
                },
            )
        })
}
pub(crate) fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = parent_dir(path);
    fs::create_dir_all(parent)?;
    let mut f = NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut f, value)?;
    f.write_all(b"\n")?;
    f.as_file().sync_all()?;
    f.persist(path).map_err(|e| e.error)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}
pub fn list_raws(path: &Path) -> Result<Vec<PathBuf>> {
    let dir = if path.is_dir() {
        path
    } else {
        parent_dir(path)
    };
    let mut files = fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| is_raw(p) && p.is_file())
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}
/// Camera RAW extensions LibRaw can decode, lower case.
pub const RAW_EXTENSIONS: [&str; 28] = [
    "3fr", "arw", "cr2", "cr3", "crw", "dcr", "dng", "erf", "fff", "gpr", "iiq", "k25", "kdc",
    "mef", "mos", "mrw", "nef", "nrw", "orf", "pef", "raf", "raw", "rw2", "rwl", "sr2", "srf",
    "srw", "x3f",
];
pub fn is_raw(p: &Path) -> bool {
    p.extension()
        .and_then(|v| v.to_str())
        .is_some_and(|v| RAW_EXTENSIONS.iter().any(|ext| v.eq_ignore_ascii_case(ext)))
}

/// Search locations for user-installed assets, including the legacy Linux location.
pub(crate) fn asset_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![data_dir()];
    let standard = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
                .join(".local/share")
        })
        .join("rawmakase");
    if !dirs.contains(&standard) {
        dirs.push(standard);
    }
    dirs
}

/// A bare filename is relative to the current directory, not an empty directory.
pub(crate) fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}
