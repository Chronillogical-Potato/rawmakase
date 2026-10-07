//! Descriptive metadata: title, caption, creator, copyright, capture time,
//! location and keywords, as set in RAWmakase or imported. Each is kept per
//! photo, a virtual copy's apart from its master's. A field with no row is the
//! file's own (its EXIF, at export); a cleared one is empty whatever the file
//! says.
use super::{Catalog, PhotoId};
use crate::metadata::{
    Capture, DEFAULT_LANG, Descriptive, Keyword, LangAlt, Location, TextField, Value, keyword_name,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;

/// The column value a text field is stored under.
fn key(field: TextField) -> &'static str {
    match field {
        TextField::Title => "title",
        TextField::Caption => "caption",
        TextField::Copyright => "copyright",
    }
}
const CREATOR: &str = "creator";

/// A photo's descriptive metadata and keywords as they were, absent rows
/// included, to put back on undo.
#[derive(Clone, Debug, PartialEq)]
pub struct MetadataSnapshot {
    pub photo: PhotoId,
    pub descriptive: Descriptive,
    pub keywords: Vec<i64>,
    /// The capture time it sorts by (`photos.captured`), which a capture
    /// override changes.
    pub captured: String,
}

impl Catalog {
    /// A photo's descriptive overrides.
    pub fn descriptive(&self, id: PhotoId) -> Result<Descriptive> {
        read(&self.db, id)
    }
    /// The overrides of several photos, by photo.
    pub fn descriptive_of(&self, ids: &[PhotoId]) -> Result<HashMap<PhotoId, Descriptive>> {
        ids.iter()
            .map(|id| Ok((*id, read(&self.db, *id)?)))
            .collect()
    }
    /// Replaces the default language of a field on every photo given, keeping
    /// its other languages; empty text clears the field, every language.
    /// One transaction.
    pub fn set_text(&mut self, ids: &[PhotoId], field: TextField, text: &str) -> Result<()> {
        self.update_descriptive(ids, |d| {
            let slot = d.text_mut(field);
            *slot = Some(if text.is_empty() {
                Value::Cleared
            } else {
                let mut langs = match slot.take() {
                    Some(Value::Set(langs)) => langs,
                    _ => LangAlt::default(),
                };
                langs.0.retain(|(lang, _)| lang != DEFAULT_LANG);
                langs.0.insert(0, (DEFAULT_LANG.into(), text.into()));
                Value::Set(langs)
            });
        })
    }
    /// Sets the creators of every photo given, in order; none clears the field.
    pub fn set_creators(&mut self, ids: &[PhotoId], names: &[String]) -> Result<()> {
        let names: Vec<String> = names
            .iter()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        self.update_descriptive(ids, |d| {
            d.creator = Some(if names.is_empty() {
                Value::Cleared
            } else {
                Value::Set(names.clone())
            });
        })
    }
    /// Leaves the location of every photo given out, its file's included.
    pub fn clear_location(&mut self, ids: &[PhotoId]) -> Result<()> {
        self.update_descriptive(ids, |d| d.location = Some(Location::Cleared))
    }
    /// Records a capture time from elsewhere than the file, and sorts the
    /// photo by it.
    #[cfg(test)]
    pub fn set_capture(&mut self, id: PhotoId, capture: &Capture) -> Result<()> {
        self.update_descriptive(&[id], |d| d.capture = Some(capture.clone()))
    }
    /// Reads, changes and writes back the overrides of every photo given, in
    /// one transaction.
    fn update_descriptive(
        &mut self,
        ids: &[PhotoId],
        mut f: impl FnMut(&mut Descriptive),
    ) -> Result<()> {
        self.update_descriptive_where(ids, |_, d| {
            f(d);
            true
        })
    }
    /// `update_descriptive`, given each photo's id, writing back only the
    /// photos `f` returns true for.
    pub(super) fn update_descriptive_where(
        &mut self,
        ids: &[PhotoId],
        mut f: impl FnMut(PhotoId, &mut Descriptive) -> bool,
    ) -> Result<()> {
        let tx = self.db.transaction()?;
        for &id in ids {
            let mut d = read(&tx, id)?;
            if f(id, &mut d) {
                write(&tx, id, &d)?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// What `restore_metadata` needs to put these photos back as they are.
    pub fn metadata_snapshot(&self, ids: &[PhotoId]) -> Result<Vec<MetadataSnapshot>> {
        ids.iter()
            .map(|id| {
                Ok(MetadataSnapshot {
                    photo: *id,
                    descriptive: read(&self.db, *id)?,
                    keywords: self
                        .db
                        .prepare_cached("SELECT keyword FROM photo_keywords WHERE photo=?")?
                        .query_map([id], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?,
                    captured: self
                        .db
                        .prepare_cached("SELECT captured FROM photos WHERE id=?")?
                        .query_row([id], |r| r.get(0))
                        .optional()?
                        .context("Unknown photo")?,
                })
            })
            .collect()
    }
    /// Puts photos' descriptive metadata and keywords back as snapshotted,
    /// rows that were absent included, in one transaction.
    pub fn restore_metadata(&mut self, snapshots: &[MetadataSnapshot]) -> Result<()> {
        let tx = self.db.transaction()?;
        for s in snapshots {
            // The time it sorts by goes back only with its capture override:
            // one filled in from the file since stays.
            let capture_changed = read(&tx, s.photo)?.capture != s.descriptive.capture;
            write(&tx, s.photo, &s.descriptive)?;
            if capture_changed {
                tx.execute(
                    "UPDATE photos SET captured=? WHERE id=?",
                    params![s.captured, s.photo],
                )?;
            }
            tx.execute("DELETE FROM photo_keywords WHERE photo=?", [s.photo])?;
            for keyword in &s.keywords {
                tag_photo(&tx, s.photo, *keyword)?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// A photo's keywords, each with its path from the top, by path.
    pub fn keywords(&self, id: PhotoId) -> Result<Vec<Keyword>> {
        let ids: Vec<i64> = self
            .db
            .prepare_cached("SELECT keyword FROM photo_keywords WHERE photo=?")?
            .query_map([id], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut keywords = ids
            .into_iter()
            .map(|k| keyword(&self.db, k))
            .collect::<Result<Vec<_>>>()?;
        keywords.sort_by(|a, b| a.path.cmp(&b.path).then(a.id.cmp(&b.id)));
        Ok(keywords)
    }
    /// The keyword at `path` (top first), made where it or its parents are
    /// missing. Names are compared in NFC, case kept.
    #[cfg(test)]
    pub fn keyword_at(&mut self, path: &[String]) -> Result<i64> {
        ensure!(!path.is_empty(), "A keyword needs a name");
        let tx = self.db.transaction()?;
        let id = keyword_at(&tx, path)?;
        tx.commit()?;
        Ok(id)
    }
    /// Adds keywords, by path (top first), to every photo given, making
    /// them where missing; one transaction.
    pub fn add_keywords(&mut self, ids: &[PhotoId], paths: &[Vec<String>]) -> Result<()> {
        let tx = self.db.transaction()?;
        for path in paths {
            ensure!(!path.is_empty(), "A keyword needs a name");
            let keyword = keyword_at(&tx, path)?;
            for id in ids {
                tag_photo(&tx, *id, keyword)?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Adds a keyword to every photo given.
    #[cfg(test)]
    pub fn add_keyword(&mut self, ids: &[PhotoId], keyword: i64) -> Result<()> {
        let tx = self.db.transaction()?;
        for id in ids {
            tag_photo(&tx, *id, keyword)?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Removes a keyword from every photo given that has it.
    pub fn remove_keyword(&mut self, ids: &[PhotoId], keyword: i64) -> Result<()> {
        let tx = self.db.transaction()?;
        for id in ids {
            tx.execute(
                "DELETE FROM photo_keywords WHERE photo=? AND keyword=?",
                params![id, keyword],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}

/// Gives `photo` `keyword`, unless it has it already.
pub(super) fn tag_photo(db: &Connection, photo: PhotoId, keyword: i64) -> Result<()> {
    db.execute(
        "INSERT OR IGNORE INTO photo_keywords(photo, keyword) VALUES (?, ?)",
        params![photo, keyword],
    )?;
    Ok(())
}

/// The keyword at `path`, made where missing.
pub(super) fn keyword_at(db: &Connection, path: &[String]) -> Result<i64> {
    let mut parent: Option<i64> = None;
    for name in path {
        let name = keyword_name(name);
        ensure!(!name.is_empty(), "A keyword needs a name");
        // Not unique: the first of any duplicates Lightroom left.
        let candidates: Vec<(i64, String)> = db
            .prepare_cached("SELECT id, name FROM keywords WHERE parent IS ? ORDER BY id")?
            .query_map([parent], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let found = candidates
            .into_iter()
            .find(|(_, n)| keyword_name(n) == name)
            .map(|(id, _)| id);
        parent = Some(match found {
            Some(id) => id,
            None => {
                db.execute(
                    "INSERT INTO keywords(name, parent) VALUES (?, ?)",
                    params![name, parent],
                )?;
                db.last_insert_rowid()
            }
        });
    }
    Ok(parent.unwrap())
}

fn keyword(db: &Connection, id: i64) -> Result<Keyword> {
    let mut path = Vec::new();
    // Include on Export of each, from the keyword up.
    let mut include = Vec::new();
    let mut parents = true;
    let mut next = Some(id);
    while let Some(k) = next {
        // A cycle in a damaged catalog would never end.
        ensure!(path.len() < 256, "Keyword hierarchy too deep");
        let (name, parent, included, with_parents): (String, Option<i64>, bool, bool) = db
            .prepare_cached(
                "SELECT k.name, k.parent, COALESCE(e.include, 1), COALESCE(e.parents, 1)
                 FROM keywords k LEFT JOIN keyword_export e ON e.keyword = k.id WHERE k.id=?",
            )?
            .query_row([k], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .optional()?
            .context("Unknown keyword")?;
        if path.is_empty() {
            parents = with_parents;
        }
        path.push(name);
        include.push(included);
        next = parent;
    }
    let own = include[0];
    let mut exported: Vec<bool> = include
        .iter()
        .enumerate()
        .map(|(i, included)| own && *included && (i == 0 || parents))
        .collect();
    path.reverse();
    exported.reverse();
    Ok(Keyword {
        id,
        name: path.last().cloned().unwrap_or_default(),
        path,
        exported,
    })
}

pub(super) fn read(db: &Connection, id: PhotoId) -> Result<Descriptive> {
    let states: HashMap<String, String> = db
        .prepare_cached("SELECT field, state FROM photo_fields WHERE photo=?")?
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut d = Descriptive::default();
    for field in TextField::ALL {
        *d.text_mut(field) = match states.get(key(field)).map(String::as_str) {
            Some("set") => {
                let langs: Vec<(String, String)> = db
                    .prepare_cached(
                        "SELECT lang, value FROM photo_text WHERE photo=? AND field=? ORDER BY position",
                    )?
                    .query_map(params![id, key(field)], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                Some(Value::Set(LangAlt(langs)))
            }
            Some(_) => Some(Value::Cleared),
            None => None,
        };
    }
    d.creator = match states.get(CREATOR).map(String::as_str) {
        Some("set") => Some(Value::Set(
            db.prepare_cached("SELECT name FROM photo_creators WHERE photo=? ORDER BY position")?
                .query_map([id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?,
        )),
        Some(_) => Some(Value::Cleared),
        None => None,
    };
    d.capture = db
        .prepare_cached("SELECT captured, subsec, offset FROM photo_capture WHERE photo=?")?
        .query_row([id], |r| {
            Ok(Capture {
                captured: r.get(0)?,
                subsec: r.get(1)?,
                offset: r.get(2)?,
            })
        })
        .optional()?;
    d.location = db
        .prepare_cached("SELECT lat, lon, alt, cleared FROM photo_location WHERE photo=?")?
        .query_row([id], |r| {
            let at: (Option<f64>, Option<f64>, Option<f64>, bool) =
                (r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?);
            Ok(at)
        })
        .optional()?
        .map(|(lat, lon, alt, cleared)| match (lat, lon) {
            (Some(lat), Some(lon)) if !cleared => Location::At { lat, lon, alt },
            _ => Location::Cleared,
        });
    Ok(d)
}

/// Replaces every descriptive row of a photo with `d`.
pub(super) fn write(db: &Connection, id: PhotoId, d: &Descriptive) -> Result<()> {
    for table in TABLES {
        db.execute(&format!("DELETE FROM {table} WHERE photo=?"), [id])?;
    }
    let state = |db: &Connection, field: &str, set: bool| -> Result<()> {
        db.execute(
            "INSERT INTO photo_fields(photo, field, state) VALUES (?, ?, ?)",
            params![id, field, if set { "set" } else { "cleared" }],
        )?;
        Ok(())
    };
    for field in TextField::ALL {
        match d.text(field) {
            None => {}
            Some(Value::Cleared) => state(db, key(field), false)?,
            Some(Value::Set(langs)) => {
                state(db, key(field), true)?;
                for (position, (lang, value)) in langs.0.iter().enumerate() {
                    db.execute(
                        "INSERT OR REPLACE INTO photo_text(photo, field, lang, position, value)
                         VALUES (?, ?, ?, ?, ?)",
                        params![id, key(field), lang, position as i64, value],
                    )?;
                }
            }
        }
    }
    match &d.creator {
        None => {}
        Some(Value::Cleared) => state(db, CREATOR, false)?,
        Some(Value::Set(names)) => {
            state(db, CREATOR, true)?;
            for (position, name) in names.iter().enumerate() {
                db.execute(
                    "INSERT INTO photo_creators(photo, position, name) VALUES (?, ?, ?)",
                    params![id, position as i64, name],
                )?;
            }
        }
    }
    if let Some(c) = &d.capture {
        db.execute(
            "INSERT INTO photo_capture(photo, captured, subsec, offset) VALUES (?, ?, ?, ?)",
            params![id, c.captured, c.subsec, c.offset],
        )?;
        let sort = crate::exif::lightroom_time(&c.captured, c.subsec.as_deref())
            .context("Invalid capture time")?;
        db.execute("UPDATE photos SET captured=? WHERE id=?", params![sort, id])?;
    }
    match &d.location {
        None => {}
        Some(Location::Cleared) => {
            db.execute(
                "INSERT INTO photo_location(photo, cleared) VALUES (?, 1)",
                [id],
            )?;
        }
        Some(Location::At { lat, lon, alt }) => {
            db.execute(
                "INSERT INTO photo_location(photo, lat, lon, alt, cleared) VALUES (?, ?, ?, ?, 0)",
                params![id, lat, lon, alt],
            )?;
        }
    }
    Ok(())
}

/// Gives `copy` its own copies of `photo`'s descriptive rows.
pub(super) fn copy_rows(db: &Connection, photo: PhotoId, copy: PhotoId) -> Result<()> {
    for (table, columns) in [
        ("photo_fields", "field, state"),
        ("photo_text", "field, lang, position, value"),
        ("photo_creators", "position, name"),
        ("photo_capture", "captured, subsec, offset"),
        ("photo_location", "lat, lon, alt, cleared"),
    ] {
        db.execute(
            &format!(
                "INSERT INTO {table}(photo, {columns}) SELECT ?, {columns} FROM {table} WHERE photo=?"
            ),
            [copy, photo],
        )?;
    }
    Ok(())
}

/// The descriptive tables, for removing a photo's rows.
pub(super) const TABLES: [&str; 5] = [
    "photo_fields",
    "photo_text",
    "photo_creators",
    "photo_capture",
    "photo_location",
];
