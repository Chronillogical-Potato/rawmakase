//! What export names photos from: the catalog's part of each photo's
//! `NameFields`, read once when Export is chosen, so the Example and the files
//! named agree whatever changes afterwards.
use super::db::{Reads, sql};
use super::value::row;
use super::{Catalog, PhotoId};
use crate::export_settings::NameFields;
use crate::metadata::{LangAlt, Value};
use anyhow::Result;

impl Catalog {
    /// The name fields of `ids`, in order, read in one transaction. The
    /// camera's make and model are the file's, which the catalog doesn't
    /// keep: they are left unread.
    pub fn name_fields(&self, ids: &[PhotoId]) -> Result<Vec<NameFields>> {
        self.db
            .snapshot(|snapshot| ids.iter().map(|&id| read(snapshot, id)).collect())
    }
}

fn read(db: &impl Reads, id: PhotoId) -> Result<NameFields> {
    row! {
        struct Photo {
            captured: String,
            copy_name: String,
            rating: i32,
            label: String,
        }
    }
    let photo: Photo = db.read_one(
        sql!("SELECT captured, copy_name, rating, label FROM photos WHERE id = ?"),
        &[&id],
    )?;
    let d = super::descriptive::read(db, id)?;
    let text = |value: &Option<Value<LangAlt>>| match value {
        Some(Value::Set(langs)) => langs.default_text().unwrap_or_default().to_string(),
        _ => String::new(),
    };
    Ok(NameFields {
        captured: Some(photo.captured).filter(|c| !c.is_empty()),
        copy_name: photo.copy_name,
        info: super::info::read(db, id)?.unwrap_or_default(),
        title: text(&d.title),
        caption: text(&d.caption),
        copyright: text(&d.copyright),
        creator: match &d.creator {
            Some(Value::Set(names)) => names.join("; "),
            _ => String::new(),
        },
        rating: photo.rating,
        label: photo.label,
        camera: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{PhotoInfo, TextField};

    #[test]
    fn a_photos_name_fields_are_the_catalogs() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let folder = dir.path().join("photos");
        std::fs::create_dir(&folder)?;
        for name in ["a.ARW", "b.ARW"] {
            std::fs::write(folder.join(name), b"synthetic")?;
        }
        let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
        cat.add_folder(&folder)?;
        let ids: Vec<PhotoId> = cat.photos()?.iter().map(|p| p.id).collect();
        cat.fill_capture_times(&[(ids[0], "2026-04-24T18:43:24.000".into())])?;
        cat.set_text(&ids[..1], TextField::Title, "Harbour")?;
        cat.set_creators(&ids[..1], &["Ann".into(), "Bo".into()])?;
        let info = PhotoInfo {
            lens: Some("FE 24-70mm".into()),
            iso: Some(400.),
            dimensions: Some((6000, 4000)),
            ..Default::default()
        };
        cat.fill_photo_info(&[(ids[0], Some(info.clone()))])?;
        cat.db_for_tests().execute(
            "UPDATE photos SET rating = 3, label = 'Red' WHERE id = ?",
            rusqlite::params![ids[0].0],
        )?;
        let copy = cat.create_virtual_copy(ids[0])?;
        let fields = cat.name_fields(&[ids[0], ids[1], copy])?;
        assert_eq!(
            fields[0].captured.as_deref(),
            Some("2026-04-24T18:43:24.000")
        );
        assert_eq!(fields[0].info, info);
        assert_eq!(
            (fields[0].title.as_str(), fields[0].creator.as_str()),
            ("Harbour", "Ann; Bo")
        );
        assert_eq!((fields[0].rating, fields[0].label.as_str()), (3, "Red"));
        assert_eq!(fields[0].camera, Default::default());
        // Nothing known is nothing.
        assert_eq!(fields[1], NameFields::default());
        // A virtual copy: its own name, its master's info.
        assert_eq!(fields[2].copy_name, "Copy 1");
        assert_eq!(fields[2].info, info);
        Ok(())
    }
}
