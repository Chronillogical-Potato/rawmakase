# Architecture

RAWmakase remains a single Rust crate with explicit domain modules. The desktop app
and CLI compose these APIs; parsing, persistence and rendering implementations do
not import the desktop UI. Use the domain paths below for new work.

For file-by-file navigation, runtime flows, storage locations and feature entry
points, see the [code map](code-map.md). This guide describes the boundaries those
files should preserve.

## Module ownership

| Module | Owns | Main extension points |
| --- | --- | --- |
| `raw` | LibRaw/Little CMS boundary, camera metadata, decoded images, oriented embedded thumbnails | RAW decoding and native color management |
| `camera_profiles` | DCP parsing and validation, camera transforms, profile discovery, reference metadata and DNG temperature/tint | `dcp.rs`, `library.rs`, `reference.rs` |
| `develop` | Validated recipes, geometry, color processing, curves, effects, detail rendering and output pixel buffers | `recipe.rs`, `pipeline.rs`, `quality.rs`, `geometry.rs` |
| `xmp` | Namespace-aware Adobe settings parsing and application to recipes | `parse.rs`, `apply.rs` |
| `presets` | Native JSON recipe presets, installed XMP collections, favorites and preset import | `native.rs`, `library.rs` |
| `storage` | RAW identity checks, protected sidecars, session state, application paths, shared format versions and atomic JSON writes | `sidecar.rs`, `session.rs`, `format.rs`, `files.rs` |
| `export` | JPEG/16-bit TIFF encoding, selected EXIF, sRGB ICC embedding, atomic output publication | `mod.rs`, `metadata.rs` |
| `catalog` | RAWmakase SQLite database, schema, photo/folder/collection models, edits, relinking and disposable preview cache | `schema.sql`, `models.rs`, `mod.rs`, `preview_cache.rs` |
| `catalog::lightroom` | Read-only Lightroom snapshot import and best-effort conversion of serialized Develop settings | `mod.rs`, `develop.rs` |
| `app` | Desktop editor state, UI, dialogs, background task coordination and presentation | Components described below |
| `platform` | OS integration such as the Linux GVFS filesystem bridge | `network.rs` |
| `comparison` | Reproducible reference-image comparisons using the same develop APIs | `comparison.rs` |

`color_math` is a private collection of shared numeric primitives. Camera profiles
use it directly, without depending on recipe or render orchestration. Presets and
camera profiles independently use storage's asset-directory policy; neither
library discovers its folders through the other.

## Desktop composition

`app/mod.rs` owns the `Editor`, initialization and lifecycle. Its fields remain
private. UI and workflow methods have visibility limited to the app module:

- `workspace.rs`: a short frame coordinator, workspace panels, shortcuts and pending-work UI.
- `toolbar.rs`: develop commands and menu presentation.
- `state.rs`: document, decoded-image pair, preview, viewport and preset-browser ownership.
- `history.rs`: bounded undo/redo and gesture transactions.
- `editing.rs`: frame snapshots tied to a document generation.
- `activity.rs`: mutually exclusive dialogs, overwrite confirmation and export.
- `task.rs`: operation generations, cancellation and running/completed state.
- `save_state.rs`: clean, pending, failed and protected edit persistence.
- `events.rs`: generation-checked worker result dispatch and accepted-result handlers.
- `inspector.rs`: develop controls and histogram.
- `viewport.rs`: image display, zoom, pan, crop and white-balance picking.
- `presets.rs`: preset filtering, favorites, compatibility and hover previews.
- `catalog.rs`: catalog operations and applying imported Lightroom edits.
- `workflow.rs`: opening photos, saving edits, preview scheduling and export orchestration.
- `dialogs.rs`: exhaustive file/catalog action enums and native file choosers.
- `widgets.rs`: reusable toolbar, slider and tone-curve widgets.
- `photo_metadata.rs`: rating, label and flag controls and shortcuts.
- `library/`: browsing state and grid, with separate tree, cell and thumbnail modules.
- `worker/`: event/job definitions, coalescing mailbox, RAW loader and preview renderer.

`Editor` deliberately remains the coordinator for state shared across panels.
A panel should call the domain API responsible for an operation, rather than
implementing file formats, SQL or pixel processing itself. Thumbnail decoding
belongs to `raw`; library browsing does not call into the preview renderer.

Each load/render task owns its generation, cancellation token and lifecycle.
The single-slot mailbox replaces pending work. The app discards obsolete results,
and a failure only finishes its owning task. Worker messages use named fields;
render stages are enums, independent of user-facing status text. Exports capture
their recipe before starting, and overwrite confirmation holds the current photo.

Document reset clears its edit history and decoded images together. A frame's
history transaction is bound to the load generation, so navigation during drawing
cannot record the previous photo's edits against the new photo. Preset-browser
preferences survive navigation; hover previews and compatibility results do not.
Session preference writes have an explicit destination; UI tests disable them or
inject a temporary file, without changing the process-wide environment.

## Boundaries to preserve

- Keep `eframe`, `egui` and native chooser code in `app`. The CLI must be able to
  use domain operations without creating an editor or UI context. The crate still
  links its existing GUI dependencies; this is module separation, not a separate
  headless build feature.
- The catalog owns its connection. Lightroom import is a child adapter with
  access to that connection for its import transaction; do not expose the
  connection publicly or put Lightroom-specific queries back into general
  catalog operations.
- Parsing XMP produces settings, while application validates and resolves a
  recipe. Collection discovery and favorites belong in `presets`.
- Validate recipe changes at domain boundaries. Saved format versions and
  migrations are shared by native presets and sidecars. Changes to recipe defaults
  must account for older saved edits and rendering-engine choices.
