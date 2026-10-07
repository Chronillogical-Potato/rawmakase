//! Sync Settings: the open photo's settings, by group, onto the other photos selected
//! with it, as one change that one Undo reverses. Each target's settings are worked
//! out for its own camera (see `develop::settings_groups`), off the UI thread.
use super::{
    Editor,
    history::{History, Step},
    settings_transfer::Settings,
    worker::Event,
};
use crate::model::recipe::Recipe;
use crate::{
    catalog::{Catalog, EditChange, EditToSave, HistoryUpdate, PhotoId, SavedHistory},
    develop::settings_groups::{self, GroupSelection, Source, Target},
    edits::{self, Origin},
    export_settings::ExportOptions,
    raw_defaults::DevelopDefaults,
};
use anyhow::{Context, Result, ensure};
use std::path::PathBuf;

/// A photo settings are synchronized to.
#[derive(Clone, Debug)]
pub(super) struct SyncTarget {
    pub id: PhotoId,
    pub path: PathBuf,
    pub name: String,
}

/// One photo's edit before and after a Sync.
#[derive(Clone, Debug, PartialEq)]
pub struct Synced {
    pub id: PhotoId,
    pub path: PathBuf,
    /// The edit before, or none when the photo had no RAWmakase edit yet.
    pub before: EditBefore,
    pub after: Recipe,
    /// The History saved with `after`, which Redo writes back.
    pub history: SavedHistory,
    /// The History before the Sync, which Undo writes back (empty when it had none).
    pub history_before: SavedHistory,
    /// The file as the Sync read it: Redo refuses a file changed since.
    pub identity: crate::storage::Identity,
}

/// A photo's edit before a Sync.
#[derive(Clone, Debug, PartialEq)]
pub enum EditBefore {
    Saved(Box<Recipe>),
    /// No RAWmakase edit: Develop started it from `starting`, its Lightroom edit or
    /// the camera defaults. Undo returns the photo to having none.
    None {
        starting: Box<Recipe>,
    },
}
impl EditBefore {
    fn recipe(&self) -> &Recipe {
        match self {
            EditBefore::Saved(r) | EditBefore::None { starting: r } => r,
        }
    }
}

/// A photo a Sync left as it was, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncFailure {
    pub name: String,
    pub reason: String,
}

/// A setting a Sync could not apply as asked on one photo (see `Transferred::notes`).
#[derive(Clone, Debug, PartialEq)]
pub struct SyncNote {
    pub name: String,
    pub note: String,
}

/// What a Sync did, in which catalog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncResult {
    pub change: BatchChange,
    pub catalog: PathBuf,
    pub synced: Vec<Synced>,
    pub failed: Vec<SyncFailure>,
    pub notes: Vec<SyncNote>,
}

/// A Sync for the shared undo log: every photo's edit before and after it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SyncCommand {
    pub sequence: u64,
    pub change: BatchChange,
    pub edits: Vec<Synced>,
}

/// What a batch change does to each of the other selected photos.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum BatchChange {
    /// Sync Settings: the open photo's settings in these groups.
    Settings(GroupSelection),
    /// Lightroom's Match Total Exposures: each photo's Exposure set so that its
    /// aperture, shutter speed and ISO end up as bright as the open photo.
    #[default]
    MatchTotalExposures,
}
impl BatchChange {
    /// The History step and Undo name.
    pub fn name(&self) -> &'static str {
        match self {
            BatchChange::Settings(_) => "Synchronize Settings",
            BatchChange::MatchTotalExposures => "Match Total Exposures",
        }
    }
}

/// Which edits an undo or redo of a Sync writes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SyncSide {
    Before,
    After,
}

