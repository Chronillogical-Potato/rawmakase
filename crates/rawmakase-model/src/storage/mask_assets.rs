//! The raster coverage that bitmap mask components refer to by content ID.
//!
//! Recipes carry only IDs. The pixels live in one process-wide store: a generated
//! selection is registered here the moment it exists and is *unsaved* until a
//! catalog write has stored it; anything else is read through the catalog the app
//! installed as the [`AssetLoader`] the first time it is needed and may be evicted
//! again, so saved history never pins every decoded raster. Rendering, previews and
//! exports all read through [`resolve`], so no job has to carry rasters along.
//!
//! The store knows nothing of catalogs, renderers or inference: the model crate owns
//! only the values and their budget.
use super::bitmaps::Bitmap;
use anyhow::{Result, ensure};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Most pixels along either axis of one raster.
pub const MAX_SIDE: u32 = 4096;
/// Most decoded bytes of one raster (8-bit coverage).
pub const MAX_RASTER_BYTES: usize = 16 << 20;
/// Most decoded bytes the store keeps of rasters that are saved and can be read
/// again. Unsaved rasters are never evicted.
const CACHE_BYTES: usize = 256 << 20;
/// Most decoded bytes of distinct rasters one generation may add to an edit.
pub const EDIT_BYTES: usize = 256 << 20;

/// Why a raster could not be provided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssetError {
    /// Nothing stores a raster with this ID.
    Missing(String),
    /// A raster is stored but is damaged or is not what the recipe describes.
    Corrupt(String, String),
}
impl std::fmt::Display for AssetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(id) => write!(f, "A mask's data ({}) is missing", short(id)),
            Self::Corrupt(id, why) => write!(f, "A mask's data ({}) is damaged: {why}", short(id)),
        }
    }
}
impl std::error::Error for AssetError {}
fn short(id: &str) -> &str {
    &id[..id.len().min(15)]
}

/// Reads rasters that are not in the store, from where the app keeps them.
pub trait AssetLoader: Send + Sync {
    /// The raster stored under `id`, or `None` when nothing is.
    fn load(&self, id: &str) -> Result<Option<Bitmap>, AssetError>;
}

struct Entry {
    raster: Arc<Bitmap>,
    /// The compressed form, kept while the raster is unsaved so each write compresses
    /// it only once.
    blob: Option<Arc<Vec<u8>>>,
    saved: bool,
    used: u64,
}
#[derive(Default)]
struct Store {
    entries: HashMap<String, Entry>,
    /// Readers of saved rasters, the catalog opened last first. Earlier catalogs' readers
    /// stay for a while: an export or preview build started under one keeps resolving
    /// its rasters after the app has moved on to another, and a content ID names the
    /// same pixels wherever they are read from.
    loaders: Vec<Arc<dyn AssetLoader>>,
    clock: u64,
}
/// Readers kept besides the current catalog's.
const KEPT_LOADERS: usize = 4;
fn store() -> &'static Mutex<Store> {
    static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
    STORE.get_or_init(Default::default)
}
fn lock() -> std::sync::MutexGuard<'static, Store> {
    store()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Checks that `bitmap` is a coverage raster within the limits.
pub fn validate_raster(bitmap: &Bitmap) -> Result<()> {
    ensure!(
        bitmap.channels == 1 && bitmap.depth == 1,
        "A mask raster is 8-bit coverage"
    );
    ensure!(
        (1..=MAX_SIDE).contains(&bitmap.width) && (1..=MAX_SIDE).contains(&bitmap.height),
        "A mask raster is at most {MAX_SIDE} pixels along either side"
    );
    ensure!(
        bitmap.data.len() == bitmap.width as usize * bitmap.height as usize
            && bitmap.data.len() <= MAX_RASTER_BYTES,
        "A mask raster has the wrong size"
    );
    Ok(())
}

/// Registers a new raster as unsaved and returns its content ID. Registering the
/// same coverage again returns the same ID.
pub fn register(bitmap: Bitmap) -> Result<String> {
    validate_raster(&bitmap)?;
    let id = bitmap.content_id();
    let blob = Arc::new(bitmap.compress()?);
    let mut store = lock();
    store.clock += 1;
    let used = store.clock;
    let entry = store.entries.entry(id.clone()).or_insert_with(|| Entry {
        raster: Arc::new(bitmap),
        blob: Some(blob),
        saved: false,
        used,
    });
    entry.used = used;
    Ok(id)
}

/// Installs the reader for the catalog just opened; readers of catalogs opened
/// before it are asked after it, for jobs still running under them.
pub fn add_loader(loader: Arc<dyn AssetLoader>) {
    let mut store = lock();
    store.loaders.insert(0, loader);
    store.loaders.truncate(KEPT_LOADERS);
}

