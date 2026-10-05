//! Exporting several photos at once, each with its own edit, as Lightroom's Export
//! does with more than one photo selected.
//!
//! A batch is taken when Export is pressed: every photo's edit and metadata as the
//! catalog has them then ([`BatchPhoto`]), and no image. [`plan`] decides where
//! each file goes before anything renders; [`run`] then works out, renders and
//! writes one photo at a time, so a batch of any size holds one photo's images.
use super::{Existing, ExportSettings, Replace, assemble::Values, job};
use crate::{
    catalog::resolve::{self, EditRecord, Origin, PhotoRecord},
    develop::{Recipe, defaults::DevelopDefaults},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// One photo of a batch, as it was when Export was pressed.
#[derive(Clone, Debug)]
pub struct BatchPhoto {
    pub id: i64,
    pub source: PathBuf,
    /// How the photo is named in a report: its file name, and a virtual copy's
    /// name after it.
    pub name: String,
    pub edit: Edit,
    pub values: Values,
}

/// The edit a photo is exported with.
#[derive(Clone, Debug)]
pub enum Edit {
    /// As the catalog stores it, worked out when the photo's turn comes, as
    /// Develop would open it (`catalog::resolve`).
    Catalog(EditRecord),
    /// The open photo's edit as shown; `unsaved` when saving it failed.
    Shown { recipe: Box<Recipe>, unsaved: bool },
}

impl BatchPhoto {
    /// A photo of the batch from its catalog record.
    pub fn from_record(record: PhotoRecord, source: PathBuf, name: String) -> Self {
        Self {
            id: record.id,
            source,
            name,
            edit: Edit::Catalog(record.edit),
            values: Values {
                descriptive: record.descriptive,
                keywords: record
                    .keywords
                    .into_iter()
                    .map(|k| crate::xmp::write::KeywordPath {
                        path: k.path,
                        exported: k.exported,
                    })
                    .collect(),
                rating: record.rating,
                label: record.label,
            },
        }
    }
}

/// How one photo's file is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    /// A name nothing had when the batch was planned.
    Create,
    /// Over a file that was there, as asked.
    Overwrite,
    /// Not at all: a file was there, and the answer was Skip.
    Skip,
}

/// Where one photo goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub target: PathBuf,
    pub write: Write,
}

/// Where every photo of a batch goes, decided before anything renders.
#[derive(Clone, Debug)]
pub struct Plan {
    /// One for each photo, in order.
    pub entries: Vec<Planned>,
    /// What a file that appears while the batch runs gets: the answer the batch
    /// was given, or Skip when Ask had nothing to ask about.
    pub late: Existing,
    reserved: Reservations,
}

/// Why a batch could not be planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unplanned {
    /// "Specific folder" with no folder chosen.
    NoFolder,
    /// Ask what to do: these files exist. Plan again with the answer.
    Conflicts(Vec<PathBuf>),
}
impl std::fmt::Display for Unplanned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unplanned::NoFolder => f.write_str("Choose a folder to export to"),
            Unplanned::Conflicts(files) => write!(f, "{} files already exist", files.len()),
        }
    }
}

