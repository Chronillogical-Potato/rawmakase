//! Descriptive metadata: title, caption, creator, copyright, capture time,
//! location and keywords, as set in RAWmakase or imported. Each is kept per
//! photo, a virtual copy's apart from its master's. A field with no row is the
//! file's own (its EXIF, at export); a cleared one is empty whatever the file
//! says.
use super::Catalog;
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization;

/// The default language of a language alternative.
pub const DEFAULT_LANG: &str = "x-default";

/// The text fields that have languages (XMP language alternatives).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextField {
    Title,
    /// dc:description, EXIF ImageDescription.
    Caption,
    /// dc:rights, EXIF Copyright.
    Copyright,
}
impl TextField {
    pub const ALL: [Self; 3] = [Self::Title, Self::Caption, Self::Copyright];
    fn key(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Caption => "caption",
            Self::Copyright => "copyright",
        }
    }
}
const CREATOR: &str = "creator";

/// A field's override: what RAWmakase has instead of the file's value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value<T> {
    Set(T),
    /// Empty, and the file's value left out.
    Cleared,
}

/// A text field's languages, `x-default` first, as (language, text).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LangAlt(pub Vec<(String, String)>);
impl LangAlt {
    pub fn new(text: &str) -> Self {
        Self(vec![(DEFAULT_LANG.into(), text.into())])
    }
    /// The default language's text, else the first one's.
    pub fn default_text(&self) -> Option<&str> {
        self.0
            .iter()
            .find(|(lang, _)| lang == DEFAULT_LANG)
            .or(self.0.first())
            .map(|(_, text)| text.as_str())
    }
}

/// A capture time from elsewhere than the file.
#[derive(Clone, Debug, PartialEq)]
pub struct Capture {
    /// Local time, "YYYY-MM-DDTHH:MM:SS".
    pub captured: String,
    /// The subsecond digits as read, of any length ("12", "120456").
    pub subsec: Option<String>,
    /// "+02:00", when known.
    pub offset: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Location {
    At {
        lat: f64,
        lon: f64,
        alt: Option<f64>,
    },
    /// The file's GPS left out.
    Cleared,
}

/// A photo's overrides; `None` is no row, the file's own value.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Descriptive {
    pub title: Option<Value<LangAlt>>,
    pub caption: Option<Value<LangAlt>>,
    pub copyright: Option<Value<LangAlt>>,
    /// In order.
    pub creator: Option<Value<Vec<String>>>,
    pub capture: Option<Capture>,
    pub location: Option<Location>,
}
impl Descriptive {
    pub fn text(&self, field: TextField) -> &Option<Value<LangAlt>> {
        match field {
            TextField::Title => &self.title,
            TextField::Caption => &self.caption,
            TextField::Copyright => &self.copyright,
        }
    }
    pub fn text_mut(&mut self, field: TextField) -> &mut Option<Value<LangAlt>> {
        match field {
            TextField::Title => &mut self.title,
            TextField::Caption => &mut self.caption,
            TextField::Copyright => &mut self.copyright,
        }
    }
}

/// A keyword and the names of its ancestors and itself, top first.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Keyword {
    pub id: i64,
    pub name: String,
    pub path: Vec<String>,
}

/// A photo's descriptive metadata and keywords as they were, absent rows
/// included, to put back on undo.
#[derive(Clone, Debug, PartialEq)]
pub struct MetadataSnapshot {
    pub photo: i64,
    pub descriptive: Descriptive,
    pub keywords: Vec<i64>,
}

/// A keyword name as stored and compared: NFC, case kept.
pub fn keyword_name(name: &str) -> String {
    name.trim().nfc().collect()
}

