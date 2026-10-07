//! Which drive a path lives on, for Lightroom-style volume headers.
use std::path::{Component, Path, PathBuf, Prefix};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Volume {
    /// Display name: the drive's name, a drive letter, or the startup disk.
    pub name: String,
    /// Where the volume is mounted, for external and network drives; `None`
    /// for the startup disk, which is always attached.
    pub mount: Option<PathBuf>,
}

/// The volume holding `path`: /Volumes/NAME on macOS, /media/USER/NAME,
/// /run/media/USER/NAME or /mnt/NAME on Linux, a drive letter or UNC share
/// on Windows, and otherwise the startup disk.
pub(crate) fn volume_of(path: &Path) -> Volume {
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
        name: startup_name(),
        mount: None,
    })
}
/// The startup disk's name: on macOS the /Volumes entry that links to "/"
/// (usually "Macintosh HD").
fn startup_name() -> String {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        if cfg!(target_os = "macos")
            && let Ok(entries) = std::fs::read_dir("/Volumes")
        {
            for entry in entries.flatten() {
                if std::fs::read_link(entry.path()).is_ok_and(|t| t == Path::new("/")) {
                    return entry.file_name().to_string_lossy().into_owned();
                }
            }
        }
        if cfg!(target_os = "macos") {
            "This Mac".into()
        } else {
            "This Computer".into()
        }
    })
    .clone()
}
/// Free and total bytes on the volume holding `path`, from `df` on macOS and
/// Linux; `None` where that isn't available.
pub(crate) fn space(path: &Path) -> Option<(u64, u64)> {
    if cfg!(windows) {
        return None;
    }
    let out = std::process::Command::new("df")
        .arg("-Pk")
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let fields: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
    let total: u64 = fields.get(1)?.parse().ok()?;
    let free: u64 = fields.get(3)?.parse().ok()?;
    Some((free * 1024, total * 1024))
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
