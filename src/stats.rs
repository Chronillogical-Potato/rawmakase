//! Opt-in anonymous usage stats: at most one small report a calendar week to
//! stats.rawmakase.com (the service is in `stats/`; the design in issue #32).
//! This builds and sends the report; `app::stats` asks for consent and shows
//! what is sent. Nothing here runs unless the user turned sharing on.
//!
//! A report holds no identifier: version, OS and its major release, CPU
//! architecture, how it was installed, graphics backend and, on Linux, the distro
//! family and display server. Failures are silent: the week's report is
//! simply missing.
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub const ENDPOINT: &str = "https://stats.rawmakase.com/v1/report";
/// The public page with the totals.
pub const PAGE: &str = "https://stats.rawmakase.com";

/// Launch settles before the report goes out.
pub const AT_LAUNCH: Duration = Duration::from_secs(30);

const DISTROS: [&str; 6] = ["arch", "debian", "ubuntu", "fedora", "opensuse", "nixos"];

/// The weekly report, exactly as sent.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Report {
    pub schema: u32,
    pub version: String,
    pub os: &'static str,
    pub arch: &'static str,
    pub channel: &'static str,
    pub os_release: String,
    pub distro: &'static str,
    pub display: &'static str,
    pub gpu: &'static str,
}

impl Report {
    /// This installation's report, or None on a platform the service doesn't
    /// count.
    pub fn collect(gpu: &'static str) -> Option<Self> {
        let os = match std::env::consts::OS {
            "macos" => "macos",
            "windows" => "windows",
            "linux" => "linux",
            _ => return None,
        };
        let arch = match std::env::consts::ARCH {
            "x86_64" => "x86_64",
            "aarch64" => "aarch64",
            _ => return None,
        };
        let linux = os == "linux";
        Some(Self {
            schema: 1,
            // The build's own version, not RAWMAKASE_PRETEND_VERSION.
            version: env!("CARGO_PKG_VERSION").to_string(),
            os,
            arch,
            channel: channel(),
            os_release: os_release()?,
            distro: if linux { distro() } else { "none" },
            display: if linux { display() } else { "none" },
            gpu,
        })
    }

    /// The report as shown before and after sharing.
    pub fn pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }
}

/// The backend the window draws with: `cpu` for a software adapter or none.
pub fn gpu_name(adapter: Option<&wgpu::AdapterInfo>) -> &'static str {
    let Some(adapter) = adapter else { return "cpu" };
    if adapter.device_type == wgpu::DeviceType::Cpu {
        return "cpu";
    }
    match adapter.backend {
        wgpu::Backend::Metal => "metal",
        wgpu::Backend::Vulkan => "vulkan",
        wgpu::Backend::Dx12 => "dx12",
        wgpu::Backend::Gl => "gl",
        _ => "cpu",
    }
}

/// The environment variable that turns sharing off, if one is set:
/// `DO_NOT_TRACK` (consoledonottrack.com) or `RAWMAKASE_NO_TELEMETRY`.
pub fn blocked_by_environment() -> Option<&'static str> {
    ["DO_NOT_TRACK", "RAWMAKASE_NO_TELEMETRY"]
        .into_iter()
        .find(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty() && v != "0"))
}

/// How this copy was installed, as fastframe-update finds it to decide
/// whether it may update itself: the release download it came from, or the
/// package manager that owns it. Inspecting the install can run the package
/// manager, so this runs off the UI thread.
fn channel() -> &'static str {
    use fastframe_update::{Kind, PackageManager, Unsupported};
    match crate::updates::installation() {
        Ok(installation) => match installation.kind {
            Kind::MacBundle => "macos-dmg",
            Kind::WindowsInstaller => "windows-installer",
            Kind::Portable if cfg!(windows) => "windows-zip",
            Kind::Portable => "linux-tarball",
        },
        Err(reason) => match reason {
            // Run from the disk image or a quarantine copy: still the DMG.
            Unsupported::MoveToApplications => "macos-dmg",
            Unsupported::Homebrew => "homebrew",
            Unsupported::SystemPackage(PackageManager::Apt) => "deb",
            Unsupported::SystemPackage(PackageManager::Dnf) => "rpm",
            // pacman can't tell the release package from an AUR build.
            Unsupported::SystemPackage(PackageManager::Pacman) => "arch-package",
            Unsupported::Flatpak => "flatpak",
            Unsupported::Snap => "snap",
            Unsupported::Nix => "nix",
            Unsupported::Cargo => "cargo",
            _ => "unknown",
        },
    }
}

