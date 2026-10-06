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
| `demosaic` | RAWmakase's own Bayer and X-Trans demosaicing of the unpacked sensor data | `demosaic.rs` |
| `dng`, `tiff` | A DNG's rendering hints (embedded profile, baseline exposure, crop, opcodes) and the bounded TIFF reader behind them | `dng.rs`, `tiff.rs` |
| `decode_cache` | Disk cache of developed camera images, keyed by file identity, demosaic setting and build | `decode_cache.rs` |
| `camera_profiles` | DCP parsing and validation, camera transforms, RAWmakase's own profiles, profile library and camera matching, DNG temperature/tint | `dcp.rs`, `library.rs`, `open.rs`, `reference.rs` |
| `optics` | The lens correction model the renderer evaluates (vignetting, distortion and lateral CA as radial functions); depends on nothing else | `optics.rs` |
| `lens` | Readers that fill the `optics` model: the tables cameras embed in their RAWs, imported Adobe LCPs and lateral CA measurement, plus profile selection | `embedded.rs`, `lcp.rs`, `auto_ca.rs`, `choice.rs` |
| `develop` | Validated recipes, geometry, color processing, curves, effects, local adjustments, detail rendering, the GPU port and output pixel buffers | `recipe.rs`, `pipeline.rs`, `quality.rs`, `geometry.rs`, `gpu/` |
| `xmp` | Namespace-aware Adobe settings parsing and application to recipes | `parse.rs`, `apply.rs` |
| `presets` | Native JSON recipe presets, installed XMP collections, favorites and preset import | `native.rs`, `library.rs` |
| `storage` | RAW identity checks, legacy sidecar import, session state, application paths, shared format versions and atomic JSON writes | `identity.rs`, `sidecar.rs`, `session.rs`, `format.rs`, `files.rs` |
| `export` | JPEG/16-bit TIFF encoding, selected EXIF, sRGB ICC embedding, atomic output publication | `mod.rs`, `metadata.rs` |
| `catalog` | RAWmakase SQLite database, schema, photo/folder/collection models, edits, relinking and disposable preview cache | `schema.sql`, `models.rs`, `mod.rs`, `preview_cache.rs` |
| `catalog::lightroom` | Read-only Lightroom snapshot import | `mod.rs`, `history.rs` |
| `lr_develop` | Best-effort conversion of Lightroom's serialized Develop settings into a recipe, through XMP; below the catalog, so edit resolution can use it | `lr_develop.rs` |
| `app` | Desktop editor state, UI, dialogs, background task coordination and presentation | Components described below |
| `platform` | OS integration: the Linux GVFS filesystem bridge, drives, the file manager and the browser | `network.rs`, `volume.rs`, `reveal.rs`, `web.rs` |
| `updates` | Release checks and self-update through fastframe-update; the notice itself is in `app` | `updates.rs` |
| `comparison` | Reproducible reference-image comparisons using the same develop APIs | `comparison.rs` |

`color` holds shared numeric primitives (matrices, sRGB transfer) and the tone-curve
model (`color::curve`). It depends on nothing else, so camera profiles, XMP and the
interface use it without depending on recipe or render orchestration. Presets and
camera profiles independently use storage's asset-directory policy; neither
library discovers its folders through the other.

## Desktop composition

