//! The edit a photo is developed with, worked out the same way wherever it is
//! needed: its saved RAWmakase edit, else its Lightroom edit, else the raw
//! defaults. Develop, Sync Settings and Export all start from here.
//!
//! The catalog is read first ([`Catalog::edit_record`]) and the edit worked out
//! after ([`resolve`]), so a caller can read many photos at once and resolve them
//! later, off the UI thread. Upright's analysis needs the developed photo, so it is
//! a step of its own: [`crate::develop::upright::complete`].
use super::{Catalog, SavedEdit, edits::local_edits};
use crate::{
    camera_profiles::CameraProfile, develop::Recipe, develop::defaults::DevelopDefaults,
    export::ExportOptions, raw::Metadata, storage::Identity,
};
use anyhow::{Context, Result, ensure};
use std::{path::Path, sync::Arc};

/// One photo's edit as the catalog stores it, not yet read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditRecord {
    pub recipe: Option<String>,
    pub export: Option<String>,
    /// The file the recipe was saved for.
    pub identity: Option<String>,
    /// Spots and masks, saved apart from the recipe.
    pub local: Option<String>,
    /// Lightroom's develop settings, from an imported catalog.
    pub lightroom: Option<String>,
}

/// Where a photo's edit came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Saved,
    Lightroom,
    Defaults,
}

/// A photo's edit, worked out.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub recipe: Recipe,
    pub export: ExportOptions,
    pub origin: Origin,
    /// What the edit could not bring along: Lightroom settings not rendered yet, or
    /// why the chosen raw defaults could not be used.
    pub warnings: Vec<String>,
}

