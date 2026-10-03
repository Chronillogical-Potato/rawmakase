//! Sync Settings: the open photo's settings, by group, onto the other photos selected
//! with it, as one change that one Undo reverses. Each target's settings are worked
//! out for its own camera (see `develop::settings_groups`), off the UI thread.
use super::{
    Editor,
    history::{History, Step},
    settings_transfer::Settings,
    worker::Event,
};
use crate::{
    catalog::{Catalog, EditChange, EditToSave, HistoryUpdate, SavedHistory},
    develop::{
        Recipe,
        settings_groups::{self, GroupSelection, Source, Target},
    },
    export::ExportOptions,
};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// A photo settings are synchronized to.
#[derive(Clone, Debug)]
pub(super) struct SyncTarget {
    pub id: i64,
    pub path: PathBuf,
    pub name: String,
    /// Where its edit starts when RAWmakase has none yet.
    pub start: StartingEdit,
}

/// The edit a photo without a RAWmakase edit has, as Develop would open it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StartingEdit {
    Defaults,
    Lightroom,
}

/// One photo's edit before and after a Sync.
#[derive(Clone, Debug, PartialEq)]
pub struct Synced {
    pub id: i64,
    pub path: PathBuf,
    /// The edit before, or none when the photo had no RAWmakase edit yet.
    pub before: EditBefore,
    pub after: Recipe,
    /// The History saved with `after`, which Redo writes back.
    pub history: SavedHistory,
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
    pub catalog: PathBuf,
    pub synced: Vec<Synced>,
    pub failed: Vec<SyncFailure>,
    pub notes: Vec<SyncNote>,
}

