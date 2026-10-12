//! A photo family's digest: everything the catalog holds for a master and
//! its virtual copies. Synchronize captures it when it scans and checks it
//! again as it applies, so a family changed meanwhile (by another computer
//! or an older release, which bump no generation) is left alone.
use super::super::PhotoId;
use super::super::db::{Reads, Row, Sql, sql};
use super::super::value::{FromRow, ValueRef};
use anyhow::Result;
use sha2::{Digest as _, Sha256};

/// Every table holding rows of a photo, with the statement that reads them.
/// `family_tables_cover_the_schema` checks it against `schema.sql`.
pub(super) const PHOTO_TABLES: [(&str, Sql); 13] = [
    (
        "photo_keywords",
        sql!("SELECT * FROM photo_keywords WHERE photo=?"),
    ),
    (
        "collection_photos",
        sql!("SELECT * FROM collection_photos WHERE photo=?"),
    ),
    (
        "lightroom_history",
        sql!("SELECT * FROM lightroom_history WHERE photo=?"),
    ),
    (
        "local_edits",
        sql!("SELECT * FROM local_edits WHERE photo=?"),
    ),
    (
        "develop_history",
        sql!("SELECT * FROM develop_history WHERE photo=?"),
    ),
    (
        "develop_snapshots",
        sql!("SELECT * FROM develop_snapshots WHERE photo=?"),
    ),
    ("photo_info", sql!("SELECT * FROM photo_info WHERE photo=?")),
    (
        "photo_checksums",
        sql!("SELECT * FROM photo_checksums WHERE photo=?"),
    ),
    (
        "photo_fields",
        sql!("SELECT * FROM photo_fields WHERE photo=?"),
    ),
    ("photo_text", sql!("SELECT * FROM photo_text WHERE photo=?")),
    (
        "photo_creators",
        sql!("SELECT * FROM photo_creators WHERE photo=?"),
    ),
    (
        "photo_capture",
        sql!("SELECT * FROM photo_capture WHERE photo=?"),
    ),
    (
        "photo_location",
        sql!("SELECT * FROM photo_location WHERE photo=?"),
    ),
];

/// SHA-256 of a family's photos and their rows.
pub type FamilyDigest = [u8; 32];

/// One row, every column with its storage class, as bytes.
struct Encoded(Vec<u8>);
impl FromRow for Encoded {
    fn from_row(row: &Row<'_>) -> Result<Self> {
        let mut out = Vec::new();
        for i in 0..row.len() {
            let value: Stored = row.get(i)?;
            out.extend(value.0);
        }
        Ok(Self(out))
    }
}
/// One column's value, tagged by class and length-prefixed.
struct Stored(Vec<u8>);
impl super::super::value::FromValue for Stored {
    fn from_value(value: ValueRef<'_>) -> Result<Self, super::super::value::ValueError> {
        let (tag, bytes): (u8, Vec<u8>) = match value {
            ValueRef::Null => (0, Vec::new()),
            ValueRef::Integer(i) => (1, i.to_le_bytes().to_vec()),
            ValueRef::Real(f) => (2, f.to_bits().to_le_bytes().to_vec()),
            ValueRef::Text(t) => (3, t.to_vec()),
            ValueRef::Blob(b) => (4, b.to_vec()),
        };
        let mut out = vec![tag];
        out.extend((bytes.len() as u64).to_le_bytes());
        out.extend(bytes);
        Ok(Self(out))
    }
}

/// The digest of the family of `master` as `db` has it now: its photos (the
/// master and every row whose master it is), by id, and for each every row
/// of every table that holds its rows. `None` when `master` is no photo.
pub(super) fn family(db: &impl Reads, master: PhotoId) -> Result<Option<FamilyDigest>> {
    let mut members: Vec<PhotoId> = db.read(
        sql!("SELECT id FROM photos WHERE id=? OR master_id=?"),
        &[&master, &master],
    )?;
    if !members.contains(&master) {
        return Ok(None);
    }
    members.sort();
    let mut sha = Sha256::new();
    for id in &members {
        sha.update(id.0.to_le_bytes());
        let photo: Vec<Encoded> = db.read(sql!("SELECT * FROM photos WHERE id=?"), &[id])?;
        update(&mut sha, "photos", photo);
        for (table, statement) in &PHOTO_TABLES {
            let rows: Vec<Encoded> = db.read(*statement, &[id])?;
            update(&mut sha, table, rows);
        }
    }
    Ok(Some(sha.finalize().into()))
}

/// Adds `rows` of `table` to `sha`, in an order of their own.
fn update(sha: &mut Sha256, table: &str, rows: Vec<Encoded>) {
    let mut rows: Vec<Vec<u8>> = rows.into_iter().map(|r| r.0).collect();
    rows.sort();
    sha.update(table.as_bytes());
    sha.update((rows.len() as u64).to_le_bytes());
    for row in rows {
        sha.update((row.len() as u64).to_le_bytes());
        sha.update(row);
    }
}
