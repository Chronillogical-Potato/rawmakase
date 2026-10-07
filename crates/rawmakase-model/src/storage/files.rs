use anyhow::Result;
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;

/// Per-user data: ~/Library/Application Support/RAWmakase on macOS,
/// %APPDATA%\RAWmakase on Windows, and $XDG_DATA_HOME/rawmakase (default
/// ~/.local/share/rawmakase) elsewhere. RAWMAKASE_DATA_DIR overrides all of them.
pub use rawmakase_protocol::paths::data_dir;
use rawmakase_protocol::paths::{home, xdg_data_home};

/// This computer's own data folder, never synced: the platform default even
/// when RAWMAKASE_DATA_DIR points elsewhere (a shared or synced folder), and
/// Windows's local rather than roaming AppData.
pub fn local_data_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        home().join("Library/Application Support/RAWmakase")
    } else if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("RAWmakase"))
            .unwrap_or_else(|| home().join(".local/share/rawmakase"))
    } else {
        xdg_data_home().join("rawmakase")
    }
}

/// What persisting does when the destination already exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Replace {
    Overwrite,
    /// Fail instead, even if another writer creates the file meanwhile.
    NoClobber,
}
/// Writes `path` whole or not at all: `write` fills a temporary file beside
/// it, which is synced and renamed over it, and the folder is synced.
pub fn write_atomic(
    path: &Path,
    replace: Replace,
    write: impl FnOnce(&mut NamedTempFile) -> Result<()>,
) -> Result<()> {
    persist(stage(parent_dir(path), write)?, path, replace)
}
/// A synced temporary file in `dir` holding what `write` wrote, for
/// [`persist`] to put in place later.
pub fn stage(
    dir: &Path,
    write: impl FnOnce(&mut NamedTempFile) -> Result<()>,
) -> Result<NamedTempFile> {
    let mut file = NamedTempFile::new_in(dir)?;
    write(&mut file)?;
    file.as_file().sync_all()?;
    Ok(file)
}
/// Renames a file from [`stage`] to `path`, in the same folder, durably.
pub fn persist(staged: NamedTempFile, path: &Path, replace: Replace) -> Result<()> {
    match replace {
        Replace::Overwrite => staged.persist(path),
        Replace::NoClobber => staged.persist_noclobber(path),
    }
    .map_err(|e| e.error)?;
    sync_dir(parent_dir(path))
}
pub fn atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    fs::create_dir_all(parent_dir(path))?;
    write_atomic(path, Replace::Overwrite, |f| {
        serde_json::to_writer_pretty(&mut *f, value)?;
        Ok(f.write_all(b"\n")?)
    })
}
/// The JSON at `path`, or the default when it is missing or unreadable.
pub fn read_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}
/// Makes a rename into `dir` durable. Unix only: Windows cannot open a folder
/// as a file, and NTFS journals the rename itself.
pub fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
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
/// Dot files, including the "._NAME" AppleDouble files macOS writes next to
/// every file on exFAT/FAT drives; they share the photo's extension but are
/// only metadata.
pub fn is_hidden(p: &Path) -> bool {
    p.file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with('.'))
}
pub fn is_raw(p: &Path) -> bool {
    !is_hidden(p)
        && p.extension()
            .and_then(|v| v.to_str())
            .is_some_and(|v| RAW_EXTENSIONS.iter().any(|ext| v.eq_ignore_ascii_case(ext)))
}

/// Search locations for user-installed assets, including the legacy Linux location.
pub fn asset_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![data_dir()];
    let standard = xdg_data_home().join("rawmakase");
    if !dirs.contains(&standard) {
        dirs.push(standard);
    }
    dirs
}

/// A bare filename is relative to the current directory, not an empty directory.
pub fn parent_dir(path: &Path) -> &Path {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
}