impl Catalog {
    /// A photo's descriptive overrides.
    pub fn descriptive(&self, id: i64) -> Result<Descriptive> {
        read(&self.db, id)
    }
    /// The overrides of several photos, by photo.
    pub fn descriptive_of(&self, ids: &[i64]) -> Result<HashMap<i64, Descriptive>> {
        ids.iter()
            .map(|id| Ok((*id, read(&self.db, *id)?)))
            .collect()
    }
    /// Replaces the default language of a field on every photo given, keeping
    /// its other languages; empty text clears the field, every language.
    /// One transaction.
    pub fn set_text(&mut self, ids: &[i64], field: TextField, text: &str) -> Result<()> {
        let tx = self.db.transaction()?;
        for id in ids {
            let mut d = read(&tx, *id)?;
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
            write(&tx, *id, &d)?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Sets the creators of every photo given, in order; none clears the field.
    pub fn set_creators(&mut self, ids: &[i64], names: &[String]) -> Result<()> {
        let names: Vec<String> = names
            .iter()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        let tx = self.db.transaction()?;
        for id in ids {
            let mut d = read(&tx, *id)?;
            d.creator = Some(if names.is_empty() {
                Value::Cleared
            } else {
                Value::Set(names.clone())
            });
            write(&tx, *id, &d)?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Leaves the location of every photo given out, its file's included.
    pub fn clear_location(&mut self, ids: &[i64]) -> Result<()> {
        let tx = self.db.transaction()?;
        for id in ids {
            let mut d = read(&tx, *id)?;
            d.location = Some(Location::Cleared);
            write(&tx, *id, &d)?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Records a capture time from elsewhere than the file, and sorts the
    /// photo by it.
    pub fn set_capture(&mut self, id: i64, capture: &Capture) -> Result<()> {
        let tx = self.db.transaction()?;
        let mut d = read(&tx, id)?;
        d.capture = Some(capture.clone());
        write(&tx, id, &d)?;
        tx.commit()?;
        Ok(())
    }
    /// What `restore_metadata` needs to put these photos back as they are.
    pub fn metadata_snapshot(&self, ids: &[i64]) -> Result<Vec<MetadataSnapshot>> {
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
                })
            })
            .collect()
    }
    /// Puts photos' descriptive metadata and keywords back as snapshotted,
    /// rows that were absent included, in one transaction.
    pub fn restore_metadata(&mut self, snapshots: &[MetadataSnapshot]) -> Result<()> {
        let tx = self.db.transaction()?;
        for s in snapshots {
            write(&tx, s.photo, &s.descriptive)?;
            tx.execute("DELETE FROM photo_keywords WHERE photo=?", [s.photo])?;
            for keyword in &s.keywords {
                tx.execute(
                    "INSERT OR IGNORE INTO photo_keywords(photo, keyword) VALUES (?, ?)",
                    [s.photo, *keyword],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// A photo's keywords, each with its path from the top, by path.
    pub fn keywords(&self, id: i64) -> Result<Vec<Keyword>> {
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
    pub fn keyword_at(&mut self, path: &[String]) -> Result<i64> {
        ensure!(!path.is_empty(), "A keyword needs a name");
        let tx = self.db.transaction()?;
        let id = keyword_at(&tx, path)?;
        tx.commit()?;
        Ok(id)
    }
    /// Adds a keyword to every photo given.
    pub fn add_keyword(&mut self, ids: &[i64], keyword: i64) -> Result<()> {
        let tx = self.db.transaction()?;
        for id in ids {
            tx.execute(
                "INSERT OR IGNORE INTO photo_keywords(photo, keyword) VALUES (?, ?)",
                [*id, keyword],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Removes a keyword from every photo given that has it.
    pub fn remove_keyword(&mut self, ids: &[i64], keyword: i64) -> Result<()> {
        let tx = self.db.transaction()?;
        for id in ids {
            tx.execute(
                "DELETE FROM photo_keywords WHERE photo=? AND keyword=?",
                [*id, keyword],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
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
    let mut next = Some(id);
    while let Some(k) = next {
        // A cycle in a damaged catalog would never end.
        ensure!(path.len() < 256, "Keyword hierarchy too deep");
        let (name, parent): (String, Option<i64>) = db
            .prepare_cached("SELECT name, parent FROM keywords WHERE id=?")?
            .query_row([k], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
            .context("Unknown keyword")?;
        path.push(name);
        next = parent;
    }
    path.reverse();
    Ok(Keyword {
        id,
        name: path.last().cloned().unwrap_or_default(),
        path,
    })
}

fn read(db: &Connection, id: i64) -> Result<Descriptive> {
    let states: HashMap<String, String> = db
        .prepare_cached("SELECT field, state FROM photo_fields WHERE photo=?")?
        .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut d = Descriptive::default();
    for field in TextField::ALL {
        *d.text_mut(field) = match states.get(field.key()).map(String::as_str) {
            Some("set") => {
                let mut langs: Vec<(String, String)> = db
                    .prepare_cached(
                        "SELECT lang, value FROM photo_text WHERE photo=? AND field=? ORDER BY lang",
                    )?
                    .query_map(params![id, field.key()], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<rusqlite::Result<_>>()?;
                // x-default first.
                langs.sort_by_key(|(lang, _)| lang != DEFAULT_LANG);
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
fn write(db: &Connection, id: i64, d: &Descriptive) -> Result<()> {
    for table in [
        "photo_fields",
        "photo_text",
        "photo_creators",
        "photo_capture",
        "photo_location",
    ] {
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
            Some(Value::Cleared) => state(db, field.key(), false)?,
            Some(Value::Set(langs)) => {
                state(db, field.key(), true)?;
                for (lang, value) in &langs.0 {
                    db.execute(
                        "INSERT OR REPLACE INTO photo_text(photo, field, lang, value) VALUES (?, ?, ?, ?)",
                        params![id, field.key(), lang, value],
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
        let sort = crate::export::exif::lightroom_time(&c.captured, c.subsec.as_deref())
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
pub(super) fn copy_rows(db: &Connection, photo: i64, copy: i64) -> Result<()> {
    for (table, columns) in [
        ("photo_fields", "field, state"),
        ("photo_text", "field, lang, value"),
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
