//! Make desktop-mounted network shares accessible to filesystem-based RAW readers.
#[cfg(target_os = "linux")]
pub(crate) fn prepare_filesystem_bridge() {
    use std::{
        path::{Path, PathBuf},
        process::{Command, Stdio},
        time::Duration,
    };
    static START: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let Ok(_guard) = START.lock() else { return };
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return;
    };
    let mount = PathBuf::from(runtime).join("gvfs");
    let mounted = || {
        std::fs::read_to_string("/proc/self/mountinfo").is_ok_and(|s| bridge_is_mounted(&s, &mount))
    };
    if mounted() {
        return;
    }
    let Some(binary) = [
        "/usr/lib/gvfsd-fuse",
        "/usr/libexec/gvfsd-fuse",
        "/usr/lib/gvfs/gvfsd-fuse",
    ]
    .into_iter()
    .find(|p| Path::new(p).is_file()) else {
        return;
    };
    if std::fs::create_dir_all(&mount).is_err() {
        return;
    }
    // GVFS already owns the share/session/authentication. This exposes its existing
    // mounts to native file choosers and LibRaw; no share credentials are handled here.
    let Ok(mut child) = Command::new(binary)
        .arg(&mount)
        .arg("-f")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return;
    };
    for _ in 0..40 {
        if mounted() {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
            return;
        }
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Reap eventual exit while leaving the user-session filesystem bridge running.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}
#[cfg(target_os = "linux")]
fn bridge_is_mounted(mountinfo: &str, path: &std::path::Path) -> bool {
    mountinfo.lines().any(|line| {
        let Some((fields, filesystem)) = line.split_once(" - ") else {
            return false;
        };
        let mount = fields
            .split_whitespace()
            .nth(4)
            .unwrap_or("")
            .replace("\\040", " ")
            .replace("\\134", "\\");
        filesystem.starts_with("fuse.gvfsd-fuse ") && std::path::Path::new(&mount) == path
    })
}
#[cfg(not(target_os = "linux"))]
pub(crate) fn prepare_filesystem_bridge() {}
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    #[test]
    fn requires_an_actual_gvfs_mount_at_the_current_users_mountpoint() {
        let path = std::path::Path::new("/run/user/1000/gvfs");
        assert!(!bridge_is_mounted(
            "1 2 0:0 / /run/user/1000 rw - tmpfs tmpfs rw",
            path
        ));
        assert!(!bridge_is_mounted(
            "1 2 0:0 / /run/user/1001/gvfs rw - fuse.gvfsd-fuse gvfsd-fuse rw",
            path
        ));
        assert!(bridge_is_mounted(
            "1 2 0:0 / /run/user/1000/gvfs rw - fuse.gvfsd-fuse gvfsd-fuse rw",
            path
        ));
    }
}
