//! Descriptive metadata read from XMP sidecars and from the XMP inside JPEG
//! and TIFF files: when a folder is added, for its new photos, and on Read
//! Metadata from Files, for photos already in the catalog. Never at render
//! time; the catalog stays the source of truth.
use super::Catalog;
#[cfg(test)]
use super::{LangAlt, TextField, Value};
use crate::xmp::descriptive::{self, Read};
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::path::{Path, PathBuf};

/// What reading sidecars found besides the metadata.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidecarReport {
    /// Files that could not be read, with the reason.
    pub unreadable: Vec<(PathBuf, String)>,
    /// A second sidecar of a RAW, unused because the first one wins.
    pub ignored: Vec<PathBuf>,
}
impl SidecarReport {
    fn add(&mut self, other: SidecarReport) {
        self.unreadable.extend(other.unreadable);
        self.ignored.extend(other.ignored);
    }
    /// "2 sidecars could not be read", or nothing to say.
    pub fn summary(&self) -> Option<String> {
        let plural = |n: usize| if n == 1 { "sidecar" } else { "sidecars" };
        let mut parts = Vec::new();
        let n = self.unreadable.len();
        if n > 0 {
            parts.push(format!("{n} {} could not be read", plural(n)));
        }
        let n = self.ignored.len();
        if n > 0 {
            parts.push(format!("{n} {} ignored", plural(n)));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
    /// Each unreadable file and why, and each ignored sidecar, one per line.
    pub fn details(&self) -> String {
        self.unreadable
            .iter()
            .map(|(path, why)| format!("{}: {why}", path.display()))
            .chain(
                self.ignored
                    .iter()
                    .map(|p| format!("{}: ignored, another sidecar is used", p.display())),
            )
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// `path` with `extension` added, in lower or upper case, if it exists.
fn existing(base: &Path, suffix: &str) -> Option<PathBuf> {
    [suffix.to_ascii_lowercase(), suffix.to_ascii_uppercase()]
        .into_iter()
        .map(|s| {
            let mut name = base.as_os_str().to_owned();
            name.push(s);
            PathBuf::from(name)
        })
        .find(|p| p.is_file())
}

/// The sidecars of `file`, the one used first. A RAW's are digiKam's
/// "IMG_1234.NEF.xmp", then "IMG_1234.xmp"; a JPEG or TIFF has only
/// "IMG_1234.JPG.xmp", as "IMG_1234.xmp" beside a RAW of the same name is the
/// RAW's.
pub fn sidecars(file: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = existing(file, ".xmp").into_iter().collect();
    if crate::storage::is_raw(file) {
        found.extend(existing(&file.with_extension(""), ".xmp"));
    }
    found.dedup();
    found
}

/// The XMP inside a JPEG (its standard APP1 packet) or TIFF (tag 700).
fn embedded(file: &Path) -> Result<Option<String>> {
    let extension = file
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "jpg" | "jpeg" => {
            use std::io::{Read as _, Seek, SeekFrom};
            const HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
            // Segment by segment up to the image data, not the whole file.
            let mut f = std::fs::File::open(file)?;
            let mut marker = [0u8; 4];
            f.read_exact(&mut marker[..2])?;
            if marker[..2] != [0xff, 0xd8] {
                return Ok(None);
            }
            let mut byte = [0u8; 1];
            loop {
                // A marker: 0xff, any fill bytes of 0xff, then its code.
                if f.read_exact(&mut byte).is_err() || byte[0] != 0xff {
                    break;
                }
                while byte[0] == 0xff {
                    if f.read_exact(&mut byte).is_err() {
                        return Ok(None);
                    }
                }
                if byte[0] == 0xda || byte[0] == 0xd9 {
                    break;
                }
                // Markers without a length: TEM and the restarts.
                if byte[0] == 0x01 || (0xd0..=0xd7).contains(&byte[0]) {
                    continue;
                }
                let mut length = [0u8; 2];
                if f.read_exact(&mut length).is_err() {
                    break;
                }
                marker = [0xff, byte[0], length[0], length[1]];
                let len = u16::from_be_bytes(length) as usize;
                let body = len.saturating_sub(2);
                if marker[1] == 0xe1 && body > HEADER.len() {
                    let mut data = vec![0u8; body];
                    f.read_exact(&mut data)?;
                    if let Some(xmp) = data.strip_prefix(HEADER) {
                        return Ok(Some(
                            String::from_utf8(xmp.to_vec())
                                .context("embedded XMP is not valid UTF-8")?,
                        ));
                    }
                } else {
                    f.seek(SeekFrom::Current(body as i64))?;
                }
            }
            Ok(None)
        }
        // A DNG keeps its XMP in the same TIFF tag.
        "tif" | "tiff" | "dng" => {
            let Some(mut t) = crate::tiff::Tiff::open(std::fs::File::open(file)?, 0) else {
                return Ok(None);
            };
            let Some(main) = t.ifd(t.first) else {
                return Ok(None);
            };
            Ok(main
                .get(&700)
                .and_then(|e| t.raw(e))
                .map(String::from_utf8)
                .transpose()
                .context("embedded XMP is not valid UTF-8")?)
        }
        _ => Ok(None),
    }
}

/// A sidecar's text: UTF-8, or UTF-16 by its byte order mark, as XML allows.
fn decode(bytes: &[u8]) -> Result<String> {
    let utf16 = |big: bool| -> Result<String> {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if big {
                    u16::from_be_bytes(*c)
                } else {
                    u16::from_le_bytes(*c)
                }
            })
            .collect();
        Ok(String::from_utf16(&units)?)
    };
    match bytes {
        [0xfe, 0xff, ..] => utf16(true),
        [0xff, 0xfe, ..] => utf16(false),
        _ => Ok(
            std::str::from_utf8(bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes))?.to_string(),
        ),
    }
}