- Keep original RAWs and Lightroom sources read-only. Preserve unsupported source
  data, sidecar conflict protection, no-clobber publication, ICC/EXIF handling and
  temporary-file write behavior.
- Rendering math stays in `develop` and `camera_profiles`. Fit previews, regions
  and exports must continue to share the relevant processing paths.
- Keep OS integration in `platform` and native decoding/color management in
  `raw`. Use the existing asset-path policy instead of duplicating environment
  variable handling in feature modules.

## Review and compatibility

The structural review found several responsibilities sharing large files:
`app.rs` mixed workflows and all editor panels; `core.rs` combined recipes,
geometry and pixel processing; `io.rs` combined unrelated persistence and export
formats; `catalog.rs` combined the native database with Lightroom adaptation.
These responsibilities now have explicit owners. Camera-profile parsing and
library discovery, XMP parsing and application, library widgets, and worker
lifecycles have also been separated.

Numeric dialog selectors were replaced with exhaustive enums. Relink actions
carry their target ID, so a new menu item cannot silently become another operation
through a catch-all numeric branch. Dense catalog/UI expressions were expanded,
and catalog DDL now lives in `catalog/schema.sql`.

XMP application is a sequence of named profile, basic, white-balance, color,
curve, grading, effects and geometry stages. Each stage mutates a private recipe;
unsupported settings and final validation must pass before that recipe is returned.

The deeper review also fixed concrete correctness issues:

- Bare relative filenames now resolve their parent to `.` consistently for JSON
  writes, catalog creation/import, RAW enumeration and image export.
- Saved-recipe migration validates the envelope and recipe object before mutation;
  malformed legacy recipes return errors rather than panicking.
- Export defaults are validated before sidecar/catalog writes and on restoration.
  Existing invalid edits remain protected against replacement.
- Catalog header handling keeps the profile-resolved recipe supplied by the loader
  until an actual saved catalog edit replaces it.
- Reloading preset compatibility cancels an outstanding hover render.
- Protected edits never enter autosave; failed saves remain pending for retry.

Existing public paths such as `core`, `profile`, `io`, `library`, `worker`,
`curve`, `effects` and `quality` remain compatibility exports. The old catalog
import and XMP-library entry points also remain available. Implementations and
production callers use the new module paths. Do not add new functionality to
compatibility facades. The application worker protocol itself changed from
positional tuples to named payloads; callers constructing `worker::Event` values
must update those constructions. The domain APIs and serialized data formats
remain compatible.

This refactor preserves recipe serialization, schema/pipeline versions, rendering
algorithms, default asset locations and CLI commands. It introduces no new
runtime dependencies or database migration. It does not establish Lightroom
rendering parity beyond the existing implementation.

## Validation

Use Rust 1.95 or newer with the native libraries documented in the main README:

```sh
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

The original suite passed 86 tests. The expanded suite adds regressions for bounded
history, gesture coalescing, cancellation, stale and cross-task failures, overwrite
confirmation, document changes during a frame, loader-resolved recipes, preset
hover invalidation, injected session persistence, malformed migration input and
invalid export defaults. Relative-path persistence runs in a child process with
its own working directory and data directory, avoiding global test interference.

Existing coverage for lossless catalog import/relinking, profile validation,
color/geometry, preview/export consistency, ICC/EXIF, and editor interactions
remains in place. The external private-fixture tests continue to compile through
the domain compatibility APIs.

Five tests require private Lightroom catalogs, RAWs, profiles or installed XMP
presets and remain explicitly ignored by default. They were not run for this
refactor. Automated egui interaction tests ran; a manual desktop session and
Linux runtime validation were not performed. See `validation.md` for the broader
photographic validation procedure.

## Preview compute backend

`develop::PreviewRenderer` owns the optional GPU processor and fallback state.
The desktop renderer creates it on its worker thread. Headless UI tests and the
legacy `worker::renderer` entry point remain CPU-only. Domain color processing,
RAW development and exports use the existing CPU implementation.

`develop::gpu` uses portable WGSL compute through wgpu: Metal on macOS and Vulkan
on supported Linux drivers, including NVIDIA and AMD. It owns a separate compute
device so its limits and error scopes do not affect the UI device. It uses ordinary
32-bit storage buffers and compute workgroups, without vendor extensions. The
current hardware validation is Apple M1 Pro; Linux GPU vendors require validation
on those machines before claiming equivalent performance or numerical precision.

Preview sharpening and Lanczos3 downsampling run on the GPU. Coefficients use the
CPU reference's normalization and boundary convention. Grain/vignette retain their
CPU implementation and ordering; those fit previews still use GPU resizing.
Full-resolution regions preserve the sharpening halo before cropping. Legacy
rendering engines and export remain on CPU. The status line identifies GPU
finishing separately from CPU rendering.

Buffers are reused by dimensions, limited by the adapter and a 1 GiB aggregate
buffer budget. No compatible adapter, oversized buffers, allocation/validation
errors and readback failures fall back to CPU. After a GPU failure the preview
renderer releases the backend and avoids retrying it for every edit. Restarting
creates a new backend. GPU execution already submitted cannot be interrupted;
obsolete readback results are discarded. Full-quality CPU stages check cancellation
within pixel/row/column work. Pending jobs still coalesce in a single-slot mailbox.

Interactive drafts now use a 1024-pixel camera-space image. They remain temporary,
explicitly labeled drafts; the final fit still processes full-resolution detail
and resizes to the physical viewport. The 150 ms refinement debounce remains so
continuous editing does not repeatedly start expensive full-quality work.