/// The new edits of `targets`, prepared photo by photo and saved in one transaction.
/// A photo that cannot be prepared (offline, changed since its edit was saved) is
/// reported and left out; if saving fails, nothing is saved.
pub(super) fn synchronize(
    catalog: &Catalog,
    source: &Settings,
    change: &BatchChange,
    targets: &[SyncTarget],
    defaults: &DevelopDefaults,
    demosaic: crate::camera_data::Demosaic,
) -> SyncResult {
    let mut result = SyncResult {
        change: change.clone(),
        catalog: catalog.path.clone(),
        ..Default::default()
    };
    let mut prepared = Vec::new();
    for target in targets {
        match prepare(catalog, source, change, target, defaults, demosaic) {
            Ok(p) => {
                result.notes.extend(p.notes.iter().map(|note| SyncNote {
                    name: target.name.clone(),
                    note: note.clone(),
                }));
                prepared.extend(p.edit);
            }
            Err(e) => result.failed.push(SyncFailure {
                name: target.name.clone(),
                reason: format!("{e:#}"),
            }),
        }
    }
    // A file replaced since it was read gets nothing: its settings were worked out
    // for the file that was there.
    let (prepared, replaced): (Vec<_>, Vec<_>) = prepared.into_iter().partition(|p| {
        crate::storage::Identity::read(&p.synced.path).is_ok_and(|now| now == p.synced.identity)
    });
    result.failed.extend(replaced.iter().map(|p| SyncFailure {
        name: p.name.clone(),
        reason: "the file changed during the Sync".into(),
    }));
    let edits: Vec<EditToSave> = prepared
        .iter()
        .map(|p| EditToSave {
            id: p.synced.id,
            path: &p.synced.path,
            recipe: &p.synced.after,
            export: &p.export,
            history: HistoryUpdate::Replace(&p.synced.history),
        })
        .collect();
    match catalog.save_edits(&edits) {
        Ok(()) => result.synced = prepared.into_iter().map(|p| p.synced).collect(),
        Err(e) => result.failed.extend(prepared.iter().map(|p| SyncFailure {
            name: p.name.clone(),
            reason: format!("{e:#}"),
        })),
    }
    result
}

/// A target's new edit, ready to save.
struct PreparedEdit {
    synced: Synced,
    name: String,
    export: ExportOptions,
}

/// A target's new edit (`None` when the Sync changes nothing on it), and what could
/// not be applied as asked.
struct Prepared {
    edit: Option<PreparedEdit>,
    notes: Vec<String>,
}

fn prepare(
    catalog: &Catalog,
    source: &Settings,
    change: &BatchChange,
    target: &SyncTarget,
    defaults: &DevelopDefaults,
    demosaic: crate::camera_data::Demosaic,
) -> Result<Prepared> {
    // Read first, so a file replaced while its settings are worked out is noticed.
    let identity = crate::storage::Identity::read(&target.path)?;
    let raw = crate::photo::open(&target.path)
        .with_context(|| format!("{} can't be read", target.name))?;
    let metadata = raw.metadata.clone();
    let (profiles, _) = crate::camera_profiles::installed(&metadata);
    let record = catalog.edit_record(target.id)?;
    let resolved = edits::resolve(&record, &target.path, &metadata, &profiles, defaults)?;
    let mut starting_warnings = Vec::new();
    let before = match resolved.origin {
        Origin::Saved => EditBefore::Saved(Box::new(resolved.recipe)),
        // What its starting edit could not bring along is said, as when it is opened.
        origin => {
            starting_warnings = match origin {
                Origin::Lightroom => resolved
                    .warnings
                    .into_iter()
                    .map(|w| format!("Lightroom edit not fully rendered: {w}"))
                    .collect(),
                _ => resolved.warnings,
            };
            EditBefore::None {
                starting: Box::new(resolved.recipe),
            }
        }
    };
    let export = resolved.export;
    let transferred = match change {
        BatchChange::Settings(groups) => settings_groups::transfer(
            Source {
                recipe: &source.recipe,
                metadata: &source.metadata,
            },
            before.recipe(),
            groups,
            Target {
                metadata: &metadata,
                profiles: &profiles,
            },
        ),
        BatchChange::MatchTotalExposures => settings_groups::Transferred {
            recipe: crate::model::recipe::Recipe {
                exposure: matched_exposure(source, &metadata).with_context(|| {
                    format!("{} has no aperture, shutter speed or ISO", target.name)
                })?,
                ..before.recipe().clone()
            },
            notes: Vec::new(),
        },
    };
    // What its starting edit could not bring along is said, as when it is opened.
    let mut notes = starting_warnings;
    notes.extend(transferred.notes);
    let mut after = transferred.recipe;
    // Upright's corrections are analysed from each photo; the open photo's editor
    // does it on Paste, and here the photo is developed for it.
    // Guided solves this photo's own guides beside that analysis.
    if after.upright.needs_analysis() {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let image = raw.develop(crate::camera_data::Decode::full(demosaic), &cancel)?;
        if let Some(issue) = crate::develop::upright::complete(&mut after, &image)
            && after.upright.mode == crate::model::transform::UprightMode::Guided
        {
            notes.push(issue.message().into());
        }
    }
    let before_recipe = before.recipe().clone();
    if after == before_recipe {
        return Ok(Prepared { edit: None, notes });
    }
    // A History this release can't read (a newer one's) is not replaced.
    ensure!(
        catalog.load_history(target.id)?.is_some() || !catalog.has_history(target.id)?,
        "Its History is from a newer RAWmakase"
    );
    let saved = catalog
        .load_history(target.id)?
        .unwrap_or_else(|| SavedHistory {
            origin: before_recipe.clone(),
            steps: Vec::new(),
            applied: 0,
        });
    let history_before = saved.clone();
    let mut history = History::restored(saved, &before_recipe);
    let mut current = before_recipe;
    history.set(&after, &mut current, Step::new(change.name(), ""));
    Ok(Prepared {
        edit: Some(PreparedEdit {
            synced: Synced {
                id: target.id,
                path: target.path.clone(),
                before,
                after,
                history: history.saved(&current),
                history_before,
                identity,
            },
            name: target.name.clone(),
            export,
        }),
        notes,
    })
}

