//! What this build calls itself in the files it writes.

/// The software name and version written into exports and their XMP.
pub(crate) const SOFTWARE: &str = concat!("RAWmakase ", env!("CARGO_PKG_VERSION"));
