//! What this build calls itself in the files it writes.

/// The software name and version written into exports and their XMP: the app's
/// version, which this crate's build script reads from the workspace's root manifest.
pub const SOFTWARE: &str = concat!("RAWmakase ", env!("RAWMAKASE_VERSION"));