#[cfg(target_os = "macos")]
fn os_release() -> Option<String> {
    let plist = std::fs::read_to_string("/System/Library/CoreServices/SystemVersion.plist").ok()?;
    macos_release(&plist)
}

/// `macos-26` from SystemVersion.plist's ProductVersion.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn macos_release(plist: &str) -> Option<String> {
    let after = &plist[plist.find("<key>ProductVersion</key>")?..];
    let start = after.find("<string>")? + "<string>".len();
    let major: u32 = after[start..]
        .split(['.', '<'])
        .next()?
        .trim()
        .parse()
        .ok()?;
    (11..100).contains(&major).then(|| format!("macos-{major}"))
}

#[cfg(windows)]
fn os_release() -> Option<String> {
    use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let key = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let value = wide("CurrentBuildNumber");
    let mut buffer = [0u16; 32];
    let mut size = std::mem::size_of_val(&buffer) as u32;
    // SAFETY: the key and value names are NUL-terminated and the buffer's
    // size in bytes is passed with it.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if status != 0 {
        return None;
    }
    let build: u32 = String::from_utf16_lossy(&buffer)
        .trim_end_matches('\0')
        .parse()
        .ok()?;
    // Windows 11 is still major version 10; its builds start at 22000.
    Some(
        if build >= 22_000 {
            "windows-11"
        } else {
            "windows-10"
        }
        .into(),
    )
}

#[cfg(target_os = "linux")]
fn os_release() -> Option<String> {
    Some("linux".into())
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
fn os_release() -> Option<String> {
    None
}

fn distro() -> &'static str {
    ["/etc/os-release", "/usr/lib/os-release"]
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .map_or("other", |text| distro_from(&text))
}

/// os-release's `ID`, else the first of `ID_LIKE` the service knows, else
/// `other`: Manjaro reports `arch`, Linux Mint `ubuntu`.
fn distro_from(os_release: &str) -> &'static str {
    let field = |name: &str| {
        os_release.lines().find_map(|line| {
            let value = line.strip_prefix(name)?.strip_prefix('=')?;
            Some(value.trim().trim_matches(['"', '\'']).to_ascii_lowercase())
        })
    };
    let id = field("ID").unwrap_or_default();
    let like = field("ID_LIKE").unwrap_or_default();
    std::iter::once(id.as_str())
        .chain(like.split_whitespace())
        .find_map(|name| {
            // openSUSE's IDs are opensuse-tumbleweed, opensuse-leap and so on.
            let name = if name.starts_with("opensuse") {
                "opensuse"
            } else {
                name
            };
            DISTROS.into_iter().find(|known| *known == name)
        })
        .unwrap_or("other")
}

/// The display server winit opens windows through.
fn display() -> &'static str {
    if crate::platform::wayland() {
        "wayland"
    } else {
        "x11"
    }
}

/// What became of a send attempt.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    /// The service answered, or may have: the week is done, so a report is
    /// never counted twice.
    Done,
    /// No connection was made; worth trying again at the next launch.
    Retry,
}

/// Posts the report. Errors are swallowed: they only decide whether to try
/// again.
pub fn send(report: &Report) -> Outcome {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .http_status_as_error(false)
        .https_only(true)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_global(Some(Duration::from_secs(20)))
        .build()
        .into();
    let body = serde_json::to_string(report).unwrap_or_default();
    match agent
        .post(ENDPOINT)
        .header("User-Agent", format!("RAWmakase/{}", report.version))
        .header("Content-Type", "application/json")
        .send(body)
    {
        Ok(_) => Outcome::Done,
        Err(error) if never_connected(&error) => Outcome::Retry,
        Err(_) => Outcome::Done,
    }
}