/// The raster stored under `id`: from the store, else read through the loader.
/// A raster read must be what its ID names and what a mask describes.
pub fn resolve(id: &str) -> Result<Arc<Bitmap>, AssetError> {
    let loader = {
        let mut store = lock();
        store.clock += 1;
        let used = store.clock;
        if let Some(entry) = store.entries.get_mut(id) {
            entry.used = used;
            return Ok(entry.raster.clone());
        }
        store.loaders.clone()
    };
    // The first reader that has it; a reader that fails does not hide one that has it.
    let mut failure = None;
    let mut found = None;
    for loader in &loader {
        match loader.load(id) {
            Ok(Some(bitmap)) => {
                found = Some(bitmap);
                break;
            }
            Ok(None) => {}
            Err(e) => {
                failure.get_or_insert(e);
            }
        }
    }
    let bitmap = match (found, failure) {
        (Some(bitmap), _) => bitmap,
        (None, Some(e)) => return Err(e),
        (None, None) => return Err(AssetError::Missing(id.to_string())),
    };
    validate_raster(&bitmap).map_err(|e| AssetError::Corrupt(id.to_string(), e.to_string()))?;
    if !bitmap.matches_id(id) {
        return Err(AssetError::Corrupt(
            id.to_string(),
            "its content does not match its ID".into(),
        ));
    }
    let raster = Arc::new(bitmap);
    let mut store = lock();
    store.clock += 1;
    let used = store.clock;
    let entry = store
        .entries
        .entry(id.to_string())
        .or_insert_with(|| Entry {
            raster: raster.clone(),
            blob: None,
            saved: true,
            used,
        });
    let out = entry.raster.clone();
    evict(&mut store);
    Ok(out)
}

/// Makes sure every raster of `refs` (`(id, width, height)` as a mask describes it)
/// can be provided and is the size the mask says: a stored raster of another size is
/// as damaged as a missing one, since the mask would render wrongly placed or empty.
pub fn ensure_shaped<'a>(
    refs: impl IntoIterator<Item = (&'a str, u32, u32)>,
) -> Result<(), AssetError> {
    for (id, width, height) in refs {
        let raster = resolve(id)?;
        if (raster.width, raster.height) != (width, height) {
            return Err(AssetError::Corrupt(
                id.to_string(),
                format!(
                    "it is {}x{} but the mask describes {width}x{height}",
                    raster.width, raster.height
                ),
            ));
        }
    }
    Ok(())
}

/// Forgets an unsaved raster nothing refers to any more (a selection the user did
/// not keep). A saved one stays cached; a raster still referred to must not be passed.
pub fn discard_unsaved(id: &str) {
    let mut store = lock();
    if store.entries.get(id).is_some_and(|e| !e.saved) {
        store.entries.remove(id);
    }
}

/// Whether the raster is in the store without reading anything.
pub fn is_held(id: &str) -> bool {
    lock().entries.contains_key(id)
}

/// Makes sure every raster of `ids` can be provided, reading the ones that are not
/// held. The first failure is returned.
pub fn ensure_all<'a>(ids: impl IntoIterator<Item = &'a str>) -> Result<(), AssetError> {
    ids.into_iter().try_for_each(|id| resolve(id).map(drop))
}

/// The compressed rasters among `ids` that a catalog write must store because they
/// were generated here and not yet saved.
pub fn unsaved<'a>(ids: impl IntoIterator<Item = &'a str>) -> Vec<(String, Arc<Vec<u8>>)> {
    let store = lock();
    ids.into_iter()
        .filter_map(|id| {
            let entry = store.entries.get(id)?;
            let blob = entry.blob.clone().filter(|_| !entry.saved)?;
            Some((id.to_string(), blob))
        })
        .collect()
}

/// Notes that a catalog stored these rasters: they can be evicted and read again.
pub fn mark_saved<'a>(ids: impl IntoIterator<Item = &'a str>) {
    let mut store = lock();
    for id in ids {
        if let Some(entry) = store.entries.get_mut(id) {
            entry.saved = true;
            entry.blob = None;
        }
    }
    evict(&mut store);
}

/// Decoded bytes of the distinct rasters among `ids`.
pub fn decoded_bytes<'a>(ids: impl IntoIterator<Item = &'a str>) -> usize {
    let mut seen = std::collections::HashSet::new();
    let store = lock();
    ids.into_iter()
        .filter(|id| seen.insert(*id))
        .map(|id| store.entries.get(id).map_or(0, |e| e.raster.data.len()))
        .sum()
}

/// Bytes the store holds of saved rasters, which it keeps for reuse.
#[cfg(test)]
fn cached_bytes() -> usize {
    lock()
        .entries
        .values()
        .filter(|e| e.saved)
        .map(|e| e.raster.data.len())
        .sum()
}

