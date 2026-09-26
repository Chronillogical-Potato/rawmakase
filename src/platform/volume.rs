//! Which drive a path lives on, for Lightroom-style volume headers.
use std::path::{Component, Path, PathBuf, Prefix};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Volume {
    /// Display name: the drive's name, a drive letter, or the startup disk.
    pub name: String,
    /// Where the volume is mounted, for external and network drives; `None`
    /// for the startup disk, which is always attached.
    pub mount: Option<PathBuf>,
}

/// The volume holding `path`: /Volumes/NAME on macOS, /media/USER/NAME,
/// /run/media/USER/NAME or /mnt/NAME on Linux, a drive letter or UNC share
/// on Windows, and otherwise the startup disk.
pub fn volume_of(path: &Path) -> Volume {
    let parts: Vec<Component> = path.components().collect();
    if let Some(Component::Prefix(prefix)) = parts.first() {
        let mount: PathBuf = parts.iter().take(2).collect();
        let name = match prefix.kind() {
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                format!("{}\\{}", server.to_string_lossy(), share.to_string_lossy())
            }
            _ => prefix
                .as_os_str()
                .to_string_lossy()
                .trim_end_matches('\\')
                .to_string(),
        };
        return Volume {
            name,
            mount: Some(mount),
        };
    }
    let names: Vec<String> = parts
        .iter()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let under = |depth: usize| -> Option<Volume> {
        let name = names.get(depth)?.clone();
        let mount = std::iter::once(PathBuf::from("/"))
            .chain(names.iter().take(depth + 1).map(PathBuf::from))
            .collect();
        Some(Volume {
            name,
            mount: Some(mount),
        })
    };
    let first = names.first().map(String::as_str);
    let external = match first {
        Some("Volumes") if cfg!(target_os = "macos") => under(1),
        Some("media") => under(2).or_else(|| under(1)),
        Some("run") if names.get(1).is_some_and(|n| n == "media") => under(3),
        Some("mnt") => under(1),
        _ => None,
    };
    external.unwrap_or_else(|| Volume {
        name: if cfg!(target_os = "macos") {
            "This Mac".into()
        } else {
            "This Computer".into()
        },
        mount: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_external_and_local_volumes() {
        let usb = volume_of(Path::new("/Volumes/Samsung USB/05 Pacific Highway"));
        let local = volume_of(Path::new("/Users/me/Pictures"));
        let linux = volume_of(Path::new("/run/media/me/CARD/DCIM"));
        if cfg!(target_os = "macos") {
            assert_eq!(usb.name, "Samsung USB");
            assert_eq!(usb.mount, Some(PathBuf::from("/Volumes/Samsung USB")));
        }
        assert_eq!(local.mount, None);
        assert_eq!(linux.name, "CARD");
        assert_eq!(linux.mount, Some(PathBuf::from("/run/media/me/CARD")));
    }
}