/// A Sync for the shared undo log: every photo's edit before and after it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SyncCommand {
    pub sequence: u64,
    pub edits: Vec<Synced>,
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
    groups: &GroupSelection,
    targets: &[SyncTarget],
) -> SyncResult {
    let mut result = SyncResult {
        catalog: catalog.path.clone(),
        ..Default::default()
    };
    let mut prepared = Vec::new();
    for target in targets {
        match prepare(catalog, source, groups, target) {
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
    groups: &GroupSelection,
    target: &SyncTarget,
) -> Result<Prepared> {
    let raw = crate::raw::Raw::open(&target.path)
        .with_context(|| format!("{} can't be read", target.name))?;
    let metadata = raw.metadata.clone();
    let (profiles, _) = crate::camera_profiles::installed(&metadata);
    let (before, export) = match catalog.load_edit(target.id, &target.path)? {
        Some(saved) => (EditBefore::Saved(Box::new(saved.recipe)), saved.export),
        None => (
            EditBefore::None {
                starting: Box::new(starting_edit(catalog, target, &metadata, &profiles)?),
            },
            ExportOptions::default(),
        ),
    };
    let transferred = settings_groups::transfer(
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
    );
    let notes = transferred.notes;
    let mut after = transferred.recipe;
    // Upright's corrections are analysed from each photo; the open photo's editor
    // does it on Paste, and here the photo is developed for it.
    let upright = &after.upright;
    if !matches!(
        upright.mode,
        crate::develop::UprightMode::Off | crate::develop::UprightMode::Guided
    ) && upright.corrections.len() <= upright.mode.code()
    {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let image = raw.develop(false, &cancel)?;
        after.upright.corrections = crate::develop::upright::analyse(&image, &after);
    }
    let before_recipe = before.recipe().clone();
    if after == before_recipe {
        return Ok(Prepared { edit: None, notes });
    }
    let saved = catalog
        .load_history(target.id)?
        .unwrap_or_else(|| SavedHistory {
            origin: before_recipe.clone(),
            steps: Vec::new(),
            applied: 0,
        });
    let mut history = History::restored(saved, &before_recipe);
    let mut current = before_recipe;
    history.set(&after, &mut current, Step::new("Synchronize Settings", ""));
    Ok(Prepared {
        edit: Some(PreparedEdit {
            synced: Synced {
                id: target.id,
                path: target.path.clone(),
                before,
                after,
                history: history.saved(&current),
            },
            name: target.name.clone(),
            export,
        }),
        notes,
    })
}

/// The edit Develop opens a photo with when RAWmakase has none: its Lightroom edit,
/// else the camera defaults. A Lightroom edit that cannot be read fails the photo
/// rather than losing its unsynchronized settings.
fn starting_edit(
    catalog: &Catalog,
    target: &SyncTarget,
    metadata: &crate::raw::Metadata,
    profiles: &[std::sync::Arc<crate::camera_profiles::CameraProfile>],
) -> Result<Recipe> {
    if target.start == StartingEdit::Defaults {
        return Ok(Recipe::with_profiles(metadata, profiles));
    }
    let text = catalog
        .lightroom_develop(target.id)?
        .context("Its Lightroom edit is missing")?;
    let (recipe, _) = crate::catalog::convert_develop(&text, metadata, profiles, None)
        .context("Its Lightroom edit can't be read")?;
    Ok(recipe)
}

/// Writes one side of a Sync back, in one transaction, keeping each photo's History,
/// which notices the change when the photo opens. A photo that had no edit before
/// goes back to having none. `path` gives each photo's current location, which a
/// relink may have changed since.
pub(super) fn restore(
    catalog: &Catalog,
    edits: &[Synced],
    side: SyncSide,
    path: impl Fn(i64) -> Option<PathBuf>,
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
            (SyncSide::Before, EditBefore::Saved(r)) => (r.as_ref(), HistoryUpdate::Keep),
            // Redo brings back the History the Sync saved, which Undo may have cleared.
            (SyncSide::After, _) => (&e.after, HistoryUpdate::Replace(&e.history)),
        };
        let export = catalog
            .load_edit(e.id, &path)
            .map_err(SyncRestoreError::Write)?
            .map(|saved| saved.export)
            .unwrap_or_default();
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
        // The open photo's settings are not final until its Lightroom edit is in.
        if self.document.pending_lightroom || self.document.metadata.is_none() {
            return Vec::new();
        }
        let selected = library.selected_photos();
        if !selected.contains(&open) {
            return Vec::new();
        }
        selected
            .into_iter()
            .filter(|id| *id != open)
            .filter_map(|id| library.photo(id))
            .map(|p| SyncTarget {
                id: p.id,
                path: p.path.clone(),
                name: format!("{}{}", p.filename, crate::app::library::copy_suffix(p)),
                start: if p.has_lightroom_edits {
                    StartingEdit::Lightroom
                } else {
                    StartingEdit::Defaults
                },
            })
            .collect()
    }
    /// Synchronizes the open photo's `groups` to the other selected photos, in the
    /// background; the result arrives as [`Event::Synced`].
    pub(super) fn start_sync(&mut self, groups: GroupSelection) {
        let targets = self.sync_targets();
        if targets.is_empty() || self.activity.is_busy() || !self.flush() {
            return;
        }
        let (Some(source), Some(library)) = (self.current_settings(), &self.library) else {
            return;
        };
        let catalog = library.catalog.path.clone();
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        // Moving to another photo or catalog waits, so neither can see an edit change
        // underneath it.
        if !self.activity.begin_sync() {
            return;
        }
        self.status = format!(
            "Synchronizing {}…",
            super::widgets::plural(targets.len(), "photo", "photos")
        );
        std::thread::spawn(move || {
            // A panic (in Upright's analysis, say) still finishes the Sync.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                Catalog::open(&catalog).map(|c| synchronize(&c, &source, &groups, &targets))
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("the Sync failed unexpectedly")))
            .unwrap_or_else(|e| SyncResult {
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
        // A result for a catalog no longer open must not reach this one's undo log.
        if self.library.as_ref().map(|l| &l.catalog.path) != Some(&result.catalog) {
            return;
        }
        let done = result.synced.len();
        if let Some(library) = &mut self.library {
            library.edits_changed(result.synced.iter().map(|e| e.id));
        }
        if done > 0 {
            self.undo_log
                .push(super::undo::Command::Sync(Box::new(SyncCommand {
                    sequence: super::undo::sequence(),
                    edits: result.synced,
                })));
        }
        let mut status = format!(
            "Settings synchronized to {}",
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
        photos: Vec<(i64, PathBuf)>,
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
    fn target(id: i64, path: &Path) -> SyncTarget {
        SyncTarget {
            id,
            path: path.to_path_buf(),
            name: path.file_name().unwrap().to_string_lossy().into(),
            start: StartingEdit::Defaults,
        }
    }

    #[test]
    fn sync_saves_every_photo_it_can_with_a_history_step_and_undoes_together() -> Result<()> {
        let Fixture {
            _dir,
            catalog: c,
            photos,
        } = catalog()?;
        let metadata = crate::raw::Raw::open(&photos[0].1)?.metadata;
        let (profiles, _) = crate::camera_profiles::installed(&metadata);
        let mut source = Recipe::with_profiles(&metadata, &profiles);
        source.exposure = 0.5;
        source.effects.clarity = 0.2;
        let source = Settings {
            recipe: source,
            metadata,
        };
        let targets: Vec<_> = photos[1..].iter().map(|(id, p)| target(*id, p)).collect();
        let result = synchronize(&c, &source, &GroupSelection::default(), &targets);
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
        let path = |id: i64| {
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
        // Settings the photos already have change nothing.
        let again = synchronize(&c, &source, &GroupSelection::default(), &targets[..2]);
        assert!(again.synced.is_empty() && again.failed.is_empty());
        Ok(())
    }

    #[test]
    fn a_lightroom_edit_that_cant_be_read_fails_its_photo_and_notes_are_reported() -> Result<()> {
        let Fixture {
            _dir,
            catalog: c,
            photos,
        } = catalog()?;
        let metadata = crate::raw::Raw::open(&photos[0].1)?.metadata;
        let mut recipe = Recipe::default();
        recipe.upright.mode = crate::develop::UprightMode::Guided;
        recipe.exposure = 0.3;
        let source = Settings { recipe, metadata };
        let mut lightroom = target(photos[1].0, &photos[1].1);
        lightroom.start = StartingEdit::Lightroom;
        let targets = [lightroom, target(photos[2].0, &photos[2].1)];
        let result = synchronize(&c, &source, &GroupSelection::default(), &targets);
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
}