/// How much light a photo's camera settings let in, in stops from f/1, 1 s, ISO 100:
/// what Match Total Exposures evens out. `None` without all three.
pub(super) fn capture_stops(m: &crate::camera_data::Metadata) -> Option<f32> {
    (m.aperture > 0. && m.shutter > 0. && m.iso > 0.)
        .then(|| m.shutter.log2() - 2. * m.aperture.log2() + (m.iso / 100.).log2())
}

/// The Exposure that makes a photo shot with `target`'s settings as bright as the
/// source: a photo that let in a stop more light gets a stop less Exposure.
fn matched_exposure(source: &Settings, target: &crate::camera_data::Metadata) -> Option<f32> {
    let difference = capture_stops(&source.metadata)? - capture_stops(target)?;
    let valid = &crate::develop::params::ParameterId::Exposure
        .descriptor()
        .valid;
    Some((source.recipe.exposure + difference).clamp(*valid.start(), *valid.end()))
}

/// Writes one side of a Sync back, in one transaction, keeping each photo's History,
/// which notices the change when the photo opens. A photo that had no edit before
/// goes back to having none. `path` gives each photo's current location, which a
/// relink may have changed since.
pub(super) fn restore(
    catalog: &Catalog,
    edits: &[Synced],
    side: SyncSide,
    path: impl Fn(PhotoId) -> Option<PathBuf>,
) -> std::result::Result<(), SyncRestoreError> {
    // A photo removed since can't be restored; the caller drops the command.
    let paths = edits
        .iter()
        .map(|e| path(e.id))
        .collect::<Option<Vec<_>>>()
        .ok_or(SyncRestoreError::PhotoRemoved)?;
    let mut saves = Vec::with_capacity(edits.len());
    for (e, path) in edits.iter().zip(paths) {
        let (recipe, history) = match (side, &e.before) {
            (SyncSide::Before, EditBefore::None { .. }) => continue,
            // Undo brings back the History it had, redo branch included.
            (SyncSide::Before, EditBefore::Saved(r)) => {
                (r.as_ref(), HistoryUpdate::Replace(&e.history_before))
            }
            // Redo brings back the History the Sync saved, which Undo may have cleared.
            (SyncSide::After, _) => (&e.after, HistoryUpdate::Replace(&e.history)),
        };
        let saved = catalog
            .load_edit(e.id, &path)
            .map_err(SyncRestoreError::Write)?;
        // Without an edit there is no stored identity to protect the file: compare
        // with the one the Sync read before writing its settings again.
        if saved.is_none()
            && crate::storage::Identity::read(&path).map_err(SyncRestoreError::Write)? != e.identity
        {
            return Err(SyncRestoreError::Write(anyhow::anyhow!(
                "{} changed since the Sync",
                path.display()
            )));
        }
        let export = saved.map(|saved| saved.export).unwrap_or_default();
        saves.push((e.id, path, recipe, export, history));
    }
    let saves: Vec<EditToSave> = saves
        .iter()
        .map(|(id, path, recipe, export, history)| EditToSave {
            id: *id,
            path,
            recipe,
            export,
            history: *history,
        })
        .collect();
    let mut changes: Vec<EditChange> = saves.iter().map(EditChange::Save).collect();
    if side == SyncSide::Before {
        changes.extend(
            edits
                .iter()
                .filter(|e| matches!(e.before, EditBefore::None { .. }))
                .map(|e| EditChange::Clear { id: e.id }),
        );
    }
    catalog
        .change_edits(&changes)
        .map_err(SyncRestoreError::Write)
}

