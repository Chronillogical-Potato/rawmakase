//! The steps of copying a file to another volume, each on its own so a
//! caller can journal between them: an empty file made where nothing has
//! its name ([`create_file`]), the bytes copied into it and hashed as they
//! are read ([`copy_into`]), made durable ([`sync_file`]), given the
//! source's extras ([`copy_extras`]) and time ([`copy_times`]), and hashed
//! again ([`hash_file`]), its extras too ([`extras`]). The file is never
//! truncated or replaced, and the source never changed. [`remove_file`] is
//! the one unlink here, for a partial copy.
//!
//! A file's or folder's extras are what it keeps beside its contents, times
//! and permissions: its extended attributes on macOS (Finder tags,
//! FinderInfo, its resource fork, and the rest, but for what the system
//! sets itself), its `user.` ones on Linux, and its alternate data streams
//! on Windows.
use super::{FileId, FsError};
use sha2::{Digest as _, Sha256};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::Path;

/// How many bytes [`copy_into`] reads and writes at a time, between which
/// it can be stopped.
pub const COPY_CHUNK: usize = 1 << 20;

/// A file's contents, as SHA-256.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    fn of(sha: Sha256) -> Self {
        Self(sha.finalize().into())
    }
}

/// Hexadecimal, as journals keep it.
impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({self})")
    }
}

impl std::str::FromStr for ContentHash {
    type Err = &'static str;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut out = [0u8; 32];
        if text.len() != 64 || !text.is_ascii() {
            return Err("not a SHA-256");
        }
        for (i, b) in out.iter_mut().enumerate() {
            *b = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).map_err(|_| "not a SHA-256")?;
        }
        Ok(Self(out))
    }
}

/// Why [`copy_into`] did not copy the whole file.
#[derive(Debug)]
pub enum CopyError {
    /// Its caller asked it to stop between chunks.
    Stopped,
    /// The file to copy into isn't the empty file object it was given.
    NotTheFile,
    Fs(FsError),
}

impl From<FsError> for CopyError {
    fn from(e: FsError) -> Self {
        Self::Fs(e)
    }
}

impl From<std::io::Error> for CopyError {
    fn from(e: std::io::Error) -> Self {
        Self::Fs(e.into())
    }
}

