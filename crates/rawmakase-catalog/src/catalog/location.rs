//! Where a catalog is, as the app names and reopens it (issue #341).
use std::fmt;
use std::path::{Path, PathBuf};

/// Where a catalog's rows live. A file is the only kind; a catalog on a
/// server would be another variant, and code that only makes sense for a
/// file matches on [`File`](Self::File), so the compiler shows each place
/// it would need deciding.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CatalogLocation {
    /// A `.rawmakase` SQLite file.
    File(PathBuf),
}

impl CatalogLocation {
    /// The catalog's name as shown: a file's name without its extension.
    pub fn name(&self) -> String {
        match self {
            Self::File(path) => path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into(),
        }
    }
}

impl From<PathBuf> for CatalogLocation {
    fn from(path: PathBuf) -> Self {
        Self::File(path)
    }
}

impl From<&Path> for CatalogLocation {
    fn from(path: &Path) -> Self {
        Self::File(path.into())
    }
}

impl From<&PathBuf> for CatalogLocation {
    fn from(path: &PathBuf) -> Self {
        Self::File(path.clone())
    }
}

impl From<&CatalogLocation> for CatalogLocation {
    fn from(location: &CatalogLocation) -> Self {
        location.clone()
    }
}

impl fmt::Display for CatalogLocation {
    /// As shown to people: a file's path.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(path) => path.display().fmt(f),
        }
    }
}
