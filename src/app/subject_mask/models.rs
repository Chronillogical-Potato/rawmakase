//! Installing the selection models: downloaded from where the release pins them (or,
//! in tests, copied from a folder). Nothing starts without the user's choice, and
//! nothing is published until its size and SHA-256 match the manifest.
use super::super::task::Stopping;
use super::super::worker::Event;
use eframe::egui;
use rawmakase_inference::ModelFile;
use rawmakase_inference::manifest::{all_files, total_bytes};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::Sender,
};
use std::time::Duration;

/// How much of the model has been fetched, and whether to stop.
#[derive(Default)]
struct Progress {
    done: AtomicU64,
    cancel: AtomicBool,
}

/// The model on disk and the install in progress, if any.
#[derive(Default)]
pub(crate) struct Installer {
    /// What the folder held when last looked at, and when: the files on disk decide
    /// whether the models are installed (a copy made, or deleted, by hand counts), not
    /// a note of what this process did.
    seen: std::cell::Cell<Option<(std::time::Instant, bool)>>,
    progress: Option<Arc<Progress>>,
    thread: Option<std::thread::JoinHandle<()>>,
    removing: bool,
}

/// Where the model is kept: under this computer's own data folder, in a folder named
/// for the model and its contract version, so a replacement model sits beside it.
pub(super) fn model_dir() -> PathBuf {
    crate::storage::local_data_dir()
        .join("models")
        .join(format!(
            "{}-v{}",
            rawmakase_inference::SUBJECT.id,
            rawmakase_inference::SUBJECT.version
        ))
}
/// Where the app looks for a runtime library it did not come with.
pub(super) fn runtime_dir() -> PathBuf {
    crate::storage::local_data_dir().join("runtime")
}
fn is_installed(dir: &Path) -> bool {
    all_files().iter().all(|f| has(dir, f))
}
fn has(dir: &Path, file: &ModelFile) -> bool {
    std::fs::metadata(dir.join(file.name)).is_ok_and(|m| m.is_file() && m.len() == file.size_bytes)
}

/// The megabytes the drawer quotes.
pub(super) fn download_megabytes() -> u64 {
    total_bytes().div_ceil(1_000_000)
}

impl Installer {
    pub(in crate::app) fn installed(&self) -> bool {
        if self.removing {
            return false;
        }
        // Looked at again after a second, so the drawer does not stat on every frame.
        match self.seen.get() {
            Some((at, installed)) if at.elapsed() < Duration::from_secs(1) => installed,
            _ => {
                let installed = is_installed(&model_dir());
                self.seen.set(Some((std::time::Instant::now(), installed)));
                installed
            }
        }
    }
    fn forget(&self) {
        self.seen.set(None);
    }
    /// The folder holding the model's files.
    pub(in crate::app) fn path(&self) -> Option<PathBuf> {
        self.installed().then(model_dir)
    }
    pub(in crate::app) fn busy(&self) -> bool {
        self.progress.is_some() || self.removing
    }
    /// Bytes fetched so far and the total, while installing.
    pub(in crate::app) fn progress(&self) -> Option<(u64, u64)> {
        self.progress
            .as_ref()
            .map(|p| (p.done.load(Ordering::Relaxed), total_bytes()))
    }
    pub(in crate::app) fn cancel(&mut self) {
        if let Some(p) = &self.progress {
            p.cancel.store(true, Ordering::Relaxed);
        }
    }
    /// Called when the install's event arrives.
    pub(in crate::app) fn finished(&mut self, _ok: bool) {
        self.progress = None;
        self.thread = None;
        self.forget();
    }
    /// Starts fetching the model, or copying `import`; one install at a time.
    pub(in crate::app) fn install(
        &mut self,
        import: Option<PathBuf>,
        tx: Sender<Event>,
        ctx: egui::Context,
    ) {
        if self.busy() || self.installed() {
            return;
        }
        let progress = Arc::new(Progress::default());
        self.progress = Some(progress.clone());
        let spawned = std::thread::Builder::new()
            .name("model-install".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    install(import.as_deref(), &progress)
                }))
                .unwrap_or_else(|_| Err("the install stopped unexpectedly".into()));
                let _ = tx.send(Event::ModelInstalled(result));
                ctx.request_repaint();
            });
        match spawned {
            Ok(handle) => self.thread = Some(handle),
            Err(e) => {
                self.progress = None;
                eprintln!("Could not start the model install: {e}");
            }
        }
    }
    /// Cancels an install and hands its thread to the shared shutdown deadline.
    pub(in crate::app) fn stop(&mut self) -> Vec<Stopping> {
        self.cancel();
        vec![Stopping::new(self.thread.take())]
    }
    /// Removes the model from disk: the worker lets go of it first, off the UI
    /// thread, and the files go once it has. Saved masks keep their pixels.
    pub(in crate::app) fn remove(
        &mut self,
        unload: impl FnOnce() + Send + 'static,
        tx: Sender<Event>,
        ctx: egui::Context,
    ) {
        if self.busy() || !self.installed() {
            return;
        }
        self.removing = true;
        std::thread::spawn(move || {
            unload();
            let result = std::fs::remove_dir_all(model_dir())
                .or_else(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(e)
                    }
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(Event::ModelRemoved(result));
            ctx.request_repaint();
        });
    }
    pub(in crate::app) fn removed(&mut self, _ok: bool) {
        self.removing = false;
        self.forget();
    }
}

