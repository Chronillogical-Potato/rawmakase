# Code map

Use this page to find an implementation or decide where a change belongs. Read
[architecture.md](architecture.md) for ownership rules, concurrency invariants and
compatibility constraints. Paths below are relative to this document and clickable.
Module roots (`mod.rs`) define their public API; implementation helpers generally
remain private to their domain. This is one Rust package, not a multi-crate workspace.

## Where to start a change

| Work | Start here | Related work |
| --- | --- | --- |
| Add a develop adjustment | [Recipe](../src/develop/recipe.rs), [pipeline](../src/develop/pipeline.rs) | Inspector, XMP application, format migration and rendering regressions |
| Change preview quality or detail | [Quality rendering](../src/develop/quality.rs) | Worker renderer, region/fit/export consistency tests |
| Support another XMP setting | [Parser](../src/xmp/parse.rs), [application stages](../src/xmp/apply.rs) | Recipe validation and XMP tests; library discovery stays in presets |
| Change preset discovery/import | [Preset library](../src/presets/library.rs) | Preset browser UI and shared asset paths |
| Add DCP support | [DCP reader](../src/camera_profiles/dcp.rs), [profile model](../src/camera_profiles/mod.rs) | Camera matching, validation, reference rendering |
| Change JPEG/TIFF output | [Export](../src/export/mod.rs), [metadata](../src/export/metadata.rs) | Export tests; UI captures a recipe before starting |
| Change native catalog behavior | [Catalog API](../src/catalog/mod.rs), [schema](../src/catalog/schema.sql) | Models, catalog tests, library UI |
| Improve Lightroom import | [Importer](../src/catalog/lightroom/mod.rs), [Develop translation](../src/catalog/lightroom/develop.rs) | Preservation tests and unsupported-setting reporting |
| Change autosave or saved formats | [Save policy](../src/app/save_state.rs), [background saver](../src/app/autosave.rs), [legacy sidecars](../src/storage/sidecar.rs), [format migration](../src/storage/format.rs) | Catalog edits, native presets and persistence tests |
| Change navigation or async behavior | [Workflow](../src/app/workflow.rs), [events](../src/app/events.rs), [task lifecycle](../src/app/task.rs) | History, state reset and app regression tests |
| Add a command-line operation | [CLI](../src/main.rs) | Call domain APIs directly; keep the operation usable without an editor |

## Entry points and native boundary

| File | Responsibility |
| --- | --- |
| [src/main.rs](../src/main.rs) | CLI argument parsing and command dispatch; starts the desktop application when no subcommand is selected. |
| [src/lib.rs](../src/lib.rs) | The module list. The library serves the binary, examples and tests; it is not a stable public API. |
| [src/decode_cache.rs](../src/decode_cache.rs) | Disk cache of developed camera images and their highlight recovery, keyed by file identity, demosaic setting and build. |
| [src/raw/mod.rs](../src/raw/mod.rs) | RAW files as RAWmakase sees them: metadata (with DNG, RAF and lens details read on top), development into camera-space images, oriented embedded thumbnails. No unsafe code. |
| [src/raw/ffi.rs](../src/raw/ffi.rs) | The C ABI of the native bridge: declarations, the mirrored metadata struct with its layout check, and one safe wrapper per entry point with its safety contract. |
| [native/raw.cpp](../native/raw.cpp) | C ABI bridge to LibRaw and Little CMS, including native image development and color management. |
| [src/color_math.rs](../src/color_math.rs) | Private shared matrix and sRGB transfer primitives. |
| [src/comparison.rs](../src/comparison.rs) | Reference-image comparisons and reproducible resolved-recipe output using the normal development APIs. |
| [src/demosaic.rs](../src/demosaic.rs) | RAWmakase's own demosaicing of the unpacked sensor data (Bayer and X-Trans); LibRaw's is the fallback. See [demosaicing](demosaic.md). |
| [src/dng.rs](../src/dng.rs) | The rendering hints a DNG carries: embedded camera profile, baseline exposure, default crop and opcode lens corrections. |
| [src/tiff.rs](../src/tiff.rs) | Minimal bounded TIFF directory reader for RAW containers (ARW, DNG, the TIFF inside RAF), and the TIFF field types. |
| [src/jpeg.rs](../src/jpeg.rs) | Walks a JPEG's marker segments up to the image data: embedded XMP and EXIF, and where an export inserts its XMP. |
| [src/exif.rs](../src/exif.rs) | The camera's own EXIF read from a RAW, JPEG or TIFF (for exports, capture times and photo info), and the names of the EXIF, TIFF and GPS tags RAWmakase uses; maker notes and offsets into the RAW are left out. |
| [src/stats.rs](../src/stats.rs) | The opt-in weekly usage report: what it holds, how the install channel and platform are found, and sending it at most once a week ([usage-stats.md](usage-stats.md)). Built only with the default `telemetry` feature. |
| [src/time.rs](../src/time.rs) | Calendar dates and ISO weeks from Unix time, without a date library. |
| [src/updates.rs](../src/updates.rs) | Release checks against GitHub, whether this install may replace itself, and the signed download and install (through fastframe-update). |
| [src/platform/mod.rs](../src/platform/mod.rs), [network.rs](../src/platform/network.rs), [volume.rs](../src/platform/volume.rs), [reveal.rs](../src/platform/reveal.rs), [web.rs](../src/platform/web.rs) | OS integration: Linux GVFS/FUSE path bridge, which drive a path is on, showing a file in the file manager, opening web pages. |