impl fmt::Display for CopyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => f.write_str("stopped"),
            Self::NotTheFile => f.write_str("the file being copied into changed"),
            Self::Fs(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CopyError {}

/// Makes an empty file `path`, only if nothing has that name: a file,
/// folder or link already there is never opened, truncated or replaced.
pub fn create_file(path: &Path) -> Result<(), FsError> {
    OpenOptions::new().write(true).create_new(true).open(path)?;
    Ok(())
}

/// Copies every byte of `from` into `to`, which must be empty and, where
/// its volume keeps file ids, the file object `to_id` (made by
/// [`create_file`]): `COPY_CHUNK` bytes at a time,
/// asking `go_on`, with how many bytes are written so far, after each
/// chunk. Returns the SHA-256 of the bytes read from `from`. Neither syncs
/// nor sets times; `to` is never made, truncated or replaced here.
pub fn copy_into(
    from: &Path,
    to: &Path,
    to_id: Option<FileId>,
    go_on: &mut dyn FnMut(u64) -> bool,
) -> Result<ContentHash, CopyError> {
    let mut source = File::open(from)?;
    let mut target = OpenOptions::new().write(true).open(to)?;
    let same = match to_id {
        Some(id) => super::os::handle_id(&target)? == id,
        None => true,
    };
    if !same || target.metadata()?.len() != 0 {
        return Err(CopyError::NotTheFile);
    }
    let mut sha = Sha256::new();
    let mut buffer = vec![0u8; COPY_CHUNK];
    let mut written = 0u64;
    loop {
        let read = match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        sha.update(&buffer[..read]);
        target.write_all(&buffer[..read])?;
        written += read as u64;
        if !go_on(written) {
            return Err(CopyError::Stopped);
        }
    }
    target.flush()?;
    Ok(ContentHash::of(sha))
}

/// Makes file `path`'s contents and times durable.
pub fn sync_file(path: &Path) -> Result<(), FsError> {
    // A file given a read-only source's permissions can't be opened to
    // write any more; Unix syncs through a handle opened to read, and
    // Windows, which needs one opened to write, copies no permissions.
    #[cfg(unix)]
    let file = File::open(path)?;
    #[cfg(not(unix))]
    let file = OpenOptions::new().write(true).open(path)?;
    Ok(file.sync_all()?)
}

/// Gives file or folder `to` the modification and access times of `from`
/// (to the nanosecond where `to`'s volume keeps them; to what it keeps
/// otherwise), and on Unix its permission bits.
pub fn copy_times(from: &Path, to: &Path) -> Result<(), FsError> {
    let source = std::fs::metadata(from)?;
    let file = match source.is_dir() {
        true => super::os::open_folder(to)?,
        false => OpenOptions::new().write(true).open(to)?,
    };
    let times = std::fs::FileTimes::new()
        .set_accessed(source.accessed()?)
        .set_modified(source.modified()?);
    file.set_times(times)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = source.permissions().mode() & 0o777;
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

/// Gives file or folder `to` each extra of `from`, as it is there; what
/// `to` has already stays. [`FsError::Unsupported`] where `to`'s volume
/// can't keep one (FAT and exFAT on Linux and Windows; macOS keeps them
/// there in AppleDouble files, `._name`). Neither syncs nor compares: see
/// [`extras`].
pub fn copy_extras(from: &Path, to: &Path) -> Result<(), FsError> {
    for name in super::os::extra_names(from)? {
        let value = super::os::read_extra(from, &name)?;
        super::os::write_extra(to, &name, &value)?;
    }
    Ok(())
}

/// The SHA-256 of every extra of file or folder `path`, by name: two
/// files or folders whose extras are the same have the same.
pub fn extras(path: &Path) -> Result<ContentHash, FsError> {
    let mut names = super::os::extra_names(path)?;
    names.sort();
    let mut sha = Sha256::new();
    for name in names {
        let value = super::os::read_extra(path, &name)?;
        for part in [name.as_encoded_bytes(), &value] {
            sha.update((part.len() as u64).to_le_bytes());
            sha.update(part);
        }
    }
    Ok(ContentHash::of(sha))
}

/// Gives file or folder `path` the extra `name` (on Linux `user.name`),
/// for tests that need one.
#[cfg(any(test, feature = "test-support"))]
pub fn set_extra(path: &Path, name: &str, value: &[u8]) -> Result<(), FsError> {
    let name = match cfg!(any(target_os = "linux", target_os = "android")) {
        true => format!("user.{name}"),
        false => name.to_string(),
    };
    super::os::write_extra(path, std::ffi::OsStr::new(&name), value)
}

/// The SHA-256 of `path`'s contents, read whole.
pub fn hash_file(path: &Path) -> Result<ContentHash, FsError> {
    hash_prefix(path, u64::MAX)
}

/// The SHA-256 of `path`'s first `len` bytes, or of all it has when it has
/// fewer: what a partial copy of it, `len` bytes long, holds.
pub fn hash_prefix(path: &Path, len: u64) -> Result<ContentHash, FsError> {
    let mut file = File::open(path)?.take(len);
    let mut sha = Sha256::new();
    let mut buffer = vec![0u8; COPY_CHUNK];
    loop {
        match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => sha.update(&buffer[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(ContentHash::of(sha))
}

/// Removes file `path`. Not for a user's file: its caller proves `path` is
/// a partial copy it made, which nothing else has.
pub fn remove_file(path: &Path) -> Result<(), FsError> {
    Ok(std::fs::remove_file(path)?)
}
