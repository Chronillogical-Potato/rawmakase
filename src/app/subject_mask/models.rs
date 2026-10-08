//! Installing the selection model: downloaded from where the release pins it, or
//! imported from a file the user already has. Nothing starts without the user's
//! choice, and nothing is published until its size and SHA-256 match the manifest.
use super::super::task::Stopping;
use super::super::worker::Event;
use eframe::egui;
use rawmakase_inference::SUBJECT;
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
pub(crate) struct Installer {
    installed: bool,
    progress: Option<Arc<Progress>>,
    thread: Option<std::thread::JoinHandle<()>>,
    removing: bool,
}
impl Default for Installer {
    fn default() -> Self {
        Self {
            installed: is_installed(&model_path()),
            progress: None,
            thread: None,
            removing: false,
        }
    }
}

/// Where the model is kept: under this computer's own data folder, in a folder named
/// for the model and its contract version, so a replacement model sits beside it.
fn model_dir() -> PathBuf {
    crate::storage::local_data_dir()
        .join("models")
        .join(format!("{}-v{}", SUBJECT.id, SUBJECT.version))
}
fn model_path() -> PathBuf {
    model_dir().join(SUBJECT.file_name)
}
/// Where the app looks for a runtime library it did not come with.
pub(in crate::app) fn runtime_dir() -> PathBuf {
    crate::storage::local_data_dir().join("runtime")
}
fn is_installed(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == SUBJECT.size_bytes)
}

/// The megabytes the drawer quotes.
pub(in crate::app) fn download_megabytes() -> u64 {
    SUBJECT.size_bytes.div_ceil(1_000_000)
}

impl Installer {
    pub(in crate::app) fn installed(&self) -> bool {
        self.installed && !self.removing
    }
    pub(in crate::app) fn path(&self) -> Option<PathBuf> {
        self.installed().then(model_path)
    }
    pub(in crate::app) fn busy(&self) -> bool {
        self.progress.is_some() || self.removing
    }
    /// Bytes fetched so far and the total, while installing.
    pub(in crate::app) fn progress(&self) -> Option<(u64, u64)> {
        self.progress
            .as_ref()
            .map(|p| (p.done.load(Ordering::Relaxed), SUBJECT.size_bytes))
    }
    pub(in crate::app) fn cancel(&mut self) {
        if let Some(p) = &self.progress {
            p.cancel.store(true, Ordering::Relaxed);
        }
    }
    /// Called when the install's event arrives.
    pub(in crate::app) fn finished(&mut self, ok: bool) {
        self.progress = None;
        self.thread = None;
        if ok {
            self.installed = is_installed(&model_path());
        }
    }
    /// Starts fetching the model, or copying `import`; one install at a time.
    pub(in crate::app) fn install(
        &mut self,
        import: Option<PathBuf>,
        tx: Sender<Event>,
        ctx: egui::Context,
    ) {
        if self.busy() || self.installed {
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
        if self.busy() || !self.installed {
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
    pub(in crate::app) fn removed(&mut self, ok: bool) {
        self.removing = false;
        if ok {
            self.installed = false;
        }
    }
}

/// Fetches or copies the model into a temporary file beside its final place, checks
/// it and publishes it by renaming.
fn install(import: Option<&Path>, progress: &Progress) -> Result<(), String> {
    let dir = model_dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    remove_abandoned(&dir);
    let part = dir.join(format!(
        ".{}.part-{}",
        SUBJECT.file_name,
        std::process::id()
    ));
    let result = (|| {
        let mut file = std::fs::File::options()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&part)
            .map_err(|e| format!("could not write {}: {e}", part.display()))?;
        let digest = match import {
            Some(path) => {
                let source = std::fs::File::open(path)
                    .map_err(|e| format!("could not read {}: {e}", path.display()))?;
                copy_checked(source, &mut file, progress)?
            }
            None => download(&mut file, progress)?,
        };
        if digest != SUBJECT.sha256 {
            return Err(match import {
                Some(_) => "that file is not the selection model this release uses".to_string(),
                None => "the download does not match its checksum".to_string(),
            });
        }
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        let target = model_path();
        // Windows will not rename over a file; a model that failed its size check
        // is the only thing that can be there.
        let _ = std::fs::remove_file(&target);
        std::fs::rename(&part, &target).map_err(|e| e.to_string())?;
        let _ = crate::storage::sync_dir(&dir);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
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

/// Copies at most the model's size from `source`, hashing as it goes. Stops at
/// cancellation, and at one byte too many.
fn copy_checked(
    mut source: impl Read,
    out: &mut impl Write,
    progress: &Progress,
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
        if total > SUBJECT.size_bytes {
            return Err("the file is larger than the selection model".into());
        }
        sha.update(&buffer[..n]);
        out.write_all(&buffer[..n]).map_err(|e| match e.kind() {
            std::io::ErrorKind::StorageFull => "the disk is full".to_string(),
            _ => e.to_string(),
        })?;
        progress.done.store(total, Ordering::Relaxed);
    }
    if total != SUBJECT.size_bytes {
        return Err("the file is smaller than the selection model".into());
    }
    let digest = sha.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

/// Fetches the model from the host its manifest pins, trying again on network
/// errors a few times.
fn download(out: &mut std::fs::File, progress: &Progress) -> Result<String, String> {
    let url = source_url();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .timeout_global(Some(Duration::from_secs(60 * 60)))
        .build()
        .into();
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
        progress.done.store(0, Ordering::Relaxed);
        match agent.get(&url).call() {
            Ok(mut response) => {
                let reader = response.body_mut().as_reader();
                match copy_checked(reader, out, progress) {
                    Ok(digest) => return Ok(digest),
                    // A stopped or full disk is final; a dropped connection is not.
                    Err(e) if e == "Cancelled" || e.contains("disk is full") => return Err(e),
                    Err(e) => last = e,
                }
            }
            Err(ureq::Error::StatusCode(code)) if (400..500).contains(&code) && code != 429 => {
                return Err(format!("the host answered {code}"));
            }
            Err(e) => last = e.to_string(),
        }
    }
    Err(format!("could not download the model: {last}"))
}

/// The pinned Hugging Face file once the maintainer has published it; until then
/// the upstream release the manifest names, which the checksum holds to the same
/// bytes.
fn source_url() -> String {
    if rawmakase_inference::ModelSpec::is_published() {
        SUBJECT.pinned_url()
    } else {
        SUBJECT.upstream_url.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copying_checks_size_and_digest_and_stops_when_asked() {
        let progress = Progress::default();
        // Too short.
        let mut sink = Vec::new();
        assert!(copy_checked(&b"abc"[..], &mut sink, &progress).is_err());
        // Cancelled before the first chunk.
        progress.cancel.store(true, Ordering::Relaxed);
        let error = copy_checked(&b"abc"[..], &mut Vec::new(), &progress).unwrap_err();
        assert_eq!(error, "Cancelled");
    }

    #[test]
    fn the_install_location_names_the_model_and_its_version() {
        let path = model_path();
        assert!(path.ends_with(format!(
            "models/{}-v{}/{}",
            SUBJECT.id, SUBJECT.version, SUBJECT.file_name
        )));
        assert!(download_megabytes() > 100);
    }
}
