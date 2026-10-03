//! Adding a folder of photos to the catalog, with the edits they got from
//! releases that saved them beside the photo.
use super::Catalog;
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use std::path::{Path, PathBuf};

impl Catalog {
    pub fn add_folder(&mut self, folder: &Path) -> Result<usize> {
        Ok(self.add_folder_with(folder, &Default::default())?.0)
    }
    /// Adds a folder's new photos with the metadata of their XMP sidecars and
    /// of the XMP inside JPEGs and TIFFs, then the default Creator and
    /// Copyright where neither the file nor its sidecar has one. Photos
    /// already in the catalog are left alone; Read Metadata from Files reads
    /// theirs. Returns the photos added and what reading the sidecars found.
    pub fn add_folder_with(
        &mut self,
        folder: &Path,
        defaults: &super::MetadataDefaults,
    ) -> Result<(usize, super::SidecarReport)> {
        let folder = folder.canonicalize()?;
        let mut files = Vec::new();
        fn walk(p: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
            for entry in std::fs::read_dir(p)? {
                let e = entry?;
                let t = e.file_type()?;
                if t.is_symlink() || crate::storage::is_hidden(&e.path()) {
                    continue;
                }
                if t.is_dir() {
                    walk(&e.path(), files)?
                } else if crate::storage::is_raw(&e.path())
                    || e.path().extension().is_some_and(|x| {
                        matches!(
                            x.to_string_lossy().to_ascii_lowercase().as_str(),
                            "jpg" | "jpeg" | "png" | "tif" | "tiff"
                        )
                    })
                {
                    files.push(e.path());
                }
            }
            Ok(())
        }
        walk(&folder, &mut files)?;
        let existing_paths: std::collections::HashSet<_> =
            self.photos()?.into_iter().map(|p| p.path).collect();
        let tx = self.db.transaction()?;
        let root: Option<i64> = tx
            .query_row(
                "SELECT id FROM roots WHERE original_path=?",
                [folder.to_string_lossy()],
                |r| r.get(0),
            )
            .optional()?;
        let root = match root {
            Some(root) => root,
            None => {
                tx.execute(
                    "INSERT INTO roots(original_path) VALUES(?)",
                    [folder.to_string_lossy()],
                )?;
                tx.last_insert_rowid()
            }
        };
        let mut added = Vec::new();
        for file in files {
            if existing_paths.contains(&file) {
                continue;
            }
            if tx
                .query_row(
                    "SELECT 1 FROM photos WHERE original_path=?",
                    [file.to_string_lossy()],
                    |r| r.get::<_, i32>(0),
                )
                .optional()?
                .is_some()
            {
                continue;
            }
            let relative = file
                .parent()
                .unwrap()
                .strip_prefix(&folder)?
                .to_string_lossy();
            let existing = tx
                .query_row(
                    "SELECT id FROM folders WHERE root=? AND relative_path=?",
                    params![root, relative],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?;
            let fid = if let Some(id) = existing {
                id
            } else {
                tx.execute(
                    "INSERT INTO folders(root,relative_path) VALUES(?,?)",
                    params![root, relative],
                )?;
                tx.last_insert_rowid()
            };
            tx.execute(
                "INSERT INTO photos(folder,filename,original_path,format) VALUES(?,?,?,?)",
                params![
                    fid,
                    file.file_name().unwrap().to_string_lossy(),
                    file.to_string_lossy(),
                    file.extension()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_ascii_uppercase()
                ],
            )?;
            added.push((tx.last_insert_rowid(), file));
        }
        tx.commit()?;
        for (id, file) in &added {
            if crate::storage::is_raw(file) {
                // A sidecar that no longer matches its photo stays unused on disk.
                let _ = self.import_sidecar(*id, file);
            }
        }
        let report = self.import_file_metadata(&added)?;
        // A photo whose metadata couldn't be read may have its own: no
        // default goes in its place.
        let read: Vec<(i64, PathBuf)> = added
            .iter()
            .filter(|(_, file)| {
                let own = super::sidecars(file);
                !report
                    .unreadable
                    .iter()
                    .any(|(path, _)| path == file || own.contains(path))
            })
            .cloned()
            .collect();
        self.apply_defaults(&read, defaults)?;
        Ok((added.len(), report))
    }
    /// Records capture times read from the photos' files, in one transaction.
    /// Only empty dates are filled, never one Lightroom or the user set, and a
    /// photo's virtual copies get its date too.
    pub fn fill_capture_times(&mut self, times: &[(i64, String)]) -> Result<()> {
        let tx = self.db.transaction()?;
        {
            // Two statements, each on an index, rather than one OR that scans.
            let mut photo =
                tx.prepare("UPDATE photos SET captured=?1 WHERE id=?2 AND captured=''")?;
            let mut copies =
                tx.prepare("UPDATE photos SET captured=?1 WHERE master_id=?2 AND captured=''")?;
            for (id, captured) in times {
                photo.execute(params![captured, id])?;
                copies.execute(params![captured, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
    /// Carries the edit a photo got outside any catalog, in its
    /// photo.rawmakase.json sidecar, into the catalog. The sidecar stays on disk.
    fn import_sidecar(&self, id: i64, file: &Path) -> Result<()> {
        let Some((sidecar, bitmaps)) = crate::storage::import(file)? else {
            return Ok(());
        };
        for bitmap in bitmaps {
            self.put_bitmap(&bitmap)?;
        }
        self.save_edit(
            id,
            file,
            &sidecar.recipe,
            &sidecar.export,
            super::HistoryUpdate::Keep,
        )
    }
}