/// Why a Sync could not be undone or redone.
#[derive(Debug)]
pub(super) enum SyncRestoreError {
    /// One of its photos was removed from the catalog since.
    PhotoRemoved,
    Write(anyhow::Error),
}
impl std::error::Error for SyncRestoreError {}
impl std::fmt::Display for SyncRestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncRestoreError::PhotoRemoved => {
                f.write_str("a synchronized photo is no longer in the catalog")
            }
            SyncRestoreError::Write(e) => write!(f, "{e:#}"),
        }
    }
}

impl Editor {
    /// The other photos selected with the open one, which Sync applies to.
    pub(super) fn sync_targets(&self) -> Vec<SyncTarget> {
        let (Some(library), Some(open)) = (&self.library, self.document.catalog_photo) else {
            return Vec::new();
        };
        // The open photo's settings are not final until its Lightroom edit is in, or
        // while an Auto estimate is still to land on them.
        // A protected edit (its file changed) shows camera defaults, not its settings;
        // profiles still being imported would resolve differently photo to photo.
        if self.document.pending_lightroom.is_some()
            || self.document.metadata.is_none()
            || self.document.auto.is_running()
            || self.document.edit.save.is_protected()
            || self.importing.is_some()
        {
            return Vec::new();
        }
        // The open photo may be hidden by the filters and still selected. While Read
        // Metadata from Files runs, which photos have a Lightroom edit can still change.
        if !library.is_selected(open) || library.rereading() {
            return Vec::new();
        }
        let selected = library.selected_photos();
        selected
            .into_iter()
            .filter(|id| *id != open)
            .filter_map(|id| library.photo(id))
            .map(|p| SyncTarget {
                id: p.id,
                path: p.path.clone(),
                name: format!("{}{}", p.filename, crate::app::library::copy_suffix(p)),
            })
            .collect()
    }
    /// Synchronizes the open photo's `groups` to the other selected photos, in the
    /// background; the result arrives as [`Event::Synced`].
    pub(super) fn start_sync(&mut self, change: BatchChange) {
        // Matching to a photo without aperture, shutter speed and ISO means nothing.
        if change == BatchChange::MatchTotalExposures
            && self
                .document
                .metadata
                .as_ref()
                .and_then(capture_stops)
                .is_none()
        {
            self.status =
                "Match Total Exposures needs this photo's aperture, shutter speed and ISO".into();
            return;
        }
        let targets = self.sync_targets();
        if targets.is_empty() || self.activity.is_busy() || !self.flush() {
            return;
        }
        let (Some(source), Some(library)) = (self.current_settings(), &self.library) else {
            return;
        };
        let catalog = library.catalog.path.clone();
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        let defaults = self.raw_defaults.clone();
        let demosaic = self.demosaic;
        // Moving to another photo or catalog waits, so neither can see an edit change
        // underneath it.
        if !self.activity.begin_sync() {
            return;
        }
        self.status = format!(
            "{} on {}…",
            change.name(),
            super::widgets::plural(targets.len(), "photo", "photos")
        );
        std::thread::spawn(move || {
            // A panic (in Upright's analysis, say) still finishes the Sync.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Catalog::open(&catalog)
                    .map(|c| synchronize(&c, &source, &change, &targets, &defaults, demosaic))
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the Sync failed unexpectedly")))
            .unwrap_or_else(|e| SyncResult {
                change: change.clone(),
                catalog: catalog.clone(),
                notes: Vec::new(),
                synced: Vec::new(),
                failed: vec![SyncFailure {
                    name: "Catalog".into(),
                    reason: format!("{e:#}"),
                }],
            });
            let _ = tx.send(Event::Synced(Box::new(result)));
            ctx.request_repaint();
        });
    }
    /// A finished Sync: one command for Undo, and a status line naming what failed.
    pub(super) fn synced(&mut self, result: SyncResult) {
        self.activity.finish_sync();
        // A close asked for during the Sync is asked again, now that it can go ahead.
        if self.close_confirm {
            self.close_confirm = false;
            self.context
                .send_viewport_cmd(eframe::egui::ViewportCommand::Close);
        }
        // A result for a catalog no longer open must not reach this one's undo log.
        if self.library.as_ref().map(|l| &l.catalog.path) != Some(&result.catalog) {
            return;
        }
        let done = result.synced.len();
        if let Some(library) = &mut self.library {
            library.edits_changed(result.synced.iter().map(|e| e.id));
        }
        // The reference photo may be among them.
        self.load_reference();
        if done > 0 {
            self.undo_log
                .push(super::undo::Command::Sync(Box::new(SyncCommand {
                    sequence: crate::edit_session::sequence(),
                    change: result.change.clone(),
                    edits: result.synced,
                })));
        }
        let mut status = format!(
            "{}: {} changed",
            result.change.name(),
            super::widgets::plural(done, "photo", "photos")
        );
        for failure in &result.failed {
            status.push_str(&format!(
                " · {} not changed: {}",
                failure.name, failure.reason
            ));
        }
        for note in &result.notes {
            status.push_str(&format!(" · {}: {}", note.name, note.note));
        }
        self.status = status;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// A catalog of copies of the synthetic chart DNG, and one file that isn't a photo.
    struct Fixture {
        _dir: tempfile::TempDir,
        catalog: Catalog,
        photos: Vec<(PhotoId, PathBuf)>,
    }
    fn catalog() -> Result<Fixture> {
        let d = tempfile::tempdir()?;
        let photos = d.path().join("photos");
        std::fs::create_dir(&photos)?;
        let chart =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/synthetic-d65.dng");
        for name in ["a.dng", "b.dng", "c.dng"] {
            std::fs::copy(&chart, photos.join(name))?;
        }
        std::fs::write(photos.join("d.dng"), b"not a photo")?;
        let mut c = Catalog::create(&d.path().join("sync.rawmakase"))?;
        c.add_folder(&photos)?;
        let photos = c.photos()?.into_iter().map(|p| (p.id, p.path)).collect();
        Ok(Fixture {
            _dir: d,
            catalog: c,
            photos,
        })
    }
    fn target(id: PhotoId, path: &Path) -> SyncTarget {
        SyncTarget {
            id,
            path: path.to_path_buf(),
            name: path.file_name().unwrap().to_string_lossy().into(),
        }
    }

    #[test]
    fn sync_saves_every_photo_it_can_with_a_history_step_and_undoes_together() -> Result<()> {
        let Fixture {
            _dir,
            catalog: c,
            photos,
        } = catalog()?;
        let metadata = crate::photo::open(&photos[0].1)?.metadata;
        let (profiles, _) = crate::camera_profiles::installed(&metadata);
        let mut source = Recipe::with_profiles(&metadata, &profiles);
        source.exposure = 0.5;
        source.effects.clarity = 0.2;
        let source = Settings {
            recipe: source,
            metadata,
        };
        let targets: Vec<_> = photos[1..].iter().map(|(id, p)| target(*id, p)).collect();
        let result = synchronize(
            &c,
            &source,
            &BatchChange::Settings(GroupSelection::default()),
            &targets,
            &DevelopDefaults::default(),
            crate::camera_data::Demosaic::default(),
        );
        // The file that isn't a photo is reported; the two charts are saved.
        assert_eq!(result.synced.len(), 2);
        assert_eq!(result.failed.len(), 1, "{:?}", result.failed);
        assert_eq!(result.failed[0].name, "d.dng");
        for (id, path) in &photos[1..3] {
            let saved = c.load_edit(*id, path)?.unwrap().recipe;
            assert_eq!((saved.exposure, saved.effects.clarity), (0.5, 0.2));
            let history = c.load_history(*id)?.unwrap();
            assert_eq!(history.steps.last().unwrap().name, "Synchronize Settings");
        }
        let path = |id: PhotoId| {
            photos
                .iter()
                .find(|(p, _)| *p == id)
                .map(|(_, path)| path.clone())
        };
        // One Undo restores every photo; these had no edit, and have none again.
        restore(&c, &result.synced, SyncSide::Before, path)?;
        for (id, path) in &photos[1..3] {
            assert!(c.load_edit(*id, path)?.is_none());
            assert!(c.load_history(*id)?.is_none());
        }
        // Redo brings the edit back with its History step.
        restore(&c, &result.synced, SyncSide::After, path)?;
        let history = c.load_history(photos[1].0)?.unwrap();
        assert_eq!(history.steps.last().unwrap().name, "Synchronize Settings");
        // A photo removed since makes the command unusable rather than wrong.
        assert!(matches!(
            restore(&c, &result.synced, SyncSide::Before, |_| None),
            Err(SyncRestoreError::PhotoRemoved)
        ));
        assert_eq!(
            c.load_edit(photos[1].0, &photos[1].1)?
                .unwrap()
                .recipe
                .exposure,
            0.5
        );
        // Undo on a photo that had an edit and History brings both back.
        let (id, path0) = (photos[1].0, photos[1].1.clone());
        let mut edited = c.load_edit(id, &path0)?.unwrap().recipe;
        edited.exposure = -0.3;
        let earlier = SavedHistory {
            origin: Recipe::default(),
            steps: vec![crate::catalog::SavedStep {
                name: "Exposure".into(),
                value: "-0.30".into(),
                recipe: edited.clone(),
            }],
            applied: 1,
        };
        c.save_edit(
            id,
            &path0,
            &edited,
            &ExportOptions::default(),
            HistoryUpdate::Replace(&earlier),
        )?;
        let again = synchronize(
            &c,
            &source,
            &BatchChange::Settings(GroupSelection::default()),
            &targets[..1],
            &DevelopDefaults::default(),
            crate::camera_data::Demosaic::default(),
        );
        assert_eq!(again.synced.len(), 1);
        restore(&c, &again.synced, SyncSide::Before, path)?;
        assert_eq!(c.load_history(id)?, Some(earlier));
        assert_eq!(c.load_edit(id, &path0)?.unwrap().recipe.exposure, -0.3);
        restore(&c, &again.synced, SyncSide::After, path)?;
        // Settings the photos already have change nothing.
        let again = synchronize(
            &c,
            &source,
            &BatchChange::Settings(GroupSelection::default()),
            &targets[..2],
            &DevelopDefaults::default(),
            crate::camera_data::Demosaic::default(),
        );
        assert!(again.synced.is_empty() && again.failed.is_empty());
        // A file changed while it had no edit is not given the old settings again.
        restore(&c, &result.synced, SyncSide::Before, path)?;
        let changed = &photos[1].1;
        let mut bytes = std::fs::read(changed)?;
        bytes.extend_from_slice(b"changed");
        std::fs::write(changed, bytes)?;
        assert!(restore(&c, &result.synced, SyncSide::After, path).is_err());
        assert!(c.load_edit(photos[2].0, &photos[2].1)?.is_none());
        Ok(())
    }

    #[test]
    fn match_total_exposures_evens_out_aperture_shutter_and_iso() -> Result<()> {
        let camera = |aperture: f32, shutter: f32, iso: f32| crate::camera_data::Metadata {
            aperture,
            shutter,
            iso,
            ..Default::default()
        };
        let source = Settings {
            recipe: Recipe {
                exposure: 0.5,
                ..Default::default()
            },
            metadata: camera(4., 1. / 125., 100.),
        };
        // A stop more light (1/60 s): a stop less Exposure. Twice the ISO and a stop
        // smaller aperture cancel out.
        assert!(
            (matched_exposure(&source, &camera(4., 1. / 62.5, 100.)).unwrap() + 0.5).abs() < 1e-4
        );
        assert!(
            (matched_exposure(&source, &camera(5.6568, 1. / 125., 200.)).unwrap() - 0.5).abs()
                < 1e-3
        );
        assert_eq!(
            matched_exposure(&source, &camera(0., 1. / 125., 100.)),
            None
        );
        // On photos without those settings it says so and leaves them alone.
        let Fixture {
            _dir,
            catalog: c,
            photos,
        } = catalog()?;
        let result = synchronize(
            &c,
            &source,
            &BatchChange::MatchTotalExposures,
            &[target(photos[1].0, &photos[1].1)],
            &DevelopDefaults::default(),
            crate::camera_data::Demosaic::default(),
        );
        assert!(result.synced.is_empty());
        assert!(
            result.failed[0].reason.contains("aperture"),
            "{:?}",
            result.failed
        );
        Ok(())
    }

    #[test]
    fn a_history_from_a_newer_release_is_not_replaced() -> Result<()> {
        let Fixture {
            _dir,
            catalog: c,
            photos,
        } = catalog()?;
        let metadata = crate::photo::open(&photos[0].1)?.metadata;
        let source = Settings {
            recipe: Recipe {
                exposure: 0.3,
                ..Default::default()
            },
            metadata,
        };
        let (id, path) = (photos[1].0, photos[1].1.clone());
        c.save_edit(
            id,
            &path,
            &Recipe::default(),
            &ExportOptions::default(),
            HistoryUpdate::Keep,
        )?;
        // What a newer release might store: a format this one can't read.
        let newer = {
            use std::io::Write;
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), Default::default());
            z.write_all(br#"{"version": 99}"#)?;
            z.finish()?
        };
        c.db_for_tests().execute(
            "INSERT INTO develop_history(photo, data) VALUES (?, ?)",
            rusqlite::params![id, newer],
        )?;
        let result = synchronize(
            &c,
            &source,
            &BatchChange::Settings(GroupSelection::default()),
            &[target(id, &path)],
            &DevelopDefaults::default(),
            crate::camera_data::Demosaic::default(),
        );
        assert_eq!(result.failed.len(), 1, "{:?}", result.failed);
        assert_eq!(c.load_edit(id, &path)?.unwrap().recipe.exposure, 0.);
        Ok(())
    }