## Development and rendering

| File | Responsibility |
| --- | --- |
| [develop/mod.rs](../src/develop/mod.rs) | Public rendering API and exports of `Recipe`, `Geometry` and `Rendered`. |
| [recipe.rs](../src/develop/recipe.rs) | Serialized adjustment model, defaults, validation, rendering-engine compatibility and profile selection. |
| [geometry.rs](../src/develop/geometry.rs) | Crop, orientation, rotation, flips, straighten, output sizing and coordinate mapping. |
| [orientation.rs](../src/develop/orientation.rs) | Rotate and Flip on the photo as shown, keeping the crop and straightening on the same part of the photo. |
| [image_space.rs](../src/develop/image_space.rs) | Image space, where spots and masks keep positions (oriented photo before lens correction, Transform and crop), and its mapping to and from the view, including the lens distortion inverse. |
| [retouch/mod.rs](../src/develop/retouch/mod.rs) | Heal and Clone operations (spots and brushed areas), validation and Visualize Spots. |
| [retouch/heal.rs](../src/develop/retouch/heal.rs) | Rendering one operation on linear camera pixels: feathered coverage, Clone, and Heal's multigrid membrane solve in log values. |
| [retouch/layer.rs](../src/develop/retouch/layer.rs) | The retouched image: built at once for exports, updated in dirty 256-pixel tiles for previews. |
| [retouch/search.rs](../src/develop/retouch/search.rs) | Automatic source selection on a reduced neighbourhood: border match, texture, clipping and overlap scores. |
| [masks/mod.rs](../src/develop/masks/mod.rs) | Mask groups, components (brush, gradients, ranges), local adjustments, validation and the overlay weights. |
| [masks/eval.rs](../src/develop/masks/eval.rs) | Mask weights for a rendered region: tracing pixels to image space, combining components, caching brush rasters. |
| [masks/brush.rs](../src/develop/masks/brush.rs) | Brush strokes rasterised in image space (flow, density, erase, Auto Mask). |
| [masks/range.rs](../src/develop/masks/range.rs) | Color Range and Luminance Range weights from developed Oklab colours. |
| [masks/local.rs](../src/develop/masks/local.rs) | A mask's sliders as per-pixel deltas, and where each acts in the pipeline. |
| [pipeline.rs](../src/develop/pipeline.rs) | Color/tone processing, sampling, render entry points, neutral picking and legacy engine paths. |
| [basic_tone.rs](../src/develop/basic_tone.rs), [basic_tone_data.rs](../src/develop/basic_tone_data.rs) | Engine 4 Contrast, Whites, Blacks and Dehaze as measured Camera Raw curves, and the measured tables. See [tone controls](tone-controls.md). |
| [local_tone.rs](../src/develop/local_tone.rs), [local_tone_data.rs](../src/develop/local_tone_data.rs) | Engine 4 Shadows and Highlights: an edge-aware local operator fitted to Camera Raw, and its tables. |
| [color_mixer.rs](../src/develop/color_mixer.rs), [color_mixer.bin](../src/develop/color_mixer.bin) | Engine 4 HSL mixer, Saturation and Vibrance as measured hue/saturation/value lookups. See [color mixer](color-mixer.md). |
| [color_grade.rs](../src/develop/color_grade.rs), [color_grade_data.rs](../src/develop/color_grade_data.rs) | Engine 4 color grading as measured per-luminance gains, and its tables. |
| [upright.rs](../src/develop/upright.rs) | Upright analysis: vanishing points from straight lines, giving Level, Vertical, Full and Auto, and the Crop panel's Auto straighten angle. See [transform](transform.md). |
| [quality.rs](../src/develop/quality.rs) | Full-quality detail/spatial processing, resizing and cancellable fit/region rendering. |
| [preview_renderer.rs](../src/develop/preview_renderer.rs) | Stateful preview backend selection, the photo's resolution pyramid, GPU diagnostics and CPU fallback. |
| [pyramid.rs](../src/develop/pyramid.rs) | Resolution pyramid of the recovered (and retouched) camera image for Fit and zoomed-out previews; patched where spot removal changed. |
| [stage_cache.rs](../src/develop/stage_cache.rs) | Preview cache of local-tone blurs, local-tone images, geometry samples, mask weights and brush rasters, keyed by the recipe fields each stage reads. |
| [gpu/mod.rs](../src/develop/gpu/mod.rs) | Optional compute device, bounded/reused buffers, command submission and readback for preview finishing. |
| [gpu/finish.wgsl](../src/develop/gpu/finish.wgsl) | Portable sharpening and separable Lanczos resize compute kernels. |
| [gpu/develop.rs](../src/develop/gpu/develop.rs) | GPU per-pixel color and tone stage: sample buffers kept per stage-cache entry, dispatch and readback. |
| [gpu/develop.wgsl](../src/develop/gpu/develop.wgsl) | WGSL port of the engine 4 per-pixel pipeline (profile tables, tone, curves, mixer, grading). |
| [pipeline/pixel_params.rs](../src/develop/pipeline/pixel_params.rs) | Which recipes the GPU stage covers, and its parameters and tables. |
| [gpu/resident.rs](../src/develop/gpu/resident.rs), [gpu/logs.wgsl](../src/develop/gpu/logs.wgsl), [gpu/local.wgsl](../src/develop/gpu/local.wgsl) | The stages before the per-pixel stage on the device: the photo kept there, local-tone blurs and gain, region sampling through geometry, lens correction and noise reduction. |
| [gpu/present.rs](../src/develop/gpu/present.rs), [gpu/present.wgsl](../src/develop/gpu/present.wgsl), [gpu/reduce.wgsl](../src/develop/gpu/reduce.wgsl) | Finishing developed pixels straight into the viewport texture (sharpening, effects, clipping overlay, monitor profile, histogram) and box-reducing it for the Navigator and thumbnails. |
| [gpu/weights.rs](../src/develop/gpu/weights.rs) | CPU-generated resampling coefficients matching reference boundaries and normalization. |
| [rendered.rs](../src/develop/rendered.rs) | Float RGB output buffers, integer pixel conversion, histogram generation with per-channel clipping counts, the clipping thresholds and the clipping overlay. |
| [curve.rs](../src/develop/curve.rs) | Tone-curve points, validation, interpolation and lookup tables. |
| [effects.rs](../src/develop/effects.rs) | Additional recipe controls used by XMP and spatial finishing such as grain and vignette. |
| [color.rs](../src/develop/color.rs) | Reference color behavior, including vibrance and grading math. |
| [calibration.rs](../src/develop/calibration.rs) | Camera-primary calibration and shadow tint. |
| [white_balance.rs](../src/develop/white_balance.rs) | Fallback illuminant and as-shot temperature estimation. |
| [auto.rs](../src/develop/auto.rs) | Auto: the Basic tone sliders and Vibrance predicted from a reduced render of the photo by fits to Lightroom's Auto values, and white balance from gray world. |

