# RAWmakase catalogs and Lightroom import

Use **Catalog → Import Lightroom catalog…**, select a closed `.lrcat`, then choose a new `.rawmakase` filename. Existing destination files are never overwritten. **New catalog** creates an empty library; **Add photo folder** recursively registers RAW files (any extension in `RAW_EXTENSIONS`), JPEG, PNG and TIFF files without copying or modifying them.

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

The importer reads Lightroom `rating`, `pick` (−1 rejected, 0 unflagged, 1 picked), and `colorLabels`. Missing ratings/flags become zero; missing labels become empty. Label text is preserved exactly, including custom/localized names. Standard Lightroom label names map to the corresponding color. Other text displays white and remains searchable/filterable; a custom Lightroom label-set mapping is not reliably available from a standalone `.lrcat`, so RAWmakase does not guess its color. No write-back/synchronization with Lightroom is performed. In the Grid, rating, flag and label keys apply to every selected photo. Each change is one step that Cmd+Z undoes and Cmd+Shift+Z redoes, in the same undo sequence as Develop; it is kept in memory and cleared when another catalog opens. Caps Lock auto-advance is not implemented.

Shortcut reference: [Adobe Lightroom Classic keyboard shortcuts](https://helpx.adobe.com/lightroom-classic/desktop/introduction-to-lightroom-classic/keyboard-shortcuts.html).

## Import and preservation

The importer takes a private snapshot of the source, checks its SQLite structure/integrity, and imports in one transaction. It requires a closed/exported catalog without a nonempty WAL/journal; it refuses an active/incomplete copy rather than ignoring pending changes. The current importer accepts catalogs under 2 GB and was exercised on the supplied Lightroom v13 catalog. Incompatible layouts fail without leaving a destination file.

Normalized tables retain images and virtual-copy identities, original paths, folder roots, capture dates, ratings, flags, color labels, collections/memberships, keywords/parents/memberships, serialized Develop settings and Develop history steps (the `lightroom_history` table, shown under **From Lightroom** in the History panel, where each step can be applied). Imported collection sets and collections can be browsed, read-only, in the Library's Collections panel; smart collections stay hidden until their rules are evaluated. The Quick Collection (B, Cmd+B, Cmd+Shift+B) is the only collection whose photos can be changed. Keywords can be added and removed in the Keywording panel, for every selected photo, as one undoable step. A byte-exact copy of the entire source `.lrcat` is stored in `sources.original_catalog`, preserving snapshots, stacks, IPTC, GPS, faces, smart rules and other fields that RAWmakase does not yet interpret. This makes the new catalog larger; it avoids discarding unrecognized Lightroom data. External `.lrcat-data`, preview bundles, originals and profiles are not embedded or synthesized.

Smart collection definitions and any stored membership are preserved; RAWmakase does not evaluate Adobe's smart-collection rule language. Collections marked “smart snapshot” may therefore have no stored members.

## Descriptive metadata and XMP sidecars

Title, caption, creators, copyright, capture time, location and keywords are kept per photo in the catalog; a virtual copy has its own once created. A field with no value in the catalog is the file's own (its EXIF, at export); a cleared field leaves the file's out too.

**Lightroom import** reads each photo's title, caption, creators, copyright, capture time and location from the XMP Lightroom keeps in its catalog; keywords, ratings, flags and labels come from Lightroom's own tables. Catalogs imported before this read it once when opened.

**Add photo folder** reads, for each new photo, its XMP sidecar and, for JPEG and TIFF, the XMP inside the file. A RAW's sidecar is `IMG_1234.NEF.xmp` (digiKam's default), else `IMG_1234.xmp`; when both exist the first is used and the other is listed as ignored. `IMG_1234.xmp` beside both `IMG_1234.NEF` and `IMG_1234.JPG` is the RAW's; the JPEG reads only `IMG_1234.JPG.xmp` and its own XMP, field by field, the sidecar's winning, and an explicitly empty sidecar value clearing the field. Sidecars that can't be read are counted on the status line, which lists them on hover. Adding a folder again does not read the sidecars of photos already in the catalog.

**Read Metadata from Files…** (thumbnail menu) does, for the selected masters: every field a file has replaces the catalog's, your edits included; fields the file lacks are kept. It asks first, is one step for Undo, and never reads into virtual copies.

Keywords are read from `lr:hierarchicalSubject` ("|" between levels), else `digiKam:TagsList` ("/"), then any `dc:subject` name no path has, as a top-level keyword; with neither, `dc:subject` is flat keywords. digiKam's color labels map to Lightroom's names (red, yellow, green, blue, purple; others by their color name) and its pick labels to picks and rejects. What can't be reconstructed: a name containing "|" (or "/" through digiKam) read from another tool's file becomes a hierarchy; a top-level keyword named like an element of a hierarchy, in a file without a path for it, merges into that hierarchy; the parents of a keyword with "|" in its name are lost on a round trip.

Not read or written: IPTC location and contact fields, headline, credit, source, usage terms, instructions, people and face regions, other digiKam data, ratings in other namespaces, XMP history and unknown namespaces. Sidecars are never written. A virtual copy created by RAWmakase 0.1.12 or older has no values of its own and uses the file's until edited.

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