/// The metadata of `file`'s sidecar and, for a JPEG or TIFF, its own XMP:
/// field by field the sidecar's where it has one, an explicitly empty value
/// included, else the file's. `None` when neither has any.
pub fn read_file(file: &Path) -> (Option<Read>, SidecarReport) {
    let mut report = SidecarReport::default();
    let mut found = sidecars(file).into_iter();
    let first = found.next();
    report.ignored.extend(found);
    let sidecar = first.and_then(|path| {
        // Larger than any sidecar: not read into memory.
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        match (if size > 16_000_000 {
            Err(anyhow::anyhow!("too large for a sidecar"))
        } else {
            std::fs::read(&path).context("not readable")
        })
        .and_then(|bytes| descriptive::read(&decode(&bytes)?))
        {
            Ok(read) => Some(read),
            Err(e) => {
                report.unreadable.push((path, format!("{e:#}")));
                None
            }
        }
    });
    let inside = match embedded(file).and_then(|x| x.map(|t| descriptive::read(&t)).transpose()) {
        Ok(read) => read,
        Err(e) => {
            report
                .unreadable
                .push((file.to_path_buf(), format!("{e:#}")));
            None
        }
    };
    let read = match (sidecar, inside) {
        (Some(s), Some(i)) => Some(s.or(i)),
        (s, i) => s.or(i),
    };
    (read, report)
}

/// Writes what was read into photo `id`'s rows, in `db`'s transaction:
/// every field the file has with `overwrite`, else only fields the photo
/// has no row for. Keywords are added, or with `overwrite` replace the
/// photo's; rating, label and flag are set where the file has them.
pub(super) fn apply(db: &Connection, id: i64, read: &Read, overwrite: bool) -> Result<()> {
    let mut d = super::descriptive::read(db, id)?;
    fn put<T: Clone>(slot: &mut Option<T>, value: &Option<T>, overwrite: bool) {
        if value.is_some() && (overwrite || slot.is_none()) {
            slot.clone_from(value);
        }
    }
    put(&mut d.title, &read.title, overwrite);
    put(&mut d.caption, &read.caption, overwrite);
    put(&mut d.copyright, &read.copyright, overwrite);
    put(&mut d.creator, &read.creator, overwrite);
    put(&mut d.capture, &read.capture, overwrite);
    put(&mut d.location, &read.location, overwrite);
    super::descriptive::write(db, id, &d)?;
    if let Some(paths) = &read.keywords {
        if overwrite {
            db.execute("DELETE FROM photo_keywords WHERE photo=?", [id])?;
        }
        for path in paths {
            let keyword = super::descriptive::keyword_at(db, path)?;
            db.execute(
                "INSERT OR IGNORE INTO photo_keywords(photo, keyword) VALUES (?, ?)",
                [id, keyword],
            )?;
        }
    }
    if let Some(rating) = read.rating {
        db.execute("UPDATE photos SET rating=? WHERE id=?", params![rating, id])?;
    }
    if let Some(label) = &read.label {
        db.execute("UPDATE photos SET label=? WHERE id=?", params![label, id])?;
    }
    if let Some(flag) = read.flag {
        db.execute("UPDATE photos SET flag=? WHERE id=?", params![flag, id])?;
    }
    Ok(())
}

