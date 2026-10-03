//! Title, caption, creator, copyright, location and keyword changes made in
//! the Metadata and Keywording panels: to every photo selected in the Grid,
//! else to the active one. Each is one transaction where it can be, and one
//! command for the shared undo log, which puts back each photo's rows as they
//! were, absent ones included.
use super::{Library, Place};
use crate::catalog::{MetadataSnapshot, TextField};
use anyhow::{Result, ensure};

/// Read Metadata from Files while its files are being read.
pub(super) struct Reread {
    reader: super::background::Reader<(
        Option<crate::catalog::FileMetadata>,
        crate::catalog::SidecarReport,
    )>,
    paths: std::collections::HashMap<i64, std::path::PathBuf>,
    read: Vec<(i64, std::path::PathBuf, crate::catalog::FileMetadata)>,
    report: crate::catalog::SidecarReport,
    place: Place,
}

/// A descriptive metadata change, for the shared undo log.
#[derive(Clone, Debug, PartialEq)]
pub struct DescriptiveCommand {
    /// Orders it among other changes made in the same frame.
    pub sequence: u64,
    pub before: Vec<MetadataSnapshot>,
    pub after: Vec<MetadataSnapshot>,
    pub place_before: Place,
    pub place_after: Place,
    /// Rating, flag and label, for a change that set them too (Read
    /// Metadata from Files); empty otherwise.
    pub ratings_before: Vec<super::Metadata>,
    pub ratings_after: Vec<super::Metadata>,
    /// What changed, as the status line said it.
    pub summary: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DescriptiveEdit {
    /// The default language's text; empty clears the field.
    Text(TextField, String),
    /// In order; none clears the field.
    Creators(Vec<String>),
    ClearLocation,
    /// Keywords by path, top first.
    AddKeywords(Vec<Vec<String>>),
    RemoveKeyword(i64),
}

/// Keywords typed as Lightroom takes them: separated by commas, a child
/// before its parents, "Child < Parent". Returns their paths, top first.
pub fn parse_keywords(text: &str) -> Result<Vec<Vec<String>>> {
    let mut paths = Vec::new();
    for entry in text.split(',') {
        if entry.trim().is_empty() {
            continue;
        }
        let mut path: Vec<String> = entry
            .split('<')
            .map(|name| name.trim().to_string())
            .collect();
        ensure!(
            path.iter().all(|name| !name.is_empty()),
            "A keyword needs a name: \"{}\"",
            entry.trim()
        );
        ensure!(
            path.iter().all(|name| !name.contains('|')),
            "Keywords can't contain \"|\""
        );
        path.reverse();
        paths.push(path);
    }
    Ok(paths)
}

impl Library {
    /// Makes `edit` to the photos given and hands it to the undo log.
    pub(in crate::app) fn edit_descriptive(
        &mut self,
        ids: &[i64],
        edit: DescriptiveEdit,
    ) -> Result<()> {
        self.edit_descriptive_at(ids, edit, None)
    }
    /// `edit_descriptive`, for an edit that belongs to `place`, where it was
    /// typed, rather than where the Library is now; undo returns there.
    pub(super) fn edit_descriptive_at(
        &mut self,
        ids: &[i64],
        edit: DescriptiveEdit,
        place: Option<Place>,
    ) -> Result<()> {
        let ids: Vec<i64> = ids
            .iter()
            .copied()
            .filter(|id| self.photo(*id).is_some())
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let place_before = place.clone().unwrap_or_else(|| self.place());
        let before = self.catalog.metadata_snapshot(&ids)?;
        let what = match &edit {
            DescriptiveEdit::Text(TextField::Title, _) => "Title",
            DescriptiveEdit::Text(TextField::Caption, _) => "Caption",
            DescriptiveEdit::Text(TextField::Copyright, _) => "Copyright",
            DescriptiveEdit::Creators(_) => "Creator",
            DescriptiveEdit::ClearLocation => "Location cleared",
            DescriptiveEdit::AddKeywords(_) => "Keywords added",
            DescriptiveEdit::RemoveKeyword(_) => "Keyword removed",
        };
        let made = match edit {
            DescriptiveEdit::Text(field, text) => self.catalog.set_text(&ids, field, &text),
            DescriptiveEdit::Creators(names) => self.catalog.set_creators(&ids, &names),
            DescriptiveEdit::ClearLocation => self.catalog.clear_location(&ids),
            DescriptiveEdit::AddKeywords(paths) => self.catalog.add_keywords(&ids, &paths),
            DescriptiveEdit::RemoveKeyword(keyword) => self.catalog.remove_keyword(&ids, keyword),
        };
        made?;
        let after = self.catalog.metadata_snapshot(&ids)?;
        if before == after {
            return self.refresh_keywords(&ids);
        }
        let summary = match ids.len() {
            1 => what.to_string(),
            n => format!("{n} photos · {what}"),
        };
        self.message = summary.clone();
        self.descriptive_done.push(DescriptiveCommand {
            sequence: crate::app::undo::sequence(),
            before,
            after,
            place_before,
            place_after: place.unwrap_or_else(|| self.place()),
            ratings_before: Vec::new(),
            ratings_after: Vec::new(),
            summary,
        });
        self.fields.reload();
        // Recorded first: a change saved is one undo can reverse, even if
        // what is shown can't be read again.
        self.refresh_keywords(&ids)
    }
    /// Puts photos' descriptive metadata back, with their rating, flag and
    /// label where the command set them, for undo and redo, without
    /// recording a change of its own.
    pub(in crate::app) fn restore_descriptive(
        &mut self,
        values: &[MetadataSnapshot],
        ratings: &[super::Metadata],
    ) -> Result<()> {
        // A photo removed since (a virtual copy) is left out.
        let values: Vec<MetadataSnapshot> = values
            .iter()
            .filter(|s| self.photo(s.photo).is_some())
            .cloned()
            .collect();
        self.catalog.restore_metadata(&values)?;
        if !ratings.is_empty() {
            self.set_metadata(ratings)?;
        }
        let ids: Vec<i64> = values.iter().map(|s| s.photo).collect();
        self.refresh_photos(&ids)?;
        self.fields.reload();
        Ok(())
    }
    /// Lightroom's Read Metadata from Files, for the masters among `ids`:
    /// the files are read in the background, then what they have replaces
    /// the catalog's values, edits included, as one command.
    pub(in crate::app) fn read_metadata_from_files(&mut self, ids: &[i64]) -> Result<()> {
        let wanted: std::collections::HashSet<i64> = ids.iter().copied().collect();
        let photos: Vec<(i64, std::path::PathBuf)> = self
            .photos
            .iter()
            .filter(|p| wanted.contains(&p.id) && p.master.is_none())
            .map(|p| (p.id, p.path.clone()))
            .collect();
        if photos.is_empty() {
            self.message = "Virtual copies are never read from files".into();
            return Ok(());
        }
        if self.reread.is_some() {
            self.message = "Still reading metadata from files; try again when it's done".into();
            return Ok(());
        }
        let n = photos.len();
        self.message = format!(
            "Reading metadata from {n} {}…",
            if n == 1 { "file" } else { "files" }
        );
        self.reread = Some(Reread {
            paths: photos.iter().cloned().collect(),
            reader: super::background::Reader::start(
                photos,
                &self.ctx,
                crate::catalog::read_file_metadata,
            ),
            read: Vec::new(),
            report: Default::default(),
            place: self.place(),
        });
        Ok(())
    }
    /// Takes in what the files read so far had, and once all are read,
    /// writes it as one command.
    pub(super) fn poll_reread(&mut self) {
        let Some(reread) = &mut self.reread else {
            return;
        };
        let (batch, done) = reread.reader.poll();
        for (id, (read, report)) in batch {
            reread.report.unreadable.extend(report.unreadable);
            reread.report.ignored.extend(report.ignored);
            if let Some(read) = read
                && let Some(path) = reread.paths.get(&id)
            {
                reread.read.push((id, path.clone(), read));
            }
        }
        if !done {
            return;
        }
        let reread = self.reread.take().unwrap();
        if let Err(e) = self.finish_reread(reread) {
            self.message = format!("Metadata not read: {e:#}");
        }
        self.reread_finished = true;
    }
    fn finish_reread(&mut self, reread: Reread) -> Result<()> {
        // Photos removed, or made copies, while their files were read are
        // left out: copies are never read.
        let read: Vec<_> = reread
            .read
            .into_iter()
            .filter(|(id, ..)| self.photo(*id).is_some_and(|p| p.master.is_none()))
            .collect();
        let ids: Vec<i64> = reread
            .paths
            .keys()
            .copied()
            .filter(|id| self.photo(*id).is_some())
            .collect();
        let ratings = |library: &Self| -> Vec<super::Metadata> {
            ids.iter()
                .filter_map(|id| library.photo(*id))
                .map(|p| (p.id, p.rating, p.flag, p.label.clone()))
                .collect()
        };
        let before = self.catalog.metadata_snapshot(&ids)?;
        let ratings_before = ratings(self);
        let mut report = reread.report;
        let written = self.catalog.apply_file_metadata(&read, true)?;
        report.unreadable.extend(written.unreadable);
        self.refresh_photos(&ids)?;
        let after = self.catalog.metadata_snapshot(&ids)?;
        let ratings_after = ratings(self);
        let n = ids.len();
        let mut summary = format!(
            "Read metadata from {n} {}",
            if n == 1 { "file" } else { "files" }
        );
        if let Some(problems) = report.summary() {
            summary.push_str(" · ");
            summary.push_str(&problems);
        }
        self.set_message_with_detail(summary.clone(), report.details());
        self.fields.reload();
        if before != after || ratings_before != ratings_after {
            self.descriptive_done.push(DescriptiveCommand {
                sequence: crate::app::undo::sequence(),
                before,
                after,
                place_before: reread.place.clone(),
                place_after: reread.place,
                ratings_before,
                ratings_after,
                summary,
            });
        }
        Ok(())
    }
    /// Rating, flag, label, capture time and keywords of `ids`, read again
    /// from the catalog, and the photos shown.
    fn refresh_photos(&mut self, ids: &[i64]) -> Result<()> {
        let wanted: std::collections::HashSet<i64> = ids.iter().copied().collect();
        let fresh: std::collections::HashMap<i64, crate::catalog::Photo> = self
            .catalog
            .photos()?
            .into_iter()
            .filter(|p| wanted.contains(&p.id))
            .map(|p| (p.id, p))
            .collect();
        for p in &mut self.photos {
            if let Some(f) = fresh.get(&p.id) {
                p.rating = f.rating;
                p.flag = f.flag;
                p.label = f.label.clone();
                p.captured = f.captured.clone();
            }
        }
        self.sort_keys = None;
        // A capture time read may move the photo, as the catalog sorts.
        self.resort_in_place(|library| {
            library.photos.sort_by(|a, b| {
                (&a.captured, &a.filename, a.id).cmp(&(&b.captured, &b.filename, b.id))
            });
        });
        self.refresh_keywords(ids)
    }
    /// The descriptive changes made since the last call, for the undo log.
    pub(in crate::app) fn take_descriptive_done(&mut self) -> Vec<DescriptiveCommand> {
        std::mem::take(&mut self.descriptive_done)
    }
    /// The keywords shown for `ids`, read again from the catalog, and the
    /// photos shown, which a text filter may pick by them.
    fn refresh_keywords(&mut self, ids: &[i64]) -> Result<()> {
        let mut names = std::collections::HashMap::new();
        for id in ids {
            let keywords: Vec<String> = self
                .catalog
                .keywords(*id)?
                .into_iter()
                .map(|k| k.name)
                .collect();
            names.insert(*id, keywords.join(", "));
        }
        for p in &mut self.photos {
            if let Some(n) = names.remove(&p.id) {
                p.keywords = n;
            }
        }
        self.filter();
        Ok(())
    }
}