/// Drops the least recently used saved rasters beyond the budget.
fn evict(store: &mut Store) {
    let mut total: usize = store
        .entries
        .values()
        .filter(|e| e.saved)
        .map(|e| e.raster.data.len())
        .sum();
    if total <= CACHE_BYTES {
        return;
    }
    let mut saved: Vec<(u64, String)> = store
        .entries
        .iter()
        .filter(|(_, e)| e.saved)
        .map(|(id, e)| (e.used, id.clone()))
        .collect();
    saved.sort();
    for (_, id) in saved {
        if total <= CACHE_BYTES {
            break;
        }
        // A render still holding the raster keeps it alive by its own Arc.
        if let Some(entry) = store.entries.remove(&id) {
            total -= entry.raster.data.len();
        }
    }
}

/// Forgets every raster and the readers (tests only).
#[cfg(any(test, feature = "test-support"))]
pub fn reset() {
    *lock() = Store::default();
}
/// Forgets every raster but keeps the readers (tests only).
#[cfg(test)]
fn reset_cache_only() {
    lock().entries.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn raster(w: u32, h: u32, v: u8) -> Bitmap {
        Bitmap {
            width: w,
            height: h,
            channels: 1,
            depth: 1,
            data: vec![v; (w * h) as usize],
        }
    }
    struct Loader(Mutex<HashMap<String, Bitmap>>, AtomicUsize);
    impl AssetLoader for Loader {
        fn load(&self, id: &str) -> Result<Option<Bitmap>, AssetError> {
            self.1.fetch_add(1, Ordering::Relaxed);
            Ok(self.0.lock().unwrap().get(id).cloned())
        }
    }

    // One test: the store is shared by the process.
    #[test]
    fn the_store_holds_unsaved_rasters_and_reads_saved_ones_through_the_loader() {
        reset();
        let a = register(raster(4, 3, 200)).unwrap();
        assert!(a.starts_with("sha256:") && Bitmap::is_valid_id(&a));
        assert_eq!(register(raster(4, 3, 200)).unwrap(), a);
        assert_ne!(register(raster(3, 4, 200)).unwrap(), a);
        assert_eq!(resolve(&a).unwrap().data, vec![200; 12]);
        // Unsaved rasters are what a catalog write must store; saved ones are not.
        assert_eq!(unsaved([a.as_str()]).len(), 1);
        mark_saved([a.as_str()]);
        assert!(unsaved([a.as_str()]).is_empty());
        assert_eq!(decoded_bytes([a.as_str(), a.as_str()]), 12);
        // Not malformed rasters.
        assert!(
            register(Bitmap {
                channels: 3,
                ..raster(2, 2, 1)
            })
            .is_err()
        );
        assert!(register(raster(MAX_SIDE + 1, 1, 0)).is_err());

        // A raster that is not held is read, checked against its ID and kept.
        let stored = raster(5, 5, 9);
        let id = stored.content_id();
        let loader = Arc::new(Loader(
            Mutex::new(HashMap::from([
                (id.clone(), stored.clone()),
                ("sha256:".to_string() + &"0".repeat(64), stored),
            ])),
            AtomicUsize::new(0),
        ));
        assert_eq!(resolve(&id), Err(AssetError::Missing(id.clone())));
        add_loader(loader.clone());
        assert!(resolve(&id).is_ok() && resolve(&id).is_ok());
        assert_eq!(loader.1.load(Ordering::Relaxed), 1);
        assert_eq!(
            resolve("sha256:ffff").unwrap_err(),
            AssetError::Missing("sha256:ffff".into())
        );
        let wrong = "sha256:".to_string() + &"0".repeat(64);
        assert!(matches!(resolve(&wrong), Err(AssetError::Corrupt(..))));
        assert!(ensure_all([id.as_str(), "sha256:ffff"]).is_err());
        assert_eq!(cached_bytes(), 12 + 25);
        // The size a mask describes must be the raster's.
        assert!(ensure_shaped([(id.as_str(), 5, 5)]).is_ok());
        assert!(matches!(
            ensure_shaped([(id.as_str(), 5, 4)]),
            Err(AssetError::Corrupt(..))
        ));
        // A later catalog's reader comes first; an earlier one still serves its rasters.
        let later = Arc::new(Loader(Mutex::new(HashMap::new()), AtomicUsize::new(0)));
        add_loader(later.clone());
        reset_cache_only();
        assert!(resolve(&id).is_ok());
        assert_eq!(later.1.load(Ordering::Relaxed), 1);
        // An unsaved raster the user did not keep is forgotten; a saved one is kept.
        let unkept = register(raster(2, 2, 77)).unwrap();
        assert!(is_held(&unkept));
        discard_unsaved(&unkept);
        assert!(!is_held(&unkept));
        discard_unsaved(&id);
        assert!(is_held(&id));
        reset();
    }
}