## Camera profiles

| File | Responsibility |
| --- | --- |
| [camera_profiles/mod.rs](../src/camera_profiles/mod.rs) | Profile/table models, validation, camera transforms and profile tone behavior. |
| [dcp.rs](../src/camera_profiles/dcp.rs) | Bounded, endian-aware TIFF/DCP tag decoding. |
| [library.rs](../src/camera_profiles/library.rs) | Explicit profile imports, RAWmakase-library loading and camera matching; lists a camera's Adobe profiles on this computer for the one-click import, and reads nothing else from there. |
| [enhanced.rs](../src/camera_profiles/enhanced.rs) | Bounded XMP HSV big-table decoding, profile curves and internal adjustments. |
| [temperature.rs](../src/camera_profiles/temperature.rs) | DNG temperature/tint and chromaticity conversion. |
| [reference.rs](../src/camera_profiles/reference.rs) | Verified camera-specific exposure baseline and neutral calibration data. |
| [dng_tone.rs](../src/camera_profiles/dng_tone.rs) | Adobe DNG default tone-curve data. |
| [open.rs](../src/camera_profiles/open.rs) | RAWmakase Standard and Color, our own profiles for every camera with a colour matrix. |

## Lens corrections

| File | Responsibility |
| --- | --- |
| [lens/mod.rs](../src/lens/mod.rs) | Radial correction model: vignetting gain, distortion and lateral CA scales, fill scale. |
| [lens/auto_ca.rs](../src/lens/auto_ca.rs) | Remove Chromatic Aberration: lateral CA measured from the decoded image as red and blue radial scales. |
| [lens/embedded.rs](../src/lens/embedded.rs) | Bounded reader for Fujifilm and Sony built-in correction tables in the RAW container. See [lens corrections](lens-corrections.md). |
| [lens/lcp.rs](../src/lens/lcp.rs) | Adobe lens profiles (LCP), imported explicitly into `lens-profiles` and matched to the photo's lens. |

