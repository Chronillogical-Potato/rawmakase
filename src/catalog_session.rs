//! The open catalog and what was read from it, apart from the window that
//! shows it: opening needs no UI, and reading it again keeps the lists in step.
use crate::catalog::{Catalog, Collection, CollectionId, Folder, Photo, PhotoId, RootId};
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub struct CatalogSession {
    pub catalog: Catalog,
    /// The catalog's photos, without macOS "._" metadata files.
    pub photos: Vec<Photo>,
    /// Folders, each counting the photos in `photos` it holds.
    pub folders: Vec<Folder>,
    pub collections: Vec<Collection>,
    /// Each collection's photos, limited to the ones in `photos`.
    pub collection_photos: HashMap<CollectionId, HashSet<PhotoId>>,
    pub roots: Vec<(RootId, String, Option<String>)>,
}

/// A catalog just opened, with what opening it could not finish.
pub struct Opened {
    pub session: CatalogSession,
    /// Lightroom's keyword export options could not be read; tried again on
    /// the next open.
    pub keyword_export: Option<anyhow::Error>,
}

impl CatalogSession {
    pub fn open(path: &Path) -> Result<Opened> {
        crate::platform::network::prepare_filesystem_bridge();
        let mut catalog = Catalog::open(path)?;
        // Catalogs imported before history was kept: recover it from the
        // stored Lightroom catalog. Best effort; a failure only hides history.
        let _ = catalog.backfill_lightroom_history();
        let _ = catalog.backfill_lightroom_snapshots();
        let _ = catalog.backfill_lightroom_info();
        let _ = catalog.backfill_lightroom_metadata();
        // Unlike those, a failure here could export keywords Lightroom keeps
        // out, so it is said; it is tried again on the next open.
        let keyword_export = catalog.backfill_keyword_export().err();
        let mut session = Self {
            catalog,
            photos: Vec::new(),
            folders: Vec::new(),
            collections: Vec::new(),
            collection_photos: HashMap::new(),
            roots: Vec::new(),
        };
        session.reload()?;
        Ok(Opened {
            session,
            keyword_export,
        })
    }
    /// Reads the photos, folders, collections and roots again. All or nothing:
    /// when a read fails, the lists stay as they were, still matching each other.
    pub fn reload(&mut self) -> Result<()> {
        // Earlier imports could pick up macOS "._" metadata files; never show them.
        let mut photos = self.catalog.photos()?;
        photos.retain(|p| !crate::storage::is_hidden(Path::new(&p.filename)));
        let mut folders = self.catalog.folders()?;
        for folder in &mut folders {
            folder.count = photos.iter().filter(|p| p.folder == folder.id).count();
        }
        let collections = self.catalog.collections()?;
        let ids: HashSet<PhotoId> = photos.iter().map(|p| p.id).collect();
        let mut collection_photos = self.catalog.collection_photos()?;
        for members in collection_photos.values_mut() {
            members.retain(|id| ids.contains(id));
        }
        let roots = self.catalog.roots()?;
        self.photos = photos;
        self.folders = folders;
        self.collections = collections;
        self.collection_photos = collection_photos;
        self.roots = roots;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_reads_the_catalog_with_no_window() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        image::RgbImage::new(8, 8).save(folder.join("a.jpg"))?;
        image::RgbImage::new(8, 8).save(folder.join("b.jpg"))?;
        let path = directory.path().join("library.rawmakase");
        let mut catalog = Catalog::create(&path)?;
        catalog.add_folder(&folder)?;
        let collection = catalog.quick_collection()?;
        let a = catalog.photos()?[0].id;
        catalog.change_collection(collection, &[a], &[])?;
        drop(catalog);

        let Opened { mut session, .. } = CatalogSession::open(&path)?;
        assert_eq!(session.photos.len(), 2);
        assert_eq!(session.folders.len(), 1);
        assert_eq!(session.folders[0].count, 2);
        assert_eq!(session.collections.len(), 1);
        assert_eq!(session.collection_photos[&collection], HashSet::from([a]));

        // Changes made through the catalog show once it is read again.
        session.catalog.create_virtual_copy(a)?;
        session.catalog.change_collection(collection, &[], &[a])?;
        session.reload()?;
        assert_eq!(session.photos.len(), 3);
        assert_eq!(session.folders[0].count, 3);
        assert!(!session.collection_photos.contains_key(&collection));
        Ok(())
    }

    #[test]
    fn a_reload_that_fails_partway_keeps_the_lists_as_they_were() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let folder = directory.path().join("photos");
        std::fs::create_dir(&folder)?;
        image::RgbImage::new(8, 8).save(folder.join("a.jpg"))?;
        let path = directory.path().join("library.rawmakase");
        Catalog::create(&path)?.add_folder(&folder)?;
        let Opened { mut session, .. } = CatalogSession::open(&path)?;
        let a = session.photos[0].id;

        // The photos read again would include the copy; a later read fails.
        session.catalog.create_virtual_copy(a)?;
        rusqlite::Connection::open(&path)?
            .execute_batch("ALTER TABLE collection_photos RENAME TO collection_photos_gone")?;
        assert!(session.reload().is_err());
        assert_eq!(session.photos.len(), 1);
        assert_eq!(session.folders[0].count, 1);
        Ok(())
    }
}