`app/mod.rs` owns the `Editor`, initialization and lifecycle. Its fields remain
private. UI and workflow methods have visibility limited to the app module, one
file per tool, workflow or policy; the Develop adjustment panels currently share
`inspector.rs`. The [code map](code-map.md#desktop-application) lists them.

`Editor` deliberately remains the coordinator for state shared across panels.
A panel should call the domain API responsible for an operation, rather than
implementing file formats, SQL or pixel processing itself. Thumbnail decoding
belongs to `raw`; library browsing does not call into the preview renderer.

Each load/render task owns its generation, cancellation token and lifecycle.
The single-slot mailbox replaces pending work. The app discards obsolete results,
and a failure only finishes its owning task. Worker messages use named fields;
render stages are enums, independent of user-facing status text. Exports capture
their recipe before starting, and overwrite confirmation holds the current photo.

Document reset clears its edit history and decoded images together; a catalog
photo's History then comes back from the catalog with its edit. A frame's
history transaction is bound to the load generation, so navigation during drawing
cannot record the previous photo's edits against the new photo. Preset-browser
preferences survive navigation; hover previews and compatibility results do not.
Session preference writes have an explicit destination; UI tests disable them or
inject a temporary file, without changing the process-wide environment.

## Boundaries to preserve

- Dependencies between top-level modules are listed in
  `scripts/deps-allowed.txt`, and `scripts/deps.py check` (CI and `make check`)
  fails on a new one. Today 13 modules still form one cycle; the refactor in
  issue #216 removes lines from the list until none remains. Add a line only
  when the new dependency points down the intended layering.
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

## Invariants the code relies on

- Dialog and relink actions are exhaustive enums carrying their target ID, so a
  new menu item cannot silently become another operation through a catch-all
  branch.
- XMP application is a sequence of named profile, basic, white-balance, color,
  curve, grading, effects and geometry stages. Each stage mutates a private
  recipe; unsupported settings and final validation must pass before that recipe
  is returned.
- Bare relative filenames resolve their parent to `.` consistently for JSON
  writes, catalog creation/import, RAW enumeration and image export.
- Saved-recipe migration validates the envelope and recipe object before
  mutation; malformed legacy recipes return errors rather than panicking.
- Export defaults are validated before catalog writes and on restoration.
  An edit the catalog cannot load is protected: it never enters autosave and is
  not replaced. Failed saves remain pending for retry.
- Autosave writes on its own thread and catalog connection, so a slow disk
  cannot stall editing. Saves before navigation and close wait for it, then
  save synchronously.

## Compatibility

The library API is internal: the RAWmakase binary, its examples and its tests are
its only clients, so modules and functions change freely with them and nothing is
kept for outside callers. The compatibility surface is the saved data: recipe and
preset envelopes are versioned and migrated by `storage::format`; a catalog must
be exactly version 1 to open (other versions are refused, the file left
unchanged), and its schema only ever gains tables, applied idempotently on open;
see [catalogs](catalogs.md#sqlite-format-version-1).

## Validation

`make check` runs the standard suite (see the [README](../README.md#development)).
Photographic checks, measured results and dated validation records live in
[validation.md](validation.md), the per-stage measurement pages and
[macOS and Lightroom validation](macos-lightroom-validation.md).

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

## Live application control

`app::commands` defines transport-independent application operations, target
checks, parameter units and explicit results. `Editor` executes each mutation
inside its generation-bound edit transaction, then answers that request.
`app::automation` owns the input queue and adapter lifecycles. Its MIDI device
adapter translates controls, while its independent socket adapter authenticates,
bounds and expires requests, each with a separate reply channel. These adapters never write catalog edits or render pixels themselves.
Headless test editors do not start listeners or touch connection/configuration
files; only desktop initialization starts them.

External parameter commands default to global scope. MIDI may resolve the active
mask; explicit mask commands require generation and revision guards because
recipe masks have no persistent IDs. Output jobs capture the photo and recipe
revision and call `export::job`; successful queueing is distinct from published
output. The protocol and CLI are documented in [External control](automation.md).

The built-in `rawmakase control` subcommand and optional `tools/rawmakase-ctl`
client share source and the data-directory policy in `storage::paths`. The CLI
performs transport and polling only, without creating an Editor or GUI context.

`rawmakase mcp` is a stdio adapter built with the official Rust MCP SDK. It shares
connection discovery and request/reply validation with the command-line client,
and sends the same guarded commands through the local socket. It owns tool
schemas and temporary preview images, but no recipe state or editing logic.
Point-curve validation, history, auto adjustment and target checks remain in the
application command layer. MCP errors retain application error codes; output
jobs retain their captured edit revision. See [MCP](mcp.md) for setup and tools.

### Automation boundaries

Application commands return typed state snapshots and results; the socket adapter
encodes them as protocol JSON. A single action-name table supports parsing,
discovery and device mapping serialization. Gesture ownership belongs to the
application command layer, with separate MIDI and socket scopes.

The automation owner manages the bounded queue and local socket independently
of MIDI. Each MIDI device owns its mapping and connection state, using a shared
profile registry. Further work can route remaining UI shortcuts through these
operations and share parameter descriptors with all editing panels.
The standalone client still includes its shared implementation by path; a
workspace protocol/client crate would also let MCP schemas reuse those types.
These follow-ups do not require a new editing engine or MIDI-specific commands.