## XMP and presets

XMP translates Adobe settings into a validated recipe. Presets manage reusable
recipes and the installed preset collection; they do not own the renderer.

| File | Responsibility |
| --- | --- |
| [xmp/mod.rs](../src/xmp/mod.rs) | Parsed preset/settings model and XMP API. |
| [parse.rs](../src/xmp/parse.rs) | Namespace-aware XML parsing, curves, provenance and unsupported-setting notes. |
| [apply.rs](../src/xmp/apply.rs) | Named application stages for profiles, basic controls, WB, color, curves, grading, effects and crop; checks consumed settings and validates before returning a recipe. |
| [write.rs](../src/xmp/write.rs) | Writes the Camera Raw-compatible subset of a recipe as `crs:` settings, the XMP packet exports embed; the keys mirror `apply`. Not a round trip: spots and masks, Levels, quarter-turn rotation and flips, and built-in lens corrections are not written. |
| [ns.rs](../src/xmp/ns.rs), [xml.rs](../src/xmp/xml.rs) | XMP namespace URIs and JPEG XMP headers; XML escaping and the packet wrapper RAWmakase writes. |
| [local.rs](../src/xmp/local.rs) | Lightroom's spot removal and masks (`RetouchAreas`, legacy `RetouchInfo`, mask correction lists) from XMP or a catalog, as retouch operations and masks; import only. |
| [presets/mod.rs](../src/presets/mod.rs) | Public preset API. |
| [native.rs](../src/presets/native.rs) | Native JSON recipe preset load/save and shared migration handling. |
| [builtin.rs](../src/presets/builtin.rs) | Built-in presets embedded from `assets/presets`, their group order and ids. |
| [library.rs](../src/presets/library.rs) | XMP collection discovery, import without overwriting existing files, display names and favorites. |

## Persistence, catalog and export

| File | Responsibility |
| --- | --- |
| [storage/mod.rs](../src/storage/mod.rs) | Shared persistence and path API. |
| [files.rs](../src/storage/files.rs) | Application/asset directories, atomic JSON writes, relative parent paths and RAW enumeration. |
| [format.rs](../src/storage/format.rs) | Saved schema/pipeline versions, envelope validation and legacy recipe migration. Recipes keep unknown fields from newer releases. |
| [bitmaps.rs](../src/storage/bitmaps.rs) | Compressed raster data referenced by hash from recipes (future AI masks and patches): catalog `bitmaps` table, sidecar `bitmaps` map. |
| [identity.rs](../src/storage/identity.rs) | RAW fingerprints (size, modification time and a hash of the first bytes) that tie edits and cached previews to a file. |
| [sidecar.rs](../src/storage/sidecar.rs) | Edits saved beside photos before editing moved into the Library: validated and imported into the catalog, with their spots and masks from the companion `*.rawmakase-local.json`, when their folder is added; also read by the CLI's `render`. The library API can still write them. |
| [session.rs](../src/storage/session.rs) | Last-opened path and monitor-profile preferences. |
| [catalog/mod.rs](../src/catalog/mod.rs) | Owns the SQLite connection: catalog lifecycle, browsing queries (photos, folders, collections, roots), metadata and relinking. |
| [catalog/edits.rs](../src/catalog/edits.rs) | A photo's saved edit: recipe and export options, the spots and masks kept beside them, and bitmaps by hash. |
| [catalog/develop_history.rs](../src/catalog/develop_history.rs) | A photo's Develop History, saved in the same transaction as its edit; large settings are stored once per History. |
| [catalog/copies.rs](../src/catalog/copies.rs) | Virtual copies: create, set as master, rename, remove. |
| [catalog/ingest.rs](../src/catalog/ingest.rs) | Adding a folder of photos, with the edits earlier releases saved beside them. |
| [models.rs](../src/catalog/models.rs) | Folder, photo, collection and saved-edit records crossing the catalog API. |
| [schema.sql](../src/catalog/schema.sql) | Every catalog table, idempotent: run on creation and on every open, so older catalogs gain tables added since. |
| [preview_cache.rs](../src/catalog/preview_cache.rs) | Separate, disposable SQLite JPEG cache with identity checks, offline hits and a size budget. |
| [lightroom/mod.rs](../src/catalog/lightroom/mod.rs) | Read-only Lightroom snapshot import, source preservation, relational transfer and atomic destination publication. |
| [lightroom/develop.rs](../src/catalog/lightroom/develop.rs) | Parses Lightroom's serialized Lua settings as data, translates supported controls through XMP, and reports unsupported settings. Never executes Lua. |
| [lightroom/history.rs](../src/catalog/lightroom/history.rs) | Lightroom's develop history per photo, and its recovery from the preserved .lrcat for catalogs imported before it was kept. |
| [export/mod.rs](../src/export/mod.rs) | Export option validation, original-file protection, overwrite policy and atomic publication. |
| [export/encode.rs](../src/export/encode.rs) | JPEG and 16-bit TIFF encoding with the ICC profile, EXIF directories and XMP. |
| [export/metadata.rs](../src/export/metadata.rs), [export/exif.rs](../src/export/exif.rs) | The EXIF directories an export writes (the camera's, with the export's size, orientation, resolution and software), as a JPEG's TIFF block. |
| [export/job.rs](../src/export/job.rs) | One photo's export from start to finish: decode when needed, render, metadata, file. |
| [export/settings.rs](../src/export/settings.rs) | The Export dialog's choices (destination, name, format, size, metadata), saved as `export.json` for the next export. |