/// The request failed before anything reached the service: no route,
/// refused, or the connection or TLS handshake didn't complete.
fn never_connected(error: &ureq::Error) -> bool {
    use std::io::ErrorKind;
    match error {
        ureq::Error::HostNotFound
        | ureq::Error::ConnectionFailed
        | ureq::Error::Timeout(ureq::Timeout::Resolve | ureq::Timeout::Connect)
        | ureq::Error::Tls(_)
        | ureq::Error::Rustls(_) => true,
        ureq::Error::Io(error) => matches!(
            error.kind(),
            ErrorKind::ConnectionRefused
                | ErrorKind::NetworkUnreachable
                | ErrorKind::HostUnreachable
                | ErrorKind::NetworkDown
                | ErrorKind::AddrNotAvailable
                | ErrorKind::NotConnected
                // A firewall or sandbox refusing the socket.
                | ErrorKind::PermissionDenied
        ),
        _ => false,
    }
}

/// `usage-stats.json` in the app data folder: the user's answer and the
/// last week reported. Kept apart from the session, so unrelated session
/// saves by another running copy can't overwrite an answer.
#[derive(Default, Serialize, Deserialize)]
struct State {
    /// Whether the user agreed to share; None until asked.
    #[serde(default)]
    consent: Option<bool>,
    /// The ISO week (UTC) last reported, e.g. "2026-W40".
    #[serde(default)]
    week: Option<String>,
}

fn state_path(dir: &Path) -> std::path::PathBuf {
    dir.join("usage-stats.json")
}

fn read_state(dir: &Path) -> State {
    crate::storage::read_json_or_default(&state_path(dir))
}

/// A held lock file, released explicitly when dropped. Closing the file isn't
/// enough: a process started meanwhile by any thread shares the open file
/// until it execs, and the lock would stay held until then.
struct Held(std::fs::File);

impl Drop for Held {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Changes the state file under a short lock, rereading it first, so the
/// answer and the reported week never overwrite each other.
fn update_state(dir: &Path, change: impl FnOnce(&mut State)) -> anyhow::Result<()> {
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("usage-stats.write.lock"))?;
    lock.lock()?;
    let _held = Held(lock);
    let mut state = read_state(dir);
    change(&mut state);
    crate::storage::atomic_json(&state_path(dir), &state)
}

/// The saved answer, as any running copy last saved it.
pub fn saved_consent(dir: &Path) -> Option<bool> {
    read_state(dir).consent
}

/// Saves the answer. The caller keeps its previous answer if this fails, so
/// an opt-out never silently reverts at the next launch.
pub fn save_consent(dir: &Path, share: bool) -> anyhow::Result<()> {
    update_state(dir, |state| state.consent = Some(share))
}

/// Sends the week's report unless this installation already did. A lock
/// keeps two running copies from both reporting it.
///
/// `week` is this week and `latest` two weeks later. A recorded week from
/// `week` to `latest` counts as reported: if the clock went back, the week
/// already sent can't be sent again. One further ahead came from a clock that
/// was badly wrong and is ignored.
pub fn report_week(
    dir: &Path,
    week: &str,
    latest: &str,
    report: &Report,
    send: impl FnOnce(&Report) -> Outcome,
) -> bool {
    let Ok(lock) = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("usage-stats.lock"))
    else {
        return false;
    };
    if lock.try_lock().is_err() {
        return false;
    }
    let _held = Held(lock);
    let state = read_state(dir);
    // ISO week strings sort in time order.
    if state
        .week
        .as_deref()
        .is_some_and(|last| last >= week && last <= latest)
    {
        return false;
    }
    // The week is recorded before sending: if the app stops mid-send, the
    // report may be lost but is never sent twice. Only a send that never
    // reached the service gives the week back.
    if update_state(dir, |s| s.week = Some(week.to_string())).is_err() {
        return false;
    }
    if send(report) == Outcome::Retry {
        let _ = update_state(dir, |s| s.week = state.week.clone());
        return false;
    }
    true
}

/// The report, collected once on first use. Collecting it can run the
/// package manager (see `channel`), so it happens off the UI thread.
pub type Lazy = Arc<OnceLock<Option<Report>>>;

/// Collects the report in the background, so it is ready to show, then
/// calls `ready` (the app repaints, so the question can appear).
pub fn collect_soon(report: Lazy, gpu: &'static str, ready: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("usage stats".into())
        .spawn(move || {
            report.get_or_init(|| Report::collect(gpu));
            ready();
        });
}