/// Fetches or copies each file of the model into a temporary file beside its final
/// place, checks it and publishes it by renaming. `import` is a folder holding the
/// files. A file already in place with the right size was checked when it was
/// published, so a retry fetches only what is missing.
fn install(import: Option<&Path>, progress: &Progress) -> Result<(), String> {
    let dir = model_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    remove_abandoned(&dir);
    // Bytes of files already there count as done.
    let mut finished = 0u64;
    for file in &all_files() {
        if has(&dir, file) {
            finished += file.size_bytes;
            progress.done.store(finished, Ordering::Relaxed);
            continue;
        }
        let part = dir.join(format!(".{}.part-{}", file.name, std::process::id()));
        let result = (|| {
            let mut out = std::fs::File::options()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&part)
                .map_err(|e| format!("could not write {}: {e}", part.display()))?;
            let digest = match import {
                Some(folder) => {
                    let path = folder.join(file.name);
                    let source = std::fs::File::open(&path)
                        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
                    copy_checked(source, &mut out, progress, file, finished)?
                }
                None => download(&mut out, progress, file, finished)?,
            };
            if digest != file.sha256 {
                return Err(match import {
                    Some(_) => format!("{} is not the file this release uses", file.name),
                    None => format!("the download of {} does not match its checksum", file.name),
                });
            }
            out.sync_all().map_err(|e| e.to_string())?;
            drop(out);
            let target = dir.join(file.name);
            // Windows will not rename over a file; only a damaged one can be there.
            let _ = std::fs::remove_file(&target);
            std::fs::rename(&part, &target).map_err(|e| e.to_string())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        result?;
        finished += file.size_bytes;
        progress.done.store(finished, Ordering::Relaxed);
    }
    let _ = crate::storage::sync_dir(&dir);
    Ok(())
}

/// Removes partial files a crashed install left, never one a running instance owns:
/// they are named for the process that made them.
fn remove_abandoned(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let ours = format!(".part-{}", std::process::id());
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(3600));
        if name.starts_with('.') && name.contains(".part-") && !name.ends_with(&ours) && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Copies at most `file`'s size from `source`, hashing as it goes. Stops at
/// cancellation, and at one byte too many. `before` is the bytes of earlier files,
/// for the shared progress.
fn copy_checked(
    mut source: impl Read,
    out: &mut impl Write,
    progress: &Progress,
    file: &ModelFile,
    before: u64,
) -> Result<String, String> {
    let mut sha = Sha256::new();
    let mut buffer = vec![0u8; 256 << 10];
    let mut total = 0u64;
    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let n = source.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > file.size_bytes {
            return Err(format!("{} is larger than expected", file.name));
        }
        sha.update(&buffer[..n]);
        out.write_all(&buffer[..n]).map_err(|e| match e.kind() {
            std::io::ErrorKind::StorageFull => "the disk is full".to_string(),
            _ => e.to_string(),
        })?;
        progress.done.store(before + total, Ordering::Relaxed);
    }
    if total != file.size_bytes {
        return Err(format!("{} is smaller than expected", file.name));
    }
    let digest = sha.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// Fetches `file` from RAWmakase's mirror, else from the repository it was copied
/// from: each a few times on network errors, and a source whose bytes do not match the
/// checksum is not used.
fn download(
    out: &mut std::fs::File,
    progress: &Progress,
    file: &ModelFile,
    before: u64,
) -> Result<String, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .timeout_global(Some(Duration::from_secs(60 * 60)))
        .build()
        .into();
    let mut last = String::new();
    for url in [file.url, file.fallback] {
        match fetch(&agent, url, out, progress, file, before) {
            Ok(digest) if digest == file.sha256 => return Ok(digest),
            Ok(_) => last = format!("the download of {} does not match its checksum", file.name),
            Err(e) if e == "Cancelled" || e.contains("disk is full") => return Err(e),
            Err(e) => last = e,
        }
    }
    Err(format!("could not download the model: {last}"))
}

