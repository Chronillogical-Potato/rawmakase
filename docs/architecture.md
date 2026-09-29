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
| `storage` | RAW identity checks, legacy sidecar import, session state, application paths, shared format versions and atomic JSON writes | `identity.rs`, `sidecar.rs`, `session.rs`, `format.rs`, `files.rs` |
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
  migrations are shared by native presets, catalog edits and legacy sidecars. Changes to recipe defaults
  must account for older saved edits and rendering-engine choices.
- Keep original RAWs and Lightroom sources read-only. Preserve unsupported source
  data, legacy sidecars, no-clobber publication, ICC/EXIF handling and
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
- Export defaults are validated before catalog writes and on restoration.
  Existing invalid edits remain protected against replacement.
- Catalog header handling keeps the profile-resolved recipe supplied by the loader
  until an actual saved catalog edit replaces it.
- Reloading preset compatibility cancels an outstanding hover render.
- Protected edits never enter autosave; failed saves remain pending for retry.
- Autosave writes on its own thread and catalog connection, so a slow disk
  cannot stall editing. Saves before navigation and close wait for it, then
  save synchronously.

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

Use Rust 1.98 or newer with the native libraries documented in the main README:

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
on supported Linux drivers, including NVIDIA and AMD. On the desktop it shares the
UI's wgpu device (eframe's), which the app creates with the storage limits previews
need (`gpu::required_limits`) and a high-performance adapter; OpenGL and software
adapters are not used for compute. Every GPU call runs inside its own validation,
internal and out-of-memory error scopes, so a failure disables the preview GPU
without reaching the UI. Headless callers (tests, the benchmark) create a device of
their own. It uses ordinary 32-bit storage buffers, Rgba8Unorm storage textures and
compute workgroups, without vendor extensions. The current hardware validation is
Apple M1 Pro; Linux GPU vendors require validation on those machines before claiming
equivalent performance or numerical precision.

The per-pixel color and tone stage of the current engine runs on the GPU for previews
(`gpu/develop.wgsl`, fed by `pipeline/pixel_params.rs`), on the stage cache's
samples; other recipes and all exports use the CPU stage, which is the reference.
When the GPU renders a preview, the developed pixels stay on the device and
`gpu/present.wgsl` finishes them into a texture that egui draws directly (registered
as a native texture by the render worker): sharpening, vignettes and grain, the
clipping overlay, the monitor profile (a 52³ lattice sampled from lcms once per
profile, interpolated trilinearly) and 8-bit encoding, in the CPU reference's order.
The histogram is counted in the same pass and the Navigator copy is reduced on the
GPU; only the histogram (3 KB) and, for catalog photos, a 640-pixel library
thumbnail are read back. Nothing is converted or uploaded on the UI thread. Recipes
the port does not cover, older engines and machines without a usable adapter render
on the CPU, and the worker then prepares the display bytes, histogram and reduced
copies before the UI uploads the texture. Fit and zoomed-out previews render from a
resolution pyramid at output size (see
[preview performance](preview-performance.md#resolution-pyramid)). Full-resolution
regions preserve the sharpening halo before cropping. Lanczos3 resizing of full
renders remains on the GPU for readback callers. Export remains on the CPU. The
status line says `GPU` for presented frames, `GPU finish` when only finishing used
the GPU, and `CPU` otherwise.

For the same recipes, the stages before the per-pixel stage also run on the device
when the photo fits its buffer limits (`gpu/resident.rs`): the photo or pyramid level
is kept there with its local-tone blurs, the gain is computed only for the pixels a
region samples, and regions are sampled there through geometry, lens correction and
noise reduction. Only the reduced input of the
Shadows/Highlights map is read back; the map is built on the CPU. A failure in these
stages turns off only this path for the session.

Presented textures are kept per view (the whole photo, or a 100% region) and size,
two of each, so a slider moving at 100% alternates between the reduced preview's and
the full region's textures without allocating. Each texture carries a generation that
changes whenever it is written; the worker reuses a finished frame when switching
views only while its texture still holds it.

Buffers are reused by dimensions, limited by the adapter and a 1 GiB aggregate
buffer budget. No compatible adapter, oversized buffers, allocation/validation
errors and readback failures fall back to CPU. After a GPU failure the preview
renderer releases the backend and avoids retrying it for every edit. Restarting
creates a new backend. GPU execution already submitted cannot be interrupted;
obsolete results are discarded before they are published. Full-quality CPU stages check cancellation
within pixel/row/column work. Pending jobs still coalesce in a single-slot mailbox.

There is no separate interactive draft. Fit renders the current pipeline from the
smallest pyramid level at or above the physical viewport size on every change;
at 100% a reduced region from the pyramid is published first (as a draft stage),
then the full-resolution region. The single-slot mailbox and cancellation keep
continuous editing on the latest change.