## Desktop application

The desktop owns interaction and task coordination. Panels use the domain APIs
above rather than implementing SQL, file formats or pixel processing.

### State and operation policy

| File | Responsibility |
| --- | --- |
| [app/mod.rs](../src/app/mod.rs) | `Editor`, initialization, lifecycle, worker connections and explicit session-write destination. |
| [state.rs](../src/app/state.rs) | Separate document, decoded-image pair, preview, viewport and preset-browser state; centralized document reset. |
| [history.rs](../src/app/history.rs) | Bounded undo/redo, redo-branch invalidation, one transaction per editing gesture, and saving and restoring History with the edit. |
| [editing.rs](../src/app/editing.rs) | Before/after frame snapshots bound to a document generation so navigation cannot mix histories. |
| [activity.rs](../src/app/activity.rs) | Mutually exclusive foreground states: file choice, overwrite confirmation and export. |
| [task.rs](../src/app/task.rs) | Load/render generations, cancellation and completion ownership. |
| [save_state.rs](../src/app/save_state.rs) | Clean, pending, saving, failed and protected edits; debounce and retry policy. |
| [autosave.rs](../src/app/autosave.rs) | The background saver thread and its catalog connection. |
| [workflow.rs](../src/app/workflow.rs) | Opening/navigating photos, flushing edits, scheduling previews, publishing textures and launching exports. |
| [events.rs](../src/app/events.rs) | Receives worker messages, rejects stale generations and applies accepted results to editor state. |
| [dialogs.rs](../src/app/dialogs.rs) | Typed dialog intents and native file/folder choosers. |
| [auto.rs](../src/app/auto.rs) | Runs Auto (the Basic panel's Auto button, the WB menu, Ctrl/Cmd+Shift+U) off the UI thread and applies the estimate as one History step. |
| [catalog.rs](../src/app/catalog.rs) | UI workflows for native catalogs, Lightroom import, folder addition, relinking and applying imported edits. |
| [bulk_import.rs](../src/app/bulk_import.rs) | Importing camera profiles, lens profiles and presets from chosen files or whole folders, reporting what could not be imported. |
| [upright.rs](../src/app/upright.rs) | Runs the Transform panel's Upright analysis off the UI thread. |
| [stats.rs](../src/app/stats.rs) | The one-time question about sharing usage stats and its Preferences row; the report itself is in `src/stats.rs`. |
| [updates.rs](../src/app/updates.rs) | The update notice under the toolbar and the About rows in Preferences; the checks and downloads themselves are in `src/updates.rs`. |

### Panels and interaction

| File | Responsibility |
| --- | --- |
| [workspace.rs](../src/app/workspace.rs) | Frame composition, workspace switching, shortcuts, filmstrip, status, pending work, autosave and close handling. |
| [toolbar.rs](../src/app/toolbar.rs) | Develop toolbar and menus. |
| [settings_transfer.rs](../src/app/settings_transfer.rs) | Copy Settings and its dialog, Paste Settings and Paste from Previous, through `develop::settings_groups`. |
| [sync.rs](../src/app/sync.rs) | Sync Settings: the open photo's chosen groups onto the other selected photos, off the UI thread, saved in one transaction with a History step each, undone as one command. |
| [export/mod.rs](../src/app/export/mod.rs), [export/dialog.rs](../src/app/export/dialog.rs) | Export dialog, remembered export settings, background exports and their progress. |
| [preferences.rs](../src/app/preferences.rs) | Preferences window: app, catalog, profile, cache and display settings. |
| [onboarding.rs](../src/app/onboarding.rs) | First-run setup: a catalog, then optional Lightroom profiles and presets. |
| [theme.rs](../src/app/theme.rs), [icons.rs](../src/app/icons.rs) | Interface colors (Lightroom's neutral grays, with fastframe-theme's palettes) and the Lucide icon set. |
| [inspector.rs](../src/app/inspector.rs) | Histogram, adjustment controls and export settings. |
| [tone_drag.rs](../src/app/tone_drag.rs) | Dragging in the histogram: its five regions, the slider each drives, and one History step per drag. |
| [clipping.rs](../src/app/clipping.rs) | The histogram's clipping triangles: independent shadow and highlight warnings, hover preview, J, and the triangles' channel colours. |
| [viewport.rs](../src/app/viewport.rs) | Photo canvas, fit/100%, pan, crop (with its guide overlay and Straighten ruler) and white-balance picking; hands the pointer to the active tool. |
| [crop_tool.rs](../src/app/crop_tool.rs) | The Crop tool's guide overlays (O, Shift+O), Straighten ruler, portrait/landscape swap (X) and Auto straighten. See [transform](transform.md#crop-and-straighten). |
| [overlay.rs](../src/app/overlay.rs) | The active tool's drawing over the photo (pins, circles, brush cursor, handles) and pointer ownership. |
| [retouch_tool.rs](../src/app/retouch_tool.rs) | Remove tool (Q): spots, brushed areas, source dragging, keys and its drawer. |
| [mask_tool.rs](../src/app/mask_tool.rs) | Masking tool (Shift+W): mask list, components, brushes and gradients on the photo, and the local adjustment sliders. |
| [presets.rs](../src/app/presets.rs) | Preset search, groups, favorites, compatibility, application and temporary hover previews. |
| [snapshots.rs](../src/app/snapshots.rs) | Develop's Snapshots panel: named states of the open photo's edit, kept per photo in the catalog (`catalog/snapshots.rs`, including Lightroom's imported snapshots). |
| [user_presets.rs](../src/app/user_presets.rs) | New Develop Preset, and Update, Rename and Delete for presets made here (`presets/user.rs`, written by `xmp/preset_write.rs`). |
| [photo_metadata.rs](../src/app/photo_metadata.rs) | Rating, color label and pick/reject controls and shortcuts. |
| [widgets.rs](../src/app/widgets.rs) | Shared buttons, adjustment sections, sliders, curve editor and workspace tabs. |
| [library/mod.rs](../src/app/library/mod.rs) | The Library: composes the owners below, writes metadata and virtual-copy changes to the catalog, and draws the sidebar, grid, filmstrip and info panel. |
| [library/filter.rs](../src/app/library/filter.rs) | The source (folder scope or collection), the filter bar's search, flag, rating and label, the offline filter and sort order; computes what is shown. |
| [library/availability.rs](../src/app/library/availability.rs) | Which originals are online, found out in the background while the Library already shows them. |
| [library/volumes.rs](../src/app/library/volumes.rs) | Whether each drive is attached and its free space, probed off the UI thread, and the volume header row. |
| [library/textures.rs](../src/app/library/textures.rs) | The bounded embedded and edited preview textures, their requests to the preview workers and the tickets that drop stale results. |
| [library/copy_name.rs](../src/app/library/copy_name.rs) | The Copy Name field: the name being typed, saved on commit and kept across selection changes and failed saves. |
| [library/tree.rs](../src/app/library/tree.rs) | Folder/collection hierarchy, rows and tree actions. |
| [library/cell.rs](../src/app/library/cell.rs) | Individual photo grid cells. |
| [library/thumbnails.rs](../src/app/library/thumbnails.rs) | Batched file availability and bounded thumbnail work using the RAW API and preview cache. |
| [library/previews.rs](../src/app/library/previews.rs) | The two preview workers (embedded thumbnails, and each photo's edited preview) with their disk cache and progress. |

### Background work

| File | Responsibility |
| --- | --- |
| [worker/mod.rs](../src/app/worker/mod.rs) | Named event payloads, load/render jobs, task kinds, render stages and repaint notification. |
| [worker/latest.rs](../src/app/worker/latest.rs) | Single-slot mailbox: submitting a new job replaces pending work rather than growing a queue. A panicking job does not end the thread. |
| [worker/loader.rs](../src/app/worker/loader.rs) | RAW metadata and profile loading, embedded preview, the half-size then full decoded image and neighboring thumbnails. |
| [worker/renderer.rs](../src/app/worker/renderer.rs) | Fit previews, reduced-then-full 100% regions, cancellation, monitor conversion and clipping overlays. After a panic it reports the job failed and rebuilds all of its state. |

## How an operation moves through the app

1. **Open a photo:** workspace/dialog/catalog actions enter `app::workflow`. It
   flushes the old document, resets document state, starts a load generation and
   submits a loader job. The loader uses `raw`, `camera_profiles` and `storage`.
   `app::events` accepts only current results. A saved catalog recipe, when present,
   replaces the loader's resolved defaults for a catalog photo.
2. **Edit and preview:** panels change the recipe. `app::editing` and `history`
   group the change; save policy marks it pending. `workflow` submits the effective
   recipe and viewport to the renderer. The worker calls `develop`; with a usable
   GPU and a recipe the GPU stage covers (engine 4 with the reference flags, see
   `pipeline/pixel_params.rs`), the color and tone stage and the finishing run on
   the device and the result is drawn straight from its texture; otherwise the
   worker renders on the CPU and prepares the display bytes. Accepted results
   become the preview. At 100%, an edit or pan whose region is not already cached
   and takes more than a moment publishes a reduced draft first, then the
   full-resolution region.
3. **Save edits:** workspace autosave and navigation/close flushing call
   `workflow`; autosave writes in the background (`autosave`), and flushing
   waits for it before saving synchronously. Photos are edited only through the
   Library, so edits always save through `catalog`. An edit the catalog cannot load
   is protected and cannot autosave, and failed writes remain pending for retry. Presets are separately saved reusable recipes.
4. **Export:** foreground activity coordinates destination/overwrite handling.
   The export job captures the recipe, renders through the development API, then
   calls `export` to encode and publish. Output embeds sRGB independently of the
   monitor profile. Navigation is held during export.
5. **Apply a preset:** `presets` discovers assets, `xmp` parses/translates Adobe
   settings, and the app handles compatibility and hover state. Hover is temporary;
   application commits a recipe through the editing/history flow.
6. **Import Lightroom:** the app or CLI calls `catalog::lightroom`. It reads a
   source snapshot, preserves source data, transfers relationships, and publishes
   a new native catalog. Develop translation is best effort and reports unsupported
   settings; it does not promise Lightroom rendering parity.

## Where data lives

`storage::data_dir()` selects `RAWMAKASE_DATA_DIR` when set. Otherwise it uses
`~/Library/Application Support/RAWmakase` on macOS and `$XDG_DATA_HOME/rawmakase`
(default `~/.local/share/rawmakase`) on Linux.

| Data | Location and lifetime |
| --- | --- |
| Original RAW and Lightroom catalog | User-selected source files; treated as read-only. |
| Legacy sidecar edits | Adjacent `photo.ARW.rawmakase.json` / `photo.RAF.rawmakase.json`, plus `photo.ARW.rawmakase-local.json` for spots and masks, or identity-keyed JSON under the data directory's `sidecars/`, saved by releases before 0.1.8. Imported into the catalog when their folder is added and left on disk unchanged. |
| Native catalog | User-selected `.rawmakase` SQLite file; authoritative catalog metadata, edits and preserved import data. Spots and masks are in the `local_edits` table, Develop History in `develop_history`, raster data in `bitmaps`. |
| Native preset / exported photo | User-selected JSON / JPEG / TIFF destination. |
| Session preferences | `session.json` in the data directory; last path and monitor ICC path. UI tests inject a temporary destination or disable writes. |
| Preset favorites | `preset-favorites.json` in the data directory. |
| Installed assets | `xmp-presets/` and `camera-profiles/` under asset roots; shared discovery also checks the legacy XDG/Linux data location. Imported XMP files go to `xmp-presets/Imported/` under the current data directory. |
| Library previews | `previews.sqlite3` in the data directory; disposable cache, not a source of edits. |
| Decoded photos | `decoded/` in the cache directory (`~/Library/Caches/RAWmakase` on macOS, `$XDG_CACHE_HOME/rawmakase`, default `~/.cache/rawmakase`, on Linux; `RAWMAKASE_CACHE_DIR` overrides it): developed camera images of opened and prefetched photos, capped at 4 GB, least recently used removed first. Disposable. |
| Build output | `target/`; generated binaries, documentation and local macOS bundle. |

## Tests and reference tools

Most unit tests live alongside their implementation, either inline or in a
sibling `tests.rs`. Keep regressions with the domain that owns the behavior.

| Location | Coverage / use |
| --- | --- |
| [app/tests.rs](../src/app/tests.rs), [library/tests.rs](../src/app/library/tests.rs) | Editor interactions, state transitions, worker results, navigation, library trees and metadata. Small state owners also contain inline tests. |
| [develop/pipeline/tests.rs](../src/develop/pipeline/tests.rs) | Rendering, geometry and reference regressions; numeric helpers also have inline tests. |
| [camera_profiles/tests.rs](../src/camera_profiles/tests.rs) | Profile parsing and validation. |
| [xmp/tests.rs](../src/xmp/tests.rs), [presets/tests.rs](../src/presets/tests.rs) | Settings parsing/application and native preset compatibility. |
| [storage/sidecar/tests.rs](../src/storage/sidecar/tests.rs) | Migration, source identity, conflict protection and fallback persistence. |
| [catalog/tests.rs](../src/catalog/tests.rs) | Catalog, import and relinking behavior; preview-cache tests live in its module. |
| [export/tests.rs](../src/export/tests.rs) | JPEG/TIFF precision, ICC and EXIF output. |
| [develop/gpu/tests.rs](../src/develop/gpu/tests.rs) | Explicit hardware tests for CPU/GPU agreement, borders, buffer reuse, crop/region handling, effects and fallback. |
| [tests/color/](../tests/color/main.rs), [tests/corpus/README.md](../tests/corpus/README.md) | The color corpus: synthetic chart DNGs rendered and compared with committed snapshots and Camera Raw renders on every `cargo test`; private photo and Adobe-profile tiers behind `RAWMAKASE_CORPUS`. |
| [examples/preview_benchmark.rs](../examples/preview_benchmark.rs) | Read-only release benchmark of first Fit, slider Fit, 100% region and export renders on a supplied RAW; checks Fit against the resized export. |
| [tests/persistence.rs](../tests/persistence.rs) | Public API regressions for relative paths, malformed legacy recipes and invalid export defaults; isolates process-wide path settings in a child process. |
| [tests/raw_fixtures.rs](../tests/raw_fixtures.rs) | Ignored private RAW development/export and repeated-navigation memory tests (`RAWMAKASE_FIXTURES`). |
| [tests/private_profiles.rs](../tests/private_profiles.rs) | Ignored installed/private DCP coverage (`RAWMAKASE_PROFILES`). |
| [tests/xmp_presets.rs](../tests/xmp_presets.rs) | Ignored installed XMP collection audit. |
| [catalog/private_tests.rs](../src/catalog/private_tests.rs) | Ignored private Lightroom catalog validation (`RAWMAKASE_LRCAT`). |
| [tests/data/README.md](../tests/data/README.md), [curve samples](../tests/data/lightroom-point-curves.json) | Small checked-in Lightroom point-curve reference data and its provenance. |
| [scripts/make-curve-fixtures.py](../scripts/make-curve-fixtures.py) | Generates synthetic TIFF ramps for manual Lightroom curve comparisons. |
| [scripts/compare-preview.py](../scripts/compare-preview.py) | Compares resized sRGB previews without exposure/color fitting; distinct from the Rust comparison command. |

Use the [README's development section](../README.md#development) for the standard
suite. Private fixture checks require external data and are explicitly ignored by
default. A passing unit suite does not replace the photographic/manual checks in
the validation docs.

## Build, packaging and documentation

| Location | Responsibility |
| --- | --- |
| [Cargo.toml](../Cargo.toml), [Cargo.lock](../Cargo.lock) | Package/toolchain requirements, dependencies and locked resolution. |
| [build.rs](../build.rs) | Locates LibRaw/Little CMS, compiles the C++ bridge and configures platform OpenMP linking. |
| [Makefile](../Makefile) | Build/check and local install/uninstall shortcuts. |
| [packaging/macos/app.sh](../packaging/macos/app.sh), [Info.plist](../packaging/macos/Info.plist) | Local Finder-launchable macOS bundle and its metadata. |
| [rawmakase.desktop](../packaging/applications/rawmakase.desktop), [rawmakase.svg](../packaging/icons/rawmakase.svg) | Linux launcher and application icon. |
| [LICENSE](../LICENSE), [Adobe notice](../licenses/Adobe-DNG-SDK.txt) | Project licensing and third-party DNG attribution. |
| [README](../README.md) | Build/run instructions, controls, CLI examples and supported scope. |
| [Architecture](architecture.md) | Module ownership, state/thread invariants and compatibility rules. |
| [Catalogs](catalogs.md) | Catalog use, import preservation, relinking, schema and preview cache. |
| [XMP presets](xmp-presets.md) | Supported settings and compatibility limitations. |
| [Preview performance](preview-performance.md) | GPU/CPU responsibilities, hardware support limits, measured timings and reproduction. |
| [Rendering quality](rendering-quality.md) | Engine 3 behavior, profile support and reference comparisons. |
| [Color pipeline](color-pipeline.md) | Versioned rendering contract, including the historical pipeline-1 description. |
| [Dependencies](dependencies.md) | Native dependency and linking notes. |
| [Validation](validation.md) | Recorded checks, evidence limits, reproduction and measured performance. |
| [macOS / Lightroom validation](macos-lightroom-validation.md) | Dated photographic comparisons and platform validation results. |
| [Parity gaps](parity-gaps.md) | Known differences and work still needed for Lightroom parity. |
| [Tone controls](tone-controls.md), [color mixer](color-mixer.md), [lens corrections](lens-corrections.md), [transform](transform.md), [demosaic](demosaic.md) | How each engine 4 stage was measured against Camera Raw and what it does. |
| [Retouching](retouching.md), [masking](masking.md) | The Remove and Masking tools: use, rendering and what is not implemented. |
| [Lightroom profiles](lightroom-profiles.md) | RAWmakase's own profiles, importing Adobe and third-party profiles, and supported profile features. |

When adding or moving a module, update its entry here. Put API contracts in Rust
doc comments, ownership decisions in the architecture guide, and measured results
in the validation documents so each has a clear home.