impl Catalog {
    /// Lightroom's Read Metadata from Files, for the masters of `ids`: every
    /// field a photo's sidecar or embedded XMP has replaces the catalog's,
    /// edits included; fields the files lack are left alone. One
    /// transaction. Virtual copies are never read.
    pub fn read_metadata_from_files(&mut self, ids: &[i64]) -> Result<SidecarReport> {
        let photos: Vec<(i64, PathBuf)> = self
            .photos()?
            .into_iter()
            .filter(|p| ids.contains(&p.id) && p.master.is_none())
            .map(|p| (p.id, p.path))
            .collect();
        let mut report = SidecarReport::default();
        let reads: Vec<(i64, PathBuf, Read)> = photos
            .into_iter()
            .filter_map(|(id, path)| {
                let (read, found) = read_file(&path);
                report.add(found);
                read.map(|r| (id, path, r))
            })
            .collect();
        report.add(self.apply_file_metadata(&reads, true)?);
        Ok(report)
    }
    /// Writes what was read from files, in one transaction: with
    /// `overwrite` every field a file has, else only fields a photo has no
    /// value for. A photo whose values can't be written (an impossible
    /// date) is left as it was and reported with its file.
    pub fn apply_file_metadata(
        &mut self,
        reads: &[(i64, PathBuf, Read)],
        overwrite: bool,
    ) -> Result<SidecarReport> {
        let mut report = SidecarReport::default();
        let mut tx = self.db.transaction()?;
        for (id, path, read) in reads {
            let sp = tx.savepoint()?;
            match apply(&sp, *id, read, overwrite) {
                Ok(()) => sp.commit()?,
                Err(e) => report.unreadable.push((path.clone(), format!("{e:#}"))),
            }
        }
        tx.commit()?;
        Ok(report)
    }
    /// Reads the metadata of photos just added from a folder.
    pub(super) fn import_file_metadata(
        &mut self,
        added: &[(i64, PathBuf)],
    ) -> Result<SidecarReport> {
        let mut report = SidecarReport::default();
        let reads: Vec<(i64, PathBuf, Read)> = added
            .iter()
            .filter_map(|(id, path)| {
                let (read, found) = read_file(path);
                report.add(found);
                read.map(|r| (*id, path.clone(), r))
            })
            .collect();
        report.add(self.apply_file_metadata(&reads, false)?);
        Ok(report)
    }
}

/// Title, caption, creator, copyright, capture time and location from the XMP
/// a Lightroom catalog keeps per photo (attached as `lr`), for the photos
/// that have no row yet. Keywords, rating, label and flag come from
/// Lightroom's own tables. Returns the photos read.
pub(super) fn copy_lightroom_metadata(db: &Connection) -> Result<usize> {
    let has: bool = db
        .query_row(
            "SELECT count(*) FROM lr.sqlite_master WHERE type='table' AND name='Adobe_AdditionalMetadata'",
            [],
            |r| r.get::<_, i64>(0),
        )?
        > 0;
    if !has {
        return Ok(0);
    }
    let rows: Vec<(i64, String)> = db
        .prepare(
            // Only photos that are still that Lightroom image: a photo
            // added since may have taken a removed copy's id.
            "SELECT m.image, m.xmp FROM lr.Adobe_AdditionalMetadata m
             JOIN photos p ON p.id = m.image
             JOIN lr.Adobe_images i ON i.id_local = m.image
             JOIN lr.AgLibraryFile f ON f.id_local = i.rootFile
             JOIN lr.AgLibraryFolder d ON d.id_local = f.folder
             JOIN lr.AgLibraryRootFolder r ON r.id_local = d.rootFolder
             WHERE p.original_path = r.absolutePath || d.pathFromRoot ||
                 CASE WHEN f.idx_filename <> '' THEN f.idx_filename
                      ELSE f.baseName || '.' || f.extension END",
        )?
        .query_map([], |r| {
            let xmp = match r.get_ref(1)? {
                rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => {
                    // Invalid text is left out rather than imported mangled.
                    String::from_utf8(t.to_vec()).unwrap_or_default()
                }
                _ => String::new(),
            };
            Ok((r.get(0)?, xmp))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut read = 0;
    for (id, xmp) in rows {
        // A packet that can't be read is left out, as Lightroom's own data.
        let Ok(found) = descriptive::read(&xmp) else {
            continue;
        };
        let descriptive_only = Read {
            keywords: None,
            rating: None,
            label: None,
            flag: None,
            ..found
        };
        apply(db, id, &descriptive_only, false)?;
        read += 1;
    }
    Ok(read)
}

/// Set in `meta` once descriptive metadata has been copied from the stored
/// Lightroom catalog.
pub(super) const METADATA_BACKFILLED: &str = "lightroom_metadata_backfilled";

impl Catalog {
    /// Catalogs imported before descriptive metadata was kept still hold
    /// the original Lightroom catalog; copy it once, for photos with no row.
    pub fn backfill_lightroom_metadata(&mut self) -> Result<usize> {
        if self.meta(METADATA_BACKFILLED)?.is_some() {
            return Ok(0);
        }
        let copied = self
            .with_stored_lightroom(|db| {
                let tx = db.unchecked_transaction()?;
                let copied = copy_lightroom_metadata(&tx)?;
                tx.commit()?;
                Ok(copied)
            })?
            .unwrap_or(0);
        self.set_meta(METADATA_BACKFILLED, "1")?;
        Ok(copied)
    }
}

#[cfg(test)]
#[path = "sidecar_tests.rs"]
mod tests;
