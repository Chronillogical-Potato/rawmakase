//! `rawmakase system-info`: the system report for a bug report, printed
//! without a window, catalog, photo or GPU.
use std::process::Command;

#[test]
fn prints_the_system_report_without_a_window_or_catalog() {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rawmakase"))
        .arg("system-info")
        // No display, GPU or catalog to find, and an empty data folder.
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("DISPLAY")
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env("XDG_CACHE_HOME", home.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.starts_with(concat!("RAWmakase: ", env!("CARGO_PKG_VERSION"), "\n")),
        "{text}"
    );
    for label in [
        "Commit",
        "Target",
        "RAW decoder",
        "Installation",
        "OS",
        "Architecture",
        "CPU threads",
    ] {
        assert!(text.contains(&format!("\n{label}: ")), "{label}: {text}");
    }
    for label in [
        "GPU",
        "Display scale",
        "Monitor profile",
        "Catalog",
        "Background task",
    ] {
        assert!(
            text.contains(&format!("\n{label}: unavailable (not in the app window)\n")),
            "{label}: {text}"
        );
    }
    // No catalog, session or data folder was made.
    assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 0);
    assert!(!text.contains(&*home.path().to_string_lossy()));
}