/// The names taken in each destination folder, compared without case, so the
/// names chosen work on any volume. Conservative on a case-sensitive one, where it
/// can rename a file that did not need it.
#[derive(Clone, Debug, Default)]
struct Reservations {
    /// Names already in each folder when the batch was planned, folded, with the
    /// file's own name.
    on_disk: HashMap<PathBuf, HashMap<String, OsString>>,
    /// Names given to the batch's own files.
    batch: HashMap<PathBuf, HashSet<String>>,
}
fn fold(name: &std::ffi::OsStr) -> String {
    name.to_string_lossy().to_lowercase()
}
impl Reservations {
    fn folder(&mut self, dir: &Path) -> &HashMap<String, OsString> {
        self.on_disk.entry(dir.to_path_buf()).or_insert_with(|| {
            std::fs::read_dir(dir)
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|e| (fold(&e.file_name()), e.file_name()))
                        .collect()
                })
                .unwrap_or_default()
        })
    }
    /// The file already in `path`'s folder under its name in any case, as it is
    /// spelled there.
    fn existing(&mut self, path: &Path) -> Option<PathBuf> {
        let name = fold(path.file_name().unwrap_or_default());
        let dir = crate::storage::parent_dir(path);
        let found = self.folder(dir).get(&name)?.clone();
        Some(dir.join(found))
    }
    fn on_disk(&mut self, path: &Path) -> bool {
        self.existing(path).is_some()
    }
    fn in_batch(&self, path: &Path) -> bool {
        let name = fold(path.file_name().unwrap_or_default());
        self.batch
            .get(crate::storage::parent_dir(path))
            .is_some_and(|names| names.contains(&name))
    }
    fn reserve(&mut self, path: &Path) {
        let name = fold(path.file_name().unwrap_or_default());
        self.batch
            .entry(crate::storage::parent_dir(path).to_path_buf())
            .or_default()
            .insert(name);
    }
    /// A file found while the batch runs: no later photo takes its name.
    fn found(&mut self, path: &Path) {
        let name = path.file_name().unwrap_or_default();
        let dir = crate::storage::parent_dir(path).to_path_buf();
        self.folder(&dir);
        self.on_disk
            .entry(dir)
            .or_default()
            .insert(fold(name), name.to_os_string());
    }
    /// The first of `path`, "name-2", "name-3"… that is neither in the folder nor
    /// the batch's, reserved for the batch.
    fn unique(&mut self, path: &Path) -> PathBuf {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let extension = path.extension().map(|e| e.to_string_lossy());
        let named = |n: usize| {
            let mut name = OsString::from(if n == 1 {
                stem.to_string()
            } else {
                format!("{stem}-{n}")
            });
            if let Some(extension) = &extension {
                name.push(format!(".{extension}"));
            }
            path.with_file_name(name)
        };
        let free = (1..)
            .map(named)
            .find(|p| !self.in_batch(p) && !self.on_disk(p))
            .expect("an unused name");
        self.reserve(&free);
        free
    }
}

/// Where each of `photos` goes with `settings`. With Ask, files that already exist
/// are returned for the question unless `answer` is given; the answer applies to
/// all of them. A later photo never replaces an earlier one's file: it gets the
/// next unique name, as Use Unique Names does.
pub fn plan(
    photos: &[BatchPhoto],
    settings: &ExportSettings,
    answer: Option<Existing>,
) -> Result<Plan, Unplanned> {
    let targets = photos
        .iter()
        .map(|p| settings.target(&p.source))
        .collect::<Option<Vec<_>>>()
        .ok_or(Unplanned::NoFolder)?;
    let mut reserved = Reservations::default();
    let mut conflicts: Vec<PathBuf> = Vec::new();
    for target in &targets {
        if let Some(existing) = reserved.existing(target)
            && !conflicts.contains(&existing)
        {
            conflicts.push(existing);
        }
    }
    let choice = match (settings.existing, answer) {
        (Existing::Ask, Some(answer)) if answer != Existing::Ask => answer,
        (Existing::Ask, _) if !conflicts.is_empty() => return Err(Unplanned::Conflicts(conflicts)),
        // Nothing to ask about: a file that appears later has no answer, so it is
        // not replaced.
        (Existing::Ask, _) => Existing::Skip,
        (existing, _) => existing,
    };
    let entries = targets
        .into_iter()
        .map(|target| {
            if reserved.in_batch(&target) {
                let target = reserved.unique(&target);
                return Planned {
                    target,
                    write: Write::Create,
                };
            }
            if !reserved.on_disk(&target) {
                reserved.reserve(&target);
                return Planned {
                    target,
                    write: Write::Create,
                };
            }
            match choice {
                // The file that is there, as it is spelled: not a second one beside
                // it in other case.
                Existing::Overwrite => {
                    let target = reserved.existing(&target).unwrap_or(target);
                    reserved.reserve(&target);
                    Planned {
                        target,
                        write: Write::Overwrite,
                    }
                }
                Existing::Unique => Planned {
                    target: reserved.unique(&target),
                    write: Write::Create,
                },
                Existing::Skip | Existing::Ask => Planned {
                    target,
                    write: Write::Skip,
                },
            }
        })
        .collect();
    Ok(Plan {
        entries,
        late: choice,
        reserved,
    })
}

