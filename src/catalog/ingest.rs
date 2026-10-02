//! Adding a folder of photos to the catalog, with the edits they got from
//! releases that saved them beside the photo.
use super::Catalog;
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use std::path::{Path, PathBuf};

impl Catalog {
    pub fn add_folder(&mut self, folder: &Path) -> Result<usize> {
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
        let root: i64 = tx
            .query_row(
                "SELECT id FROM roots WHERE original_path=?",
                [folder.to_string_lossy()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        let root = if root == 0 {
            tx.execute(
                "INSERT INTO roots(original_path) VALUES(?)",
                [folder.to_string_lossy()],
            )?;
            tx.last_insert_rowid()
        } else {
            root
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
        Ok(added.len())
    }
    /// Records capture times read from the photos' files, in one transaction.
    /// Only empty dates are filled, never one Lightroom or the user set, and a
    /// photo's virtual copies get its date too.
    pub fn fill_capture_times(&mut self, times: &[(i64, String)]) -> Result<()> {
        let tx = self.db.transaction()?;
        for (id, captured) in times {
            tx.execute(
                "UPDATE photos SET captured=?1 WHERE (id=?2 OR master_id=?2) AND captured=''",
                params![captured, id],
            )?;
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
        self.save_edit(id, file, &sidecar.recipe, &sidecar.export)
    }
}
