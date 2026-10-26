//! Reading what export file names need from the photos' files, in the
//! background: every photo's camera make and model (the catalog keeps a
//! combined camera label, which is not Lightroom's Make or Model), and for a
//! photo opened without a catalog also its capture time and photo info.
//!
//! A read belongs to the Export scope it was started for. It is started only
//! when the template needs what it reads, and cancelled when the scope is
//! replaced or the dialog closes; a completion of any other read is dropped.
//! Failure, a panic or a lost worker end it as Failed, never busy. The worker
//! only reads files, so at exit it is left to end with the process: nothing
//! waits for it.
use super::super::worker::Event;
use crate::export_settings::{CameraNames, NameFields, Needs};
use crate::metadata::PhotoInfo;
use eframe::egui;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::Sender,
};

/// What one photo's file gave.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FileFields {
    pub camera: CameraNames,
    /// For a photo without a catalog: its capture time and photo info.
    pub captured: Option<String>,
    pub info: Option<PhotoInfo>,
}

/// One photo to read: its file, and whether it has no catalog (so its
/// capture time and photo info come from the file too).
#[derive(Clone, Debug)]
pub(super) struct ToRead {
    pub path: PathBuf,
    pub standalone: bool,
}

/// Reads one photo's file: what it gave, or why it can't be read (a file
/// with no names gives none; one that can't be opened is an error).
pub(super) type Reader = Arc<dyn Fn(&ToRead) -> Result<FileFields, String> + Send + Sync>;

/// Each photo's read, in order: what its file gave, or why it couldn't be
/// read.
pub(crate) type Reads = Vec<Result<FileFields, String>>;

/// Where a read stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum Status {
    /// None started for the scope.
    #[default]
    Idle,
    /// `done` of `total` photos read.
    Reading { done: usize, total: usize },
    /// Read, and in the scope's fields.
    Ready,
    /// Not all could be read: why. What was read is in the scope's fields.
    Failed(String),
}

enum State {
    Idle,
    Reading {
        request: u64,
        cancel: Arc<AtomicBool>,
        done: Arc<AtomicUsize>,
        total: usize,
    },
    Ready,
    Failed(String),
}

/// The scope's read.
pub(super) struct NameRead {
    state: State,
    /// The last request's id; each read has its own.
    request: u64,
    reader: Reader,
}
impl Default for NameRead {
    fn default() -> Self {
        Self {
            state: State::Idle,
            request: 0,
            reader: Arc::new(read_file),
        }
    }
}

impl NameRead {
    pub fn status(&self) -> Status {
        match &self.state {
            State::Idle => Status::Idle,
            State::Reading { done, total, .. } => Status::Reading {
                done: done.load(Ordering::Relaxed).min(*total),
                total: *total,
            },
            State::Ready => Status::Ready,
            State::Failed(why) => Status::Failed(why.clone()),
        }
    }
    #[cfg(test)]
    pub fn set_reader(&mut self, reader: Reader) {
        self.reader = reader;
    }