/// What happened to one photo.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Written to `path`, with what the export has to say besides.
    Exported {
        path: PathBuf,
        notes: Vec<String>,
    },
    Skipped(String),
    Failed(String),
    /// Stopped part-way by Cancel: nothing written.
    Cancelled,
    /// Cancelled before its turn.
    NotStarted,
}

/// A batch ready to run: its photos, where each goes, and the settings it shares.
#[derive(Clone, Debug)]
pub struct Batch {
    pub photos: Vec<BatchPhoto>,
    pub plan: Plan,
    pub settings: ExportSettings,
    /// What a photo without an edit starts from, as when it is opened.
    pub defaults: Arc<DevelopDefaults>,
    /// The watermark chosen in the Export dialog.
    pub watermark: Option<crate::watermark::Watermark>,
}

/// How far a batch is: `done` photos of `total`, and how far the current one is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub fraction: f32,
}

/// Exports `batch` one photo at a time, reporting progress, and returns each
/// photo's outcome. Cancel stops between stages: a cancel observed before a file
/// is committed discards it; a commit already under way may finish, and that photo
/// counts as exported.
pub fn run(batch: &Batch, cancel: &AtomicBool, progress: impl Fn(Progress)) -> Vec<Outcome> {
    let total = batch.photos.len();
    let mut reserved = batch.plan.reserved.clone();
    let mut outcomes = Vec::with_capacity(total);
    for (done, (photo, planned)) in batch.photos.iter().zip(&batch.plan.entries).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            outcomes.push(Outcome::NotStarted);
            continue;
        }
        let report = |fraction| {
            progress(Progress {
                done,
                total,
                fraction,
            })
        };
        report(0.);
        let outcome = match planned.write {
            Write::Skip => Outcome::Skipped("a file with its name already exists".into()),
            // A panic fails its own photo; the others keep their outcomes.
            _ => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                export(batch, photo, planned, &mut reserved, cancel, &report)
            }))
            .unwrap_or_else(|_| Outcome::Failed("the export failed unexpectedly".into())),
        };
        outcomes.push(outcome);
    }
    progress(Progress {
        done: total,
        total,
        fraction: 0.,
    });
    outcomes
}

/// Why a photo stopped, kept apart from failures until the end.
enum Stop {
    Cancelled,
    Skipped(String),
    Failed(anyhow::Error),
}
impl From<anyhow::Error> for Stop {
    fn from(e: anyhow::Error) -> Self {
        Stop::Failed(e)
    }
}

fn export(
    batch: &Batch,
    photo: &BatchPhoto,
    planned: &Planned,
    reserved: &mut Reservations,
    cancel: &AtomicBool,
    progress: &impl Fn(f32),
) -> Outcome {
    let result = (|| -> Result<(PathBuf, Vec<String>), Stop> {
        let (prepared, mut notes) = render(batch, photo, cancel, progress)?;
        if let Some(parent) = planned.target.parent() {
            std::fs::create_dir_all(parent).map_err(anyhow::Error::from)?;
        }
        let staged = super::stage(
            &planned.target,
            &prepared.rendered,
            &prepared.metadata,
            &prepared.options,
            &prepared.embed,
        )?;
        progress(0.95);
        let (path, note) = commit(staged, photo, planned, batch.plan.late, reserved, cancel)?;
        notes.extend(note);
        progress(1.);
        Ok((path, notes))
    })();
    match result {
        Ok((path, notes)) => Outcome::Exported { path, notes },
        Err(Stop::Skipped(reason)) => Outcome::Skipped(reason),
        // Whatever stopped it, a cancel seen is a cancel, not a failure.
        Err(_) if cancel.load(Ordering::Relaxed) => Outcome::Cancelled,
        Err(Stop::Cancelled) => Outcome::Cancelled,
        Err(Stop::Failed(e)) => Outcome::Failed(format!("{e:#}")),
    }
}