/// One source, tried up to three times; a client error is final for it.
fn fetch(
    agent: &ureq::Agent,
    url: &str,
    out: &mut std::fs::File,
    progress: &Progress,
    file: &ModelFile,
    before: u64,
) -> Result<String, String> {
    let mut last = String::new();
    for attempt in 0..3u32 {
        if attempt > 0 {
            // Back off, noticing a cancel meanwhile.
            for _ in 0..(10 * attempt) {
                if progress.cancel.load(Ordering::Relaxed) {
                    return Err("Cancelled".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        // Start over: the file is rewritten from the top.
        use std::io::{Seek, SeekFrom};
        out.set_len(0).map_err(|e| e.to_string())?;
        out.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        progress.done.store(before, Ordering::Relaxed);
        match agent.get(url).call() {
            Ok(mut response) => {
                let reader = response.body_mut().as_reader();
                match copy_checked(reader, out, progress, file, before) {
                    Ok(digest) => return Ok(digest),
                    // A stopped or full disk is final; a dropped connection is not.
                    Err(e) if e == "Cancelled" || e.contains("disk is full") => return Err(e),
                    Err(e) => last = e,
                }
            }
            Err(ureq::Error::StatusCode(code)) if (400..500).contains(&code) && code != 429 => {
                return Err(format!("the host answered {code} for {}", file.name));
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: ModelFile = ModelFile {
        name: "x.onnx",
        size_bytes: 3,
        sha256: "",
        url: "",
        fallback: "",
    };

    #[test]
    fn copying_checks_size_and_digest_and_stops_when_asked() {
        let progress = Progress::default();
        // Too short, then too long.
        assert!(copy_checked(&b"ab"[..], &mut Vec::new(), &progress, &FILE, 0).is_err());
        assert!(copy_checked(&b"abcd"[..], &mut Vec::new(), &progress, &FILE, 0).is_err());
        // The right bytes give their SHA-256, and progress counts earlier files too.
        let digest = copy_checked(&b"abc"[..], &mut Vec::new(), &progress, &FILE, 10).unwrap();
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(progress.done.load(Ordering::Relaxed), 13);
        // Cancelled before the first chunk.
        progress.cancel.store(true, Ordering::Relaxed);
        let error = copy_checked(&b"abc"[..], &mut Vec::new(), &progress, &FILE, 0).unwrap_err();
        assert_eq!(error, "Cancelled");
    }

    #[test]
    fn the_install_location_names_the_model_and_its_version() {
        assert!(model_dir().ends_with(format!(
            "models/{}-v{}",
            rawmakase_inference::SUBJECT.id,
            rawmakase_inference::SUBJECT.version
        )));
        assert!(download_megabytes() > 250);
    }

    #[test]
    fn a_model_is_installed_only_when_every_file_is_there_with_its_size() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_installed(dir.path()));
        for file in all_files() {
            let f = std::fs::File::create(dir.path().join(file.name)).unwrap();
            f.set_len(file.size_bytes).unwrap();
        }
        assert!(is_installed(dir.path()));
        std::fs::File::create(dir.path().join(all_files()[1].name))
            .unwrap()
            .set_len(5)
            .unwrap();
        assert!(!is_installed(dir.path()));
    }
}