    /// Reads `photos` on a worker, which sends [`Event::NamesRead`] with this
    /// read's request id; any read under way is cancelled.
    pub fn start(&mut self, photos: Vec<ToRead>, events: Sender<Event>, ctx: egui::Context) {
        self.cancel();
        self.request += 1;
        let request = self.request;
        let cancel = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicUsize::new(0));
        self.state = State::Reading {
            request,
            cancel: cancel.clone(),
            done: done.clone(),
            total: photos.len(),
        };
        let reader = self.reader.clone();
        let repaint = ctx.clone();
        super::super::task::spawn(
            events,
            ctx,
            move |events| {
                let mut read = Vec::with_capacity(photos.len());
                for photo in &photos {
                    // Nobody waits for it any more.
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    read.push(reader(photo));
                    done.fetch_add(1, Ordering::Relaxed);
                    repaint.request_repaint();
                }
                let _ = events.send(Event::NamesRead {
                    request,
                    read: Ok(read),
                });
            },
            move |events, message| {
                let _ = events.send(Event::NamesRead {
                    request,
                    read: Err(message),
                });
            },
        );
    }

    /// Stops the read under way, or forgets the one done: the scope is
    /// replaced, or the dialog closed.
    pub fn cancel(&mut self) {
        if let State::Reading { cancel, .. } = &self.state {
            cancel.store(true, Ordering::Relaxed);
        }
        self.state = State::Idle;
    }

    /// A read finished: for the read under way, each photo's fields (`None`
    /// for one that couldn't be read) and why some couldn't, after which the
    /// read is Ready or Failed; `None` for any other read (cancelled,
    /// replaced).
    pub fn finished(
        &mut self,
        request: u64,
        read: Result<Reads, String>,
    ) -> Option<(Vec<Option<FileFields>>, Option<String>)> {
        let total = match &self.state {
            State::Reading {
                request: current,
                total,
                ..
            } if *current == request => *total,
            _ => return None,
        };
        let (fields, failed) = match read {
            Ok(reads) if reads.len() == total => {
                let failures: Vec<String> = reads
                    .iter()
                    .filter_map(|r| r.as_ref().err().cloned())
                    .collect();
                let failed = match failures.as_slice() {
                    [] => None,
                    [one] => Some(one.clone()),
                    [first, ..] => Some(format!(
                        "{} of {total} files could not be read ({first})",
                        failures.len()
                    )),
                };
                (reads.into_iter().map(Result::ok).collect(), failed)
            }
            Ok(_) => (
                vec![None; total],
                Some("the photos changed while they were read".into()),
            ),
            Err(why) => (vec![None; total], Some(why)),
        };
        self.state = match &failed {
            None => State::Ready,
            Some(why) => State::Failed(why.clone()),
        };
        Some((fields, failed))
    }
}

/// What the files of a scope give, `standalone` when its photo has no
/// catalog: the catalog has the rest.
pub(super) fn file_sourced(standalone: bool) -> Needs {
    Needs {
        captured: standalone,
        info: standalone,
        camera: true,
    }
}

/// Puts what `photo`'s file gave in its fields.
pub(super) fn fill(fields: &mut NameFields, read: FileFields, standalone: bool) {
    fields.camera = read.camera;
    if standalone {
        fields.captured = read.captured;
        fields.info = read.info.unwrap_or_default();
    }
}

/// The real reader: the camera's make and model from the file's EXIF, else
/// (a CR3 and other RAWs whose EXIF isn't read) from its RAW header, as the
/// file writes them; the maker spelled as RAWmakase spells it, as Lightroom
/// does ("SONY" is "Sony"). A photo without a catalog gets its capture time
/// and photo info (dimensions included) as the catalog reads them for
/// photos added from folders. A file that can't be opened now is an error,
/// never a photo without names.
fn read_file(photo: &ToRead) -> Result<FileFields, String> {
    let path = &photo.path;
    let unreadable = || {
        format!(
            "{} can't be read",
            path.file_name().unwrap_or_default().to_string_lossy()
        )
    };
    if !crate::catalog_session::background::can_read(path) {
        return Err(unreadable());
    }
    let (captured, info) = if photo.standalone {
        let info = crate::catalog_session::file_photo_info(path).ok_or_else(unreadable)?;
        (crate::catalog_session::file_capture_time(path), info)
    } else {
        (None, None)
    };
    let (make, model) = match crate::exif::camera_names(path) {
        Some((make, model)) if !(make.is_empty() && model.is_empty()) => (make, model),
        _ => crate::raw::file_camera_names(path)
            .map(|n| (n.make, n.model))
            .unwrap_or_default(),
    };
    let known = |text: String| Some(text.trim().to_string()).filter(|t| !t.is_empty());
    Ok(FileFields {
        camera: CameraNames::Read {
            make: known(make).map(|make| crate::raw::maker_name(&make)),
            model: known(model),
        },
        captured,
        info,
    })
}

/// For tests: the real reader.
#[cfg(test)]
pub(super) fn read_path(path: &std::path::Path, standalone: bool) -> Result<FileFields, String> {
    read_file(&ToRead {
        path: path.to_path_buf(),
        standalone,
    })
}