    #[test]
    fn a_lightroom_edit_that_cant_be_read_fails_its_photo_and_notes_are_reported() -> Result<()> {
        let Fixture {
            _dir,
            catalog: c,
            photos,
        } = catalog()?;
        let metadata = crate::photo::open(&photos[0].1)?.metadata;
        let mut recipe = Recipe::default();
        recipe.upright.mode = crate::model::transform::UprightMode::Guided;
        recipe.exposure = 0.3;
        let source = Settings { recipe, metadata };
        // Settings cut off mid-value.
        rusqlite::Connection::open(&c.path)?.execute(
            "UPDATE photos SET lightroom_develop='s = { Exposure2012 = ' WHERE id=?",
            [photos[1].0],
        )?;
        let targets = [
            target(photos[1].0, &photos[1].1),
            target(photos[2].0, &photos[2].1),
        ];
        let result = synchronize(
            &c,
            &source,
            &BatchChange::Settings(GroupSelection::default()),
            &targets,
            &DevelopDefaults::default(),
            crate::camera_data::Demosaic::default(),
        );
        assert_eq!(result.failed.len(), 1, "{:?}", result.failed);
        assert!(result.failed[0].reason.contains("Lightroom edit"));
        assert!(c.load_edit(photos[1].0, &photos[1].1)?.is_none());
        // Guided Upright can't move without this photo's guides: said, not hidden.
        assert_eq!(result.synced.len(), 1);
        assert!(
            result.notes.iter().any(|n| n.note.contains("Guided")),
            "{:?}",
            result.notes
        );
        Ok(())
    }

