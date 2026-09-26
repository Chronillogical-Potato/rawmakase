//! Bounded, asynchronous disk-cache work and its UI progress.
use crate::{catalog::preview_cache::PreviewCache, storage::Identity};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, SyncSender},
};

pub(super) struct PreviewResult {
    pub path: PathBuf,
    pub image: Option<image::RgbImage>,
    pub cache_error: Option<String>,
}

pub(super) fn spawn(
    cache_path: PathBuf,
    ctx: egui::Context,
) -> (SyncSender<PathBuf>, Receiver<PreviewResult>) {
    let (tx, rx) = mpsc::sync_channel::<PathBuf>(24);
    let (result_tx, result_rx) = mpsc::sync_channel(24);
    std::thread::spawn(move || {
        let (mut cache, open_error) = match PreviewCache::open(&cache_path) {
            Ok(cache) => (Some(cache), None),
            Err(error) => (None, Some(error.to_string())),
        };
        while let Ok(path) = rx.recv() {
            let mut cache_error = open_error.clone();
            let cached = cache.as_ref().and_then(|cache| match cache.load(&path) {
                Ok(image) => image,
                Err(error) => {
                    cache_error = Some(error.to_string());
                    None
                }
            });
            let image = cached.or_else(|| {
                let identity = Identity::read(&path).ok()?;
                let image = super::thumbnail(&path).ok()?;
                if let Some(cache) = &mut cache
                    && let Err(error) = cache.store(&path, &identity, &image)
                {
                    cache_error = Some(error.to_string());
                }
                Some(image)
            });
            if result_tx
                .send(PreviewResult {
                    path,
                    image,
                    cache_error,
                })
                .is_err()
            {
                break;
            }
            ctx.request_repaint();
        }
    });
    (tx, result_rx)
}

#[derive(Default)]
pub(super) struct Progress {
    total: usize,
    completed: usize,
    failed: usize,
    cache_error: Option<String>,
}

impl Progress {
    pub fn queued(&mut self) {
        if self.completed == self.total {
            *self = Self::default();
        }
        self.total += 1;
    }

    pub fn finish(&mut self, result: &PreviewResult) {
        self.completed += 1;
        self.failed += usize::from(result.image.is_none());
        if let Some(error) = &result.cache_error {
            self.cache_error = Some(error.clone());
        }
    }

    /// Worth a status line: still working, or something could not be prepared.
    pub fn active(&self) -> bool {
        self.completed < self.total || self.failed > 0 || self.cache_error.is_some()
    }
    /// One line of small text, the same height as the status row it sits in.
    pub fn show(&self, ui: &mut egui::Ui) {
        if self.total == 0 {
            return;
        }
        if let Some(error) = &self.cache_error {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                egui::RichText::new("Preview cache could not be updated").small(),
            )
            .on_hover_text(error);
        }
        if self.failed > 0 {
            ui.small(format!("{} previews unavailable", self.failed))
                .on_hover_text(
                    "The original may be offline, damaged, or have no usable embedded preview.",
                );
        }
        if self.completed < self.total {
            ui.small(format!("Preparing previews {} / {}", self.completed, self.total))
                .on_hover_text("Cached previews load first; missing ones are built in the background as you browse.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn worker_persists_previews_and_reuses_them_when_original_is_offline() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("photo.png");
        image::RgbImage::new(720, 480).save(&source)?;
        let cache_path = directory.path().join("previews.sqlite3");
        let (tx, rx) = spawn(cache_path.clone(), egui::Context::default());
        tx.try_send(source.clone())?;
        let result = rx.recv_timeout(Duration::from_secs(10))?;
        assert_eq!(result.image.unwrap().dimensions(), (640, 427));
        assert!(result.cache_error.is_none());
        // Reopen through another worker: a memory-only result cannot pass this.
        std::fs::remove_file(&source)?;
        let (tx, rx) = spawn(cache_path, egui::Context::default());
        tx.try_send(source.clone())?;
        let result = rx.recv_timeout(Duration::from_secs(10))?;
        assert_eq!(result.path, source);
        assert!(result.image.is_some());
        assert!(result.cache_error.is_none());
        Ok(())
    }

    #[test]
    fn cache_failure_does_not_stop_previews_or_completion() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let source = directory.path().join("photo.png");
        image::RgbImage::new(16, 16).save(&source)?;
        // A directory cannot be opened as a SQLite database.
        let (tx, rx) = spawn(directory.path().into(), egui::Context::default());
        let mut progress = Progress::default();
        for path in [source, directory.path().join("missing.ARW")] {
            tx.try_send(path)?;
            progress.queued();
        }
        let result = rx.recv_timeout(Duration::from_secs(10))?;
        assert!(result.image.is_some());
        assert!(result.cache_error.is_some());
        progress.finish(&result);
        assert_eq!((progress.completed, progress.total), (1, 2));
        progress.finish(&rx.recv_timeout(Duration::from_secs(10))?);
        assert_eq!(
            (progress.completed, progress.total, progress.failed),
            (2, 2, 1)
        );
        assert!(progress.cache_error.is_some());
        progress.queued();
        assert_eq!(
            (progress.completed, progress.total, progress.failed),
            (0, 1, 0)
        );
        Ok(())
    }
}
