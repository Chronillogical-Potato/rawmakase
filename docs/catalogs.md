# RAWmakase catalogs and Lightroom import

Use **Catalog → Import Lightroom catalog…**, select a closed `.lrcat`, then choose a new `.rawmakase` filename. Existing destination files are never overwritten. **New catalog** creates an empty library; **Add photo folder** recursively registers ARW, RAF, JPEG, PNG and TIFF files without copying or modifying them.

**Library** (`G`) provides a virtualized thumbnail grid, folder browsing, filename/keyword/date/label search, minimum-rating and flag filters, offline filtering, capture-date sorting, and rating/flag/color-label editing. Double-click an available RAW file (any LibRaw format, see `RAW_EXTENSIONS` in `src/storage/files.rs`) or select it and use **Develop** (`D`). JPEG, PNG and TIFF remain browsable but can't be developed. Library thumbnails are original embedded/file previews, not Lightroom-rendered previews.

**Develop** retains the preset browser and editing controls. When the photo belongs to a catalog, edits and export options save in that catalog rather than beside the photograph. Virtual copies share the source photograph but have independent recipes. Create Virtual Copy (⌘' or the thumbnail menu, in Library and Develop) starts a copy from the photo's current edit, rating, flag, label and keywords, named Copy 1, Copy 2 and so on. The thumbnail menu also offers Set Copy as Master and Remove Virtual Copy (which asks first and leaves the file alone); rename a copy in the Library Metadata panel's Copy Name field. Copies show a folded corner on their thumbnail and their own edited preview. Raw files opened directly continue to use existing JSON sidecars. Opening a catalog photo does not implicitly import a possibly unrelated RAWmakase sidecar.

## Stars, picks and color labels

The selected photo has clickable stars, Pick/Reject/Unflag controls and a color-label menu in both Library and Develop. Grid thumbnails use Lightroom-style gray cells, muted label-colored backgrounds, thin dividers, black photo borders and a lighter selection outline. Stars, pick/reject status and a bordered color swatch appear beneath the photo. Right-click a thumbnail for Set Flag, Set Rating and Set Color Label (all five colors and None). The Library label filter includes imported custom text as well as the five standard colors. Changes save immediately to the RAWmakase catalog, work for offline Library photos, and remain independent for virtual copies. Changing metadata never changes the RAW, its sidecars or the Lightroom catalog.

| Key | Action |
|---|---|
| 1–5 | Set that many stars |
| 0 | Clear stars |
| [ / ] | Decrease / increase stars, bounded to 0–5 |
| P / X / U | Pick / reject / unflag |
| Backtick | Toggle pick / unflag |
| 6 / 7 / 8 / 9 | Toggle Red / Yellow / Green / Blue |
| Shift + rating, flag or color key | Apply and advance in the current filtered order |
| Left / Right | Previous / next photo |
| Z | Toggle Fit / 100% zoom in Develop |

Purple and **No label** are available in the menu. Repeating a color shortcut clears that same label; ratings set an explicit number, with 0 clearing them. Clicking the currently selected star clears the rating. Shortcuts are suppressed while editing text or numbers, during dialogs and during export; modified system shortcuts such as Command+X are not intercepted. `1` now means one star, replacing its previous zoom shortcut.

The importer reads Lightroom `rating`, `pick` (−1 rejected, 0 unflagged, 1 picked), and `colorLabels`. Missing ratings/flags become zero; missing labels become empty. Label text is preserved exactly, including custom/localized names. Standard Lightroom label names map to the corresponding color. Other text displays white and remains searchable/filterable; a custom Lightroom label-set mapping is not reliably available from a standalone `.lrcat`, so RAWmakase does not guess its color. No write-back/synchronization with Lightroom is performed. Metadata undo, multi-selection assignment and Caps Lock auto-advance are not implemented.

Shortcut reference: [Adobe Lightroom Classic keyboard shortcuts](https://helpx.adobe.com/lightroom-classic/desktop/introduction-to-lightroom-classic/keyboard-shortcuts.html).

## Import and preservation

The importer takes a private snapshot of the source, checks its SQLite structure/integrity, and imports in one transaction. It requires a closed/exported catalog without a nonempty WAL/journal; it refuses an active/incomplete copy rather than ignoring pending changes. The current importer accepts catalogs under 2 GB and was exercised on the supplied Lightroom v13 catalog. Incompatible layouts fail without leaving a destination file.

Normalized tables retain images and virtual-copy identities, original paths, folder roots, capture dates, ratings, flags, color labels, collections/memberships, keywords/parents/memberships, serialized Develop settings and Develop history steps (the `lightroom_history` table, shown under **From Lightroom** in the History panel, where each step can be applied). Collections and keywords are stored but not yet usable: the Library has no collection browser, and keywords are shown read-only. A byte-exact copy of the entire source `.lrcat` is stored in `sources.original_catalog`, preserving snapshots, stacks, IPTC, GPS, faces, smart rules and other fields that RAWmakase does not yet interpret. This makes the new catalog larger; it avoids discarding unrecognized Lightroom data. External `.lrcat-data`, preview bundles, originals and profiles are not embedded or synthesized.

Smart collection definitions and any stored membership are preserved; RAWmakase does not evaluate Adobe's smart-collection rule language. Collections marked “smart snapshot” may therefore have no stored members.

## Lightroom rendering

A photo without a RAWmakase edit opens with its Lightroom edit applied, once camera profiles are known. **Apply compatible Lightroom edits** re-applies it as one undoable change using RAWmakase's supported controls. It parses Lightroom's serialized settings as data, never as executable Lua. Lens corrections, Transform and Upright (from Lightroom's stored corrections) apply. It reports missing profiles and unsupported controls, such as AI and color-range masks, Glow, Reshape and profile Amount other than 100. Spot removal and brush, gradient, radial and luminance-range masks convert to RAWmakase's experimental spots and masks. Compatible controls use RAWmakase's algorithms; this is not a Lightroom appearance guarantee. The untouched original settings remain in the database even after further editing. Lightroom orientation metadata is preserved; current previews/Develop use the source camera orientation.

## Relinking offline photos

On Linux, Lightroom's `/Volumes/...` and `/Users/...` paths will often be offline. Click the **…** on a root row, or right-click it and choose **Locate root folder…**, to map a complete source root to its local directory. The **Browse…** button opens the native directory picker for the selected tree folder; choosing a directory applies the mapping, and Cancel leaves it unchanged. After relinking, RAWmakase reports the available photo count. Right-click a folder and choose **Locate this folder…** for a narrower mapping. Descendants inherit a folder mapping; a more specific mapping takes precedence. Paths are changed only in the RAWmakase catalog. Files are never moved, renamed or matched by filename alone.

## SQLite format, version 1

A `.rawmakase` file is SQLite with application ID `0x4f4d4152` and `PRAGMA user_version=1`. Core tables: `sources`, `roots`, `folders`, `photos`, `collections`, `collection_photos`, `keywords`, `photo_keywords`, `folder_mappings`. Foreign keys are enabled. Photo recipes and export options are JSON fields, separate from `lightroom_develop`. Saved recipes include source identity checks, so a replaced file cannot silently overwrite a previous edit. Unknown future catalog versions are refused.

Creation/import publish atomically without clobbering another file. SQLite transactions protect metadata and recipe writes. Back up the `.rawmakase` file with RAWmakase closed. No Lightroom file or RAW is modified by catalog operations.

## CLI

```sh
rawmakase import-catalog input.lrcat Photos.rawmakase
rawmakase catalog-info Photos.rawmakase
rawmakase relink-catalog Photos.rawmakase ROOT_ID /local/photos
rawmakase Photos.rawmakase
```

`catalog-info` lists source root IDs and mappings. Synthetic tests exercise exact source preservation, archived bytes, virtual-copy isolation, metadata, collections, keywords, descendant relinking, idempotent folder imports, changed-source protection, malformed/active catalog rejection, rollback and no-overwrite behavior.

## Preview cache

Library thumbnails are generated on demand and stored as JPEG blobs in a separate `~/.local/share/rawmakase/previews.sqlite3` database (`$XDG_DATA_HOME/rawmakase`, or `RAWMAKASE_DATA_DIR` when configured). All catalogs share this disposable cache. Catalog databases continue to hold metadata and edits only, apart from the preserved Lightroom source archive.

The cache checks file size, nanosecond modification time, a prefix fingerprint and the preview generation version before reuse. Previously generated previews remain usable while originals are offline. Missing Lightroom preview bundles cannot be reconstructed from the `.lrcat` alone; a photo needs to be available at least once to generate its preview.

SQLite WAL permits concurrent readers/workers. The image-payload budget is 512 MiB with least-recently-used eviction; an additional 192 thumbnails are retained in GPU memory. Corrupt individual images are discarded and regenerated. Cache failure falls back to uncached previews. With RAWmakase closed, the preview database and its WAL/SHM companions can be deleted without losing edits. This cache currently covers original Library thumbnails, not full-resolution Develop renders.

The folder tree uses expandable nested rows and aligned counts. Selecting a folder includes its descendant folders. Library and Develop are available through the workspace tabs or G/D shortcuts.

On Linux, desktop network shares require the GVFS FUSE bridge for native filesystem access. RAWmakase starts the installed `gvfsd-fuse` helper when needed before catalog loading or directory selection. Existing GVFS mounts and desktop-managed authentication are reused; no credentials are stored by RAWmakase.

## Metadata validation — 2026-09-26

A closed copied Lightroom catalog with 8,115 images (8,112 photo records plus three generated curve fixtures) was imported and every `(id, rating, pick, label text)` compared against its source. All rows matched, including 478 rated photos, 19 picks, 5 rejects and 79 color labels. Source bytes remained identical after import and native UI metadata edits; the original Lightroom catalog was not used for testing.

Native macOS checks exercised 5-star/red/pick assignment in Library, 1-star/blue/reject assignment in Develop without zooming, clearing with 0/U/repeated color key, choosing Purple, and Shift+5 advancing between offline photos. Database reads confirmed persistence. Automated tests additionally cover virtual copies, null metadata, all five labels, custom/localized label text, filtered selection/advance, invalid writes, text-focus/modifier protection, and both-module shortcut routing. Metadata is edited only in the separate RAWmakase catalog.