    #[test]
    fn photos_without_an_edit_start_from_the_raw_defaults_and_lightroom_edits_from_adobe_default()
    -> Result<()> {
        let Fixture {
            _dir,
            catalog: c,
            photos,
        } = catalog()?;
        let lightroom_text = "s = { Exposure2012 = 0.25 }";
        rusqlite::Connection::open(&c.path)?.execute(
            "UPDATE photos SET lightroom_develop=? WHERE id=?",
            rusqlite::params![lightroom_text, photos[1].0],
        )?;
        let metadata = crate::photo::open(&photos[0].1)?.metadata;
        let (profiles, _) = crate::camera_profiles::installed(&metadata);
        let mut source = Recipe::with_profiles(&metadata, &profiles);
        source.effects.clarity = 0.2;
        let source = Settings {
            recipe: source,
            metadata: metadata.clone(),
        };
        let targets = [
            target(photos[1].0, &photos[1].1),
            target(photos[2].0, &photos[2].1),
        ];
        let mut clarity = GroupSelection::none();
        clarity.set(
            settings_groups::SettingGroup::Clarity,
            settings_groups::GroupInclusion::Included,
        );
        let defaults = crate::raw_defaults::brighter_defaults();
        let result = synchronize(
            &c,
            &source,
            &BatchChange::Settings(clarity),
            &targets,
            &defaults,
            crate::camera_data::Demosaic::default(),
        );
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        let starting =
            |id: PhotoId| match &result.synced.iter().find(|s| s.id == id).unwrap().before {
                EditBefore::None { starting } => (**starting).clone(),
                EditBefore::Saved(_) => panic!("photo {id} had no edit"),
            };
        // The Lightroom edit is relative to Adobe Default, whatever the raw defaults.
        let converted =
            crate::lr_develop::convert_develop(lightroom_text, &metadata, &profiles, None)?.0;
        assert_eq!(starting(photos[1].0), converted);
        assert_eq!(converted.exposure, 0.25);
        // The photo without one starts from the raw defaults.
        let start = starting(photos[2].0);
        assert_eq!(start, defaults.resolve(&metadata, &profiles).recipe);
        assert_eq!(start.exposure, 0.7);
        let saved = c.load_edit(photos[2].0, &photos[2].1)?.unwrap().recipe;
        assert_eq!((saved.exposure, saved.effects.clarity), (0.7, 0.2));

        // A raw default that can't be used is reported as such, not as a Lightroom edit.
        let missing = DevelopDefaults::with_presets(
            crate::raw_defaults::RawDefaults {
                master: crate::raw_defaults::DefaultChoice::Preset {
                    id: "gone".into(),
                    name: "Gone".into(),
                },
                ..Default::default()
            },
            |_| None,
        );
        let result = synchronize(
            &c,
            &source,
            &BatchChange::Settings(GroupSelection::default()),
            &[target(photos[0].0, &photos[0].1)],
            &missing,
            crate::camera_data::Demosaic::default(),
        );
        let notes: Vec<_> = result.notes.iter().map(|n| n.note.as_str()).collect();
        assert!(
            notes.iter().any(|n| n.contains("‘Gone’ is missing")),
            "{notes:?}"
        );
        assert!(!notes.iter().any(|n| n.contains("Lightroom")), "{notes:?}");
        Ok(())
    }
}