impl Catalog {
    /// Photo `id`'s edit as stored.
    pub fn edit_record(&self, id: i64) -> Result<EditRecord> {
        let (recipe, export, identity, lightroom) = self.db.query_row(
            "SELECT recipe,export_options,identity,lightroom_develop FROM photos WHERE id=?",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        Ok(EditRecord {
            recipe,
            export,
            identity,
            local: self.local_text(id)?,
            lightroom,
        })
    }
}

/// What exporting one photo needs from the catalog: its edit and the metadata the
/// file carries.
#[derive(Clone, Debug)]
pub struct PhotoRecord {
    pub id: i64,
    pub edit: EditRecord,
    pub descriptive: crate::metadata::Descriptive,
    pub keywords: Vec<crate::metadata::Keyword>,
    pub rating: i32,
    pub label: String,
    /// The capture time the catalog sorts by ("2026-05-04 10:21:33.000"), empty
    /// when unknown.
    pub captured: String,
}

impl Catalog {
    /// The records of `ids`, in order, read in one transaction: one consistent
    /// state of the catalog however many photos there are.
    pub fn photo_records(&self, ids: &[i64]) -> Result<Vec<PhotoRecord>> {
        let tx = self.db.unchecked_transaction()?;
        let records = ids
            .iter()
            .map(|&id| {
                let (rating, label, captured) = self.db.query_row(
                    "SELECT rating,label,captured FROM photos WHERE id=?",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?;
                Ok(PhotoRecord {
                    id,
                    edit: self.edit_record(id)?,
                    descriptive: self.descriptive(id)?,
                    keywords: self.keywords(id)?,
                    rating,
                    label,
                    captured,
                })
            })
            .collect::<Result<_>>()?;
        tx.commit()?;
        Ok(records)
    }
}

impl EditRecord {
    /// The saved RAWmakase edit, if there is one. An error when it can't be read, or
    /// when the file at `path` is not the one it was saved for: the edit is then
    /// protected, never replaced by another.
    pub fn saved(&self, path: &Path) -> Result<Option<SavedEdit>> {
        let Some(recipe) = &self.recipe else {
            return Ok(None);
        };
        let saved: Identity =
            serde_json::from_str(self.identity.as_deref().context("Missing photo identity")?)?;
        ensure!(
            saved == Identity::read(path)?,
            "Photo changed since this catalog edit was saved; catalog edit protected"
        );
        let recipe: Recipe = serde_json::from_str(recipe)?;
        let recipe = recipe.with_local(local_edits(self.local.as_deref())?);
        recipe.validate()?;
        let export: ExportOptions =
            serde_json::from_str(self.export.as_deref().context("Missing export settings")?)?;
        export.validate()?;
        Ok(Some(SavedEdit { recipe, export }))
    }
    /// Lightroom's develop settings, when the photo has any.
    pub fn lightroom(&self) -> Option<&str> {
        self.lightroom.as_deref().filter(|text| !text.is_empty())
    }
}

/// A Lightroom edit as RAWmakase renders it, converted from Adobe Default as
/// Lightroom stores it, and the settings it can't render yet.
pub fn lightroom_edit(
    text: &str,
    m: &Metadata,
    profiles: &[Arc<CameraProfile>],
) -> Result<(Recipe, Vec<String>)> {
    crate::lr_develop::convert_develop(text, m, profiles, None)
}

/// The edit photo `record` at `path` is developed with: its saved edit, else its
/// Lightroom edit, else `defaults`. An edit that is there but can't be read is an
/// error, never the defaults.
pub fn resolve(
    record: &EditRecord,
    path: &Path,
    m: &Metadata,
    profiles: &[Arc<CameraProfile>],
    defaults: &DevelopDefaults,
) -> Result<Resolved> {
    if let Some(saved) = record.saved(path)? {
        return Ok(Resolved {
            recipe: saved.recipe,
            export: saved.export,
            origin: Origin::Saved,
            warnings: Vec::new(),
        });
    }
    if let Some(text) = record.lightroom() {
        let (recipe, warnings) =
            lightroom_edit(text, m, profiles).context("Its Lightroom edit can't be read")?;
        return Ok(Resolved {
            recipe,
            export: ExportOptions::default(),
            origin: Origin::Lightroom,
            warnings,
        });
    }
    let resolved = defaults.resolve(m, profiles);
    Ok(Resolved {
        recipe: resolved.recipe,
        export: ExportOptions::default(),
        origin: Origin::Defaults,
        warnings: resolved.note.into_iter().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop::masks;

    /// Each photo's catalog id and file.
    type Photos = Vec<(i64, std::path::PathBuf)>;
    /// A catalog of three copies of the synthetic chart DNG.
    fn catalog() -> Result<(tempfile::TempDir, Catalog, Photos)> {
        let d = tempfile::tempdir()?;
        let photos = d.path().join("photos");
        std::fs::create_dir(&photos)?;
        let chart =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/synthetic-d65.dng");
        for name in ["a.dng", "b.dng", "c.dng"] {
            std::fs::copy(&chart, photos.join(name))?;
        }
        let mut c = Catalog::create(&d.path().join("resolve.rawmakase"))?;
        c.add_folder(&photos)?;
        let photos = c.photos()?.into_iter().map(|p| (p.id, p.path)).collect();
        Ok((d, c, photos))
    }
    fn set_lightroom(c: &Catalog, id: i64, text: &str) -> Result<()> {
        c.db.execute(
            "UPDATE photos SET lightroom_develop=? WHERE id=?",
            rusqlite::params![text, id],
        )?;
        Ok(())
    }

    #[test]
    fn a_saved_edit_comes_first_then_lightroom_then_the_defaults() -> Result<()> {
        let (_d, c, photos) = catalog()?;
        let metadata = crate::raw::Raw::open(&photos[0].1)?.metadata;
        let (profiles, _) = crate::camera_profiles::installed(&metadata);
        let defaults = crate::develop::defaults::brighter_defaults();
        let resolve_photo = |(id, path): &(i64, std::path::PathBuf)| {
            resolve(&c.edit_record(*id)?, path, &metadata, &profiles, &defaults)
        };
        // Saved, masks included: they are stored apart from the recipe.
        let mut saved = Recipe::with_profiles(&metadata, &profiles);
        saved.exposure = 0.4;
        saved.masks.push(masks::MaskGroup {
            components: vec![masks::MaskComponent::new(masks::MaskShape::Radial {
                center: [0.5, 0.5],
                radii: [0.2, 0.1],
                angle: 0.,
                feather: 0.5,
            })],
            adjust: masks::LocalAdjust {
                shadows: 0.5,
                ..Default::default()
            },
            ..Default::default()
        });
        let export = ExportOptions {
            quality: 80,
            max_edge: 0,
        };
        let (a, b, unedited) = (&photos[0], &photos[1], &photos[2]);
        c.save_edit(
            a.0,
            &a.1,
            &saved,
            &export,
            super::super::HistoryUpdate::Keep,
        )?;
        // A Lightroom edit under it changes nothing.
        set_lightroom(&c, a.0, "s = { Exposure2012 = 0.25 }")?;
        let resolved = resolve_photo(a)?;
        assert_eq!(resolved.origin, Origin::Saved);
        assert_eq!(resolved.recipe, saved);
        assert_eq!(resolved.export.quality, 80);
        assert_eq!(resolved.recipe, c.load_edit(a.0, &a.1)?.unwrap().recipe);

        let text = "s = { Exposure2012 = 0.25 }";
        set_lightroom(&c, b.0, text)?;
        let resolved = resolve_photo(b)?;
        assert_eq!(resolved.origin, Origin::Lightroom);
        // From Adobe Default, as Lightroom stores it, whatever the raw defaults.
        assert_eq!(
            resolved.recipe,
            crate::lr_develop::convert_develop(text, &metadata, &profiles, None)?.0
        );
        assert_eq!(resolved.recipe.exposure, 0.25);

        let resolved = resolve_photo(unedited)?;
        assert_eq!(resolved.origin, Origin::Defaults);
        assert_eq!(
            resolved.recipe,
            defaults.resolve(&metadata, &profiles).recipe
        );
        assert_eq!(resolved.recipe.exposure, 0.7);
        // Empty Lightroom settings are none.
        set_lightroom(&c, unedited.0, "")?;
        assert_eq!(resolve_photo(unedited)?.origin, Origin::Defaults);
        Ok(())
    }

    #[test]
    fn an_edit_that_cant_be_used_is_an_error_never_the_defaults() -> Result<()> {
        let (_d, c, photos) = catalog()?;
        let metadata = crate::raw::Raw::open(&photos[0].1)?.metadata;
        let (profiles, _) = crate::camera_profiles::installed(&metadata);
        let defaults = DevelopDefaults::default();
        let resolve_photo = |(id, path): &(i64, std::path::PathBuf)| {
            c.edit_record(*id)
                .and_then(|record| resolve(&record, path, &metadata, &profiles, &defaults))
                .map(|r| r.origin)
                .map_err(|e| format!("{e:#}"))
        };
        let (changed, unreadable, lightroom) = (&photos[0], &photos[1], &photos[2]);
        let edit = Recipe::with_profiles(&metadata, &profiles);
        c.save_edit(
            changed.0,
            &changed.1,
            &edit,
            &ExportOptions::default(),
            super::super::HistoryUpdate::Keep,
        )?;
        // The file it was saved for was replaced: the edit is protected.
        let mut bytes = std::fs::read(&changed.1)?;
        bytes.extend_from_slice(b"changed");
        std::fs::write(&changed.1, bytes)?;
        let error = resolve_photo(changed).unwrap_err();
        assert!(error.contains("protected"), "{error}");

        c.save_edit(
            unreadable.0,
            &unreadable.1,
            &edit,
            &ExportOptions::default(),
            super::super::HistoryUpdate::Keep,
        )?;
        c.db.execute("UPDATE photos SET recipe='{' WHERE id=?", [unreadable.0])?;
        assert!(resolve_photo(unreadable).is_err());

        // Settings cut off mid-value.
        set_lightroom(&c, lightroom.0, "s = { Exposure2012 = ")?;
        let error = resolve_photo(lightroom).unwrap_err();
        assert!(error.contains("Lightroom edit can't be read"), "{error}");
        Ok(())
    }
}