/// Sends this week's report, if it is due, on a short-lived thread that
/// waits `delay` first. Called at launch while sharing is on and when it is
/// turned on; there is no timer. Turning sharing off before it sends cancels
/// it.
pub fn report_soon(
    dir: std::path::PathBuf,
    report: Lazy,
    gpu: &'static str,
    enabled: Arc<AtomicBool>,
    delay: Duration,
) {
    let _ = std::thread::Builder::new()
        .name("usage stats".into())
        .spawn(move || {
            std::thread::sleep(delay);
            if !enabled.load(Ordering::Relaxed) {
                return;
            }
            let Some(report) = report.get_or_init(|| Report::collect(gpu)) else {
                return;
            };
            let seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64);
            let week = crate::time::iso_week(seconds);
            let latest = crate::time::iso_week(seconds + 14 * 86_400);
            report_week(&dir, &week, &latest, report, |report| {
                // Another running copy may have turned sharing off since.
                let saved = saved_consent(&dir) == Some(true);
                if enabled.load(Ordering::Relaxed) && saved {
                    send(report)
                } else {
                    Outcome::Retry
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Report {
        Report {
            schema: 1,
            version: "0.1.10".into(),
            os: "linux",
            arch: "x86_64",
            channel: "arch-package",
            os_release: "linux".into(),
            distro: "arch",
            display: "wayland",
            gpu: "vulkan",
        }
    }

    #[test]
    fn serializes_exactly_the_documented_fields() {
        assert_eq!(
            serde_json::to_string(&report()).unwrap(),
            r#"{"schema":1,"version":"0.1.10","os":"linux","arch":"x86_64","channel":"arch-package","os_release":"linux","distro":"arch","display":"wayland","gpu":"vulkan"}"#
        );
    }

    #[test]
    fn collects_a_report_for_this_platform() {
        let report = Report::collect("vulkan").expect("a supported platform");
        assert_eq!(report.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(report.os, std::env::consts::OS);
        assert_eq!(report.os == "linux", report.distro != "none");
        assert_eq!(report.os == "linux", report.display != "none");
    }

    #[test]
    fn reads_the_macos_major_release() {
        let plist = "<dict>\n\t<key>ProductName</key>\n\t<string>macOS</string>\n\t<key>ProductVersion</key>\n\t<string>26.0.1</string>\n</dict>";
        assert_eq!(macos_release(plist).as_deref(), Some("macos-26"));
        let plist = plist.replace("26.0.1", "15.6");
        assert_eq!(macos_release(&plist).as_deref(), Some("macos-15"));
        assert_eq!(macos_release("<dict></dict>"), None);
    }

    #[test]
    fn maps_os_release_to_a_distro_family() {
        assert_eq!(distro_from("NAME=\"Arch Linux\"\nID=arch\n"), "arch");
        assert_eq!(distro_from("ID=manjaro\nID_LIKE=arch\n"), "arch");
        assert_eq!(
            distro_from("ID=linuxmint\nID_LIKE=\"ubuntu debian\"\n"),
            "ubuntu"
        );
        assert_eq!(
            distro_from("ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n"),
            "opensuse"
        );
        assert_eq!(distro_from("ID=fedora\nVERSION_ID=43\n"), "fedora");
        assert_eq!(distro_from("ID=gentoo\n"), "other");
        assert_eq!(distro_from(""), "other");
    }

    #[test]
    fn names_the_gpu_backend() {
        let adapter = |backend, device_type| wgpu::AdapterInfo::new(device_type, backend);
        assert_eq!(gpu_name(None), "cpu");
        let metal = adapter(wgpu::Backend::Metal, wgpu::DeviceType::IntegratedGpu);
        assert_eq!(gpu_name(Some(&metal)), "metal");
        let llvmpipe = adapter(wgpu::Backend::Vulkan, wgpu::DeviceType::Cpu);
        assert_eq!(gpu_name(Some(&llvmpipe)), "cpu");
    }

    #[test]
    fn reports_each_week_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut sent = 0;
        assert!(report_week(
            dir.path(),
            "2026-W40",
            "2026-W99",
            &report(),
            |_| {
                sent += 1;
                Outcome::Done
            }
        ));
        assert!(!report_week(
            dir.path(),
            "2026-W40",
            "2026-W99",
            &report(),
            |_| {
                sent += 1;
                Outcome::Done
            }
        ));
        assert!(report_week(
            dir.path(),
            "2026-W41",
            "2026-W99",
            &report(),
            |_| {
                sent += 1;
                Outcome::Done
            }
        ));
        assert_eq!(sent, 2);
    }

    #[test]
    fn records_the_week_before_sending() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("usage-stats.json");
        report_week(dir.path(), "2026-W40", "2026-W99", &report(), |_| {
            assert!(
                std::fs::read_to_string(&state)
                    .unwrap()
                    .contains("2026-W40")
            );
            Outcome::Done
        });
        // A send that never connected gives the week back.
        report_week(dir.path(), "2026-W41", "2026-W99", &report(), |_| {
            Outcome::Retry
        });
        assert!(
            std::fs::read_to_string(&state)
                .unwrap()
                .contains("2026-W40")
        );
    }

    #[test]
    fn never_reports_a_week_again_when_the_clock_goes_back() {
        let dir = tempfile::tempdir().unwrap();
        let sent = std::cell::Cell::new(0);
        let send = |_: &Report| {
            sent.set(sent.get() + 1);
            Outcome::Done
        };
        report_week(dir.path(), "2026-W41", "2026-W43", &report(), send);
        // The clock is set back to the previous week: already reported.
        report_week(dir.path(), "2026-W40", "2026-W42", &report(), send);
        assert_eq!(sent.get(), 1);
        // A week recorded far ahead came from a wrong clock and is ignored.
        report_week(dir.path(), "2030-W01", "2030-W03", &report(), send);
        report_week(dir.path(), "2026-W42", "2026-W44", &report(), send);
        assert_eq!(sent.get(), 3);
    }

    #[test]
    fn keeps_the_answer_and_the_week_apart() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(saved_consent(dir.path()), None);
        save_consent(dir.path(), true).unwrap();
        report_week(dir.path(), "2026-W40", "2026-W42", &report(), |_| {
            Outcome::Done
        });
        assert_eq!(saved_consent(dir.path()), Some(true));
        save_consent(dir.path(), false).unwrap();
        assert_eq!(read_state(dir.path()).week.as_deref(), Some("2026-W40"));
    }

    #[test]
    fn retries_only_failures_before_connecting() {
        use std::io::{Error, ErrorKind};
        assert!(never_connected(&ureq::Error::HostNotFound));
        assert!(never_connected(&ureq::Error::Io(Error::from(
            ErrorKind::ConnectionRefused
        ))));
        assert!(never_connected(&ureq::Error::Io(Error::from(
            ErrorKind::NetworkUnreachable
        ))));
        assert!(!never_connected(&ureq::Error::Io(Error::from(
            ErrorKind::ConnectionReset
        ))));
        assert!(!never_connected(&ureq::Error::Timeout(
            ureq::Timeout::Global
        )));
        assert!(!never_connected(&ureq::Error::StatusCode(503)));
    }

    #[test]
    fn retries_only_when_no_connection_was_made() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!report_week(
            dir.path(),
            "2026-W40",
            "2026-W99",
            &report(),
            |_| { Outcome::Retry }
        ));
        let mut sent = 0;
        report_week(dir.path(), "2026-W40", "2026-W99", &report(), |_| {
            sent += 1;
            Outcome::Done
        });
        assert_eq!(sent, 1);
    }

    #[test]
    fn frees_the_lock_while_other_threads_start_processes() {
        // A process started elsewhere shares the lock's open file until it
        // execs, so closing the file alone can leave it locked.
        let done = Arc::new(AtomicBool::new(false));
        let spawner = std::thread::spawn({
            let done = done.clone();
            move || {
                while !done.load(Ordering::Relaxed) {
                    let _ = std::process::Command::new("true").status();
                }
            }
        });
        let dir = tempfile::tempdir().unwrap();
        let skipped = (0..300)
            .filter(|n| {
                let week = format!("2026-W{n:03}");
                !report_week(dir.path(), &week, "2026-W999", &report(), |_| Outcome::Done)
            })
            .count();
        done.store(true, Ordering::Relaxed);
        spawner.join().unwrap();
        assert_eq!(skipped, 0);
    }

    #[test]
    fn skips_the_week_while_another_copy_holds_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let other = std::fs::File::create(dir.path().join("usage-stats.lock")).unwrap();
        other.lock().unwrap();
        assert!(!report_week(
            dir.path(),
            "2026-W40",
            "2026-W99",
            &report(),
            |_| { panic!("must not send while locked") }
        ));
    }
}
