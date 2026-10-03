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
    catalog::{Catalog, EditToSave, HistoryUpdate, SavedHistory},
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
    pub before: Recipe,
    pub after: Recipe,
}

/// A photo a Sync left as it was, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncFailure {
    pub name: String,
    pub reason: String,
}

/// What a Sync did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncResult {
    pub synced: Vec<Synced>,
    pub failed: Vec<SyncFailure>,
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
    let mut result = SyncResult::default();
    let mut prepared = Vec::new();
    for target in targets {
        match prepare(catalog, source, groups, target) {
            Ok(Some(p)) => prepared.push(p),
            Ok(None) => {}
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
            history: HistoryUpdate::Replace(&p.history),
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
struct Prepared {
    synced: Synced,
    name: String,
    export: ExportOptions,
    history: SavedHistory,
}

/// `None` when the Sync changes nothing on this photo.
fn prepare(
    catalog: &Catalog,
    source: &Settings,
    groups: &GroupSelection,
    target: &SyncTarget,
) -> Result<Option<Prepared>> {
    let metadata = crate::raw::Raw::open(&target.path)
        .with_context(|| format!("{} can't be read", target.name))?
        .metadata;
    let (profiles, _) = crate::camera_profiles::installed(&metadata);
    let (before, export) = match catalog.load_edit(target.id, &target.path)? {
        Some(saved) => (saved.recipe, saved.export),
        None => (
            starting_edit(catalog, target, &metadata, &profiles),
            ExportOptions::default(),
        ),
    };
    let after = settings_groups::transfer(
        Source {
            recipe: &source.recipe,
            metadata: &source.metadata,
        },
        &before,
        groups,
        Target {
            metadata: &metadata,
            profiles: &profiles,
        },
    )
    .recipe;
    if after == before {
        return Ok(None);
    }
    let saved = catalog
        .load_history(target.id)?
        .unwrap_or_else(|| SavedHistory {
            origin: before.clone(),
            steps: Vec::new(),
            applied: 0,
        });
    let mut history = History::restored(saved, &before);
    let mut current = before.clone();
    history.set(&after, &mut current, Step::new("Synchronize Settings", ""));
    Ok(Some(Prepared {
        history: history.saved(&current),
        synced: Synced {
            id: target.id,
            path: target.path.clone(),
            before,
            after,
        },
        name: target.name.clone(),
        export,
    }))
}

/// The edit Develop opens a photo with when RAWmakase has none: its Lightroom edit,
/// else the camera defaults.
fn starting_edit(
    catalog: &Catalog,
    target: &SyncTarget,
    metadata: &crate::raw::Metadata,
    profiles: &[std::sync::Arc<crate::camera_profiles::CameraProfile>],
) -> Recipe {
    let defaults = || Recipe::with_profiles(metadata, profiles);
    if target.start == StartingEdit::Defaults {
        return defaults();
    }
    catalog
        .lightroom_develop(target.id)
        .ok()
        .flatten()
        .and_then(|text| crate::catalog::convert_develop(&text, metadata, profiles, None).ok())
        .map_or_else(defaults, |(recipe, _)| recipe)
}

/// Writes one side of a Sync back, in one transaction, keeping each photo's History,
/// which notices the change when the photo opens.
pub(super) fn restore(catalog: &Catalog, edits: &[Synced], side: SyncSide) -> Result<()> {
    let mut exports = Vec::with_capacity(edits.len());
    for e in edits {
        exports.push(
            catalog
                .load_edit(e.id, &e.path)?
                .map(|saved| saved.export)
                .unwrap_or_default(),
        );
    }
    let saves: Vec<EditToSave> = edits
        .iter()
        .zip(&exports)
        .map(|(e, export)| EditToSave {
            id: e.id,
            path: &e.path,
            recipe: match side {
                SyncSide::Before => &e.before,
                SyncSide::After => &e.after,
            },
            export,
            history: HistoryUpdate::Keep,
        })
        .collect();
    catalog.save_edits(&saves)
}

impl Editor {
    /// The other photos selected with the open one, which Sync applies to.
    pub(super) fn sync_targets(&self) -> Vec<SyncTarget> {
        let (Some(library), Some(open)) = (&self.library, self.document.catalog_photo) else {
            return Vec::new();
        };
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
        if targets.is_empty() || self.syncing || !self.flush() {
            return;
        }
        let (Some(source), Some(library)) = (self.current_settings(), &self.library) else {
            return;
        };
        let catalog = library.catalog.path.clone();
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        self.syncing = true;
        self.status = format!(
            "Synchronizing {}…",
            super::widgets::plural(targets.len(), "photo", "photos")
        );
        std::thread::spawn(move || {
            let result = Catalog::open(&catalog)
                .map(|c| synchronize(&c, &source, &groups, &targets))
                .unwrap_or_else(|e| SyncResult {
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
        self.syncing = false;
        let done = result.synced.len();
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
        // One Undo restores every photo.
        restore(&c, &result.synced, SyncSide::Before)?;
        for (id, path) in &photos[1..3] {
            assert_eq!(c.load_edit(*id, path)?.unwrap().recipe.exposure, 0.);
        }
        restore(&c, &result.synced, SyncSide::After)?;
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
}