/// `photo` rendered with its edit, and what the export has to say about it.
fn render(
    batch: &Batch,
    photo: &BatchPhoto,
    cancel: &AtomicBool,
    progress: &impl Fn(f32),
) -> Result<(job::Prepared, Vec<String>)> {
    let raw = crate::raw::Raw::open(&photo.source)
        .with_context(|| format!("{} can't be read", photo.source.display()))?;
    let (mut recipe, mut notes) = match &photo.edit {
        Edit::Catalog(record) => {
            let (profiles, _) = crate::camera_profiles::installed(&raw.metadata);
            let resolved = resolve::resolve(
                record,
                &photo.source,
                &raw.metadata,
                &profiles,
                &batch.defaults,
            )?;
            let notes = match resolved.origin {
                Origin::Lightroom => resolved
                    .warnings
                    .into_iter()
                    .map(|w| format!("Lightroom edit not fully rendered: {w}"))
                    .collect(),
                _ => resolved.warnings,
            };
            (resolved.recipe, notes)
        }
        Edit::Shown { recipe, unsaved } => (
            (**recipe).clone(),
            unsaved
                .then(|| "exported using unsaved adjustments".to_string())
                .into_iter()
                .collect(),
        ),
    };
    ensure!(!cancel.load(Ordering::Relaxed), "Cancelled");
    let image = Arc::new(job::decode_full(raw, &photo.source, cancel)?);
    // Upright's corrections, as Develop works them out once the photo is open.
    if let Some(issue) = crate::develop::upright::complete(&mut recipe, &image)
        && recipe.upright.mode == crate::develop::UprightMode::Guided
    {
        notes.push(issue.message().into());
    }
    let prepared = job::prepare(
        &job::Photo {
            image,
            source: photo.source.clone(),
            recipe,
            values: photo.values.clone(),
            watermark: batch.watermark.clone(),
        },
        &batch.settings,
        cancel,
        &|p| progress(p * 0.9),
    )?;
    notes.extend(prepared.notice.clone());
    Ok((prepared, notes))
}

/// Puts a staged export in place, as `planned` says, unless Cancel came first.
/// A file that appeared at a name planned as new gets the batch's answer.
fn commit(
    mut staged: tempfile::NamedTempFile,
    photo: &BatchPhoto,
    planned: &Planned,
    late: Existing,
    reserved: &mut Reservations,
    cancel: &AtomicBool,
) -> Result<(PathBuf, Option<String>), Stop> {
    // The last moment to stop: dropping the staged file leaves nothing behind.
    if cancel.load(Ordering::Relaxed) {
        return Err(Stop::Cancelled);
    }
    let mut target = planned.target.clone();
    if planned.write == Write::Overwrite {
        overwrite(staged, &target, &photo.source)?;
        return Ok((target, None));
    }
    loop {
        // A file under the name in other case is the same name here; an exact-name
        // check alone misses it on a case-sensitive volume.
        let appeared = match spelled_otherwise(&target) {
            Some(existing) => existing,
            None => match staged.persist_noclobber(&target) {
                Ok(_) => {
                    crate::storage::sync_dir(crate::storage::parent_dir(&target))?;
                    return Ok((target, None));
                }
                Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    staged = e.file;
                    target.clone()
                }
                Err(e) => return Err(Stop::Failed(e.error.into())),
            },
        };
        reserved.found(&appeared);
        match late {
            Existing::Unique => target = reserved.unique(&target),
            Existing::Overwrite => {
                overwrite(staged, &appeared, &photo.source)?;
                return Ok((
                    appeared,
                    Some("replaced a file that appeared during the export".into()),
                ));
            }
            Existing::Skip | Existing::Ask => {
                return Err(Stop::Skipped(
                    "a file with its name appeared during the export".into(),
                ));
            }
        }
    }
}

/// The file in `path`'s folder whose name is `path`'s in other case, if any.
fn spelled_otherwise(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    let folded = fold(name);
    let dir = crate::storage::parent_dir(path);
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name())
        .find(|other| other != name && fold(other) == folded)
        .map(|other| dir.join(other))
}

/// Replaces the file at `target` with `staged`, never the photo's own file.
fn overwrite(staged: tempfile::NamedTempFile, target: &Path, source: &Path) -> Result<()> {
    if target.exists() && std::fs::canonicalize(target)? == std::fs::canonicalize(source)? {
        bail!("Cannot overwrite source");
    }
    crate::storage::persist(staged, target, Replace::Overwrite)
}

#[cfg(test)]
mod tests;
