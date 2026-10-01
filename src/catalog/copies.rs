//! Lightroom's virtual copies: photos of the same file with their own edit,
//! metadata and name.
use super::Catalog;
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};

impl Catalog {
    /// Lightroom's Create Virtual Copy: a new photo of the same file with the
    /// edit, rating, flag, label and keywords of `id`, named "Copy N" after
    /// its master's other copies. Returns the copy's id.
    pub fn create_virtual_copy(&mut self, id: i64) -> Result<i64> {
        let master: i64 = self
            .db
            .query_row(
                "SELECT COALESCE(master_id, id) FROM photos WHERE id=?",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .context("Unknown photo")?;
        let name = self.unused_copy_name(master)?;
        let tx = self.db.transaction()?;
        tx.execute(
            "INSERT INTO photos(folder,filename,original_path,captured,rating,flag,label,format,
                copy_name,master_id,orientation,lightroom_develop,recipe,export_options,identity,edited_at)
             SELECT folder,filename,original_path,captured,rating,flag,label,format,
                ?,?,orientation,lightroom_develop,recipe,export_options,identity,edited_at
             FROM photos WHERE id=?",
            params![name, master, id],
        )?;
        let copy = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO local_edits(photo,data) SELECT ?,data FROM local_edits WHERE photo=?",
            [copy, id],
        )?;
        tx.execute(
            "INSERT INTO photo_keywords(photo,keyword) SELECT ?,keyword FROM photo_keywords WHERE photo=?",
            [copy, id],
        )?;
        tx.commit()?;
        Ok(copy)
    }
    /// The first "Copy N" none of `master`'s copies is named.
    fn unused_copy_name(&self, master: i64) -> Result<String> {
        let names: std::collections::HashSet<String> = self
            .db
            .prepare("SELECT copy_name FROM photos WHERE master_id=?")?
            .query_map([master], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok((1..)
            .map(|n| format!("Copy {n}"))
            .find(|name| !names.contains(name))
            .unwrap())
    }
    fn master_of(&self, id: i64) -> Result<Option<i64>> {
        self.db
            .query_row("SELECT master_id FROM photos WHERE id=?", [id], |r| {
                r.get(0)
            })
            .optional()?
            .context("Unknown photo")
    }
    /// Lightroom's Set Copy as Master: the copy becomes the master, and the
    /// former master and the other copies become its copies.
    pub fn set_copy_as_master(&mut self, id: i64) -> Result<()> {
        let master = self
            .master_of(id)?
            .context("This photo is already the master")?;
        let name: String =
            self.db
                .query_row("SELECT copy_name FROM photos WHERE id=?", [id], |r| {
                    r.get(0)
                })?;
        let name = if name.is_empty() {
            self.unused_copy_name(master)?
        } else {
            name
        };
        let tx = self.db.transaction()?;
        tx.execute(
            "UPDATE photos SET master_id=?1 WHERE master_id=?2 AND id<>?1",
            [id, master],
        )?;
        tx.execute(
            "UPDATE photos SET master_id=?, copy_name=? WHERE id=?",
            params![id, name, master],
        )?;
        tx.execute(
            "UPDATE photos SET master_id=NULL, copy_name='' WHERE id=?",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn set_copy_name(&self, id: i64, name: &str) -> Result<()> {
        ensure!(
            self.master_of(id)?.is_some(),
            "Only virtual copies have a copy name"
        );
        self.db.execute(
            "UPDATE photos SET copy_name=? WHERE id=?",
            params![name.trim(), id],
        )?;
        Ok(())
    }
    /// Removes a virtual copy, with its edit and metadata, from the catalog.
    /// The file and the other photos of it are untouched.
    pub fn remove_virtual_copy(&mut self, id: i64) -> Result<()> {
        ensure!(
            self.master_of(id)?.is_some(),
            "Only virtual copies can be removed"
        );
        let tx = self.db.transaction()?;
        for table in [
            "local_edits",
            "lightroom_history",
            "photo_keywords",
            "collection_photos",
        ] {
            tx.execute(&format!("DELETE FROM {table} WHERE photo=?"), [id])?;
        }
        tx.execute("DELETE FROM photos WHERE id=?", [id])?;
        tx.commit()?;
        Ok(())
    }
}
