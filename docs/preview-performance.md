# Preview performance and GPU support

Fit and zoomed-out views render from a resolution pyramid of the photo (see
[below](#resolution-pyramid)); 100% regions and export use the full-resolution
image. The desktop runs the per-pixel color and tone stage on the GPU (see
[GPU develop stage](#gpu-develop-stage)) and uses GPU compute for sharpening and
Lanczos3 resizing of full resolution renders. RAW decoding, geometry sampling, local
tones, grain/vignette and export stay on the CPU. There is no separate draft: every slider change renders the real
pipeline at Fit size, and at 100% a half-resolution preview of the region comes
first (see [slider responsiveness](#slider-responsiveness)). The status line shows `GPU finish` when compute was used.

The shaders are portable WGSL through wgpu, with no CUDA or Metal-specific code.
Metal is used on macOS; Linux NVIDIA/AMD devices can use Vulkan with a working
compatible driver and sufficient compute/storage limits. Unsupported adapters,
oversized images and GPU failures fall back to CPU. That is intended platform
support, not evidence of testing on every driver. **Hardware tested here: Apple
M1 Pro only. Linux/NVIDIA/AMD runtime validation remains outstanding.**

See [architecture](architecture.md#preview-compute-backend) for resource limits,
fallback behavior, cancellation and the division between CPU and GPU work.

## Resolution pyramid

Opening a photo recovers highlights once at full resolution. Fit and zoomed-out
renders then use a pyramid of that image: each level halves the previous one with a
2×2 box average in linear camera space, and levels are built on first use and kept
while the photo is open. A render takes the smallest level with at least one pixel
per output pixel and develops each output pixel exactly once, averaging four
bilinear taps over the pixel's footprint. Before, Fit developed a camera image
reduced to twice the output size (four times the pixels) and resized it afterwards.

Radius-based effects are scaled to the output: sharpening uses the full-resolution
radius times the output scale (a three-tap kernel of the same variance below half a
pixel), Clarity and Texture radii follow the level size, and grain keeps its
full-resolution pattern with the amplitude a resized export would have. Fit is
therefore an approximation of the export resized, not identical to it; a unit test
bounds the mean difference on a textured image with sharpening, local and spatial
effects, and the benchmark checks each photo. 100% regions and exports are
unchanged.

`examples/preview_benchmark`, release build, Apple M1 Pro, 1600-pixel Fit, median of
three exposure changes after a warm-up. "Local" adds Shadows +40, Highlights −30
and Clarity +20. Error is the mean absolute channel difference from the export
render resized to the same size (0–1 scale).

| Photo | Render | Before | After |
| --- | --- | ---: | ---: |
| X100F DSCF7853, 6032×4032, Adobe Color DCP | First Fit after opening | 1407 ms | 508 ms |
| | Fit | 1098 ms | 283 ms |
| | Fit, local | 1429 ms | 498 ms |
| | Fit error vs export | 0.0023 | 0.0016 |
| Sony A7CR, 9564×6376, no DCP | First Fit after opening | 640 ms | 379 ms |
| | Fit | 565 ms | 249 ms |
| | Fit, local | 731 ms | 278 ms |
| | Fit error vs export | 0.0047 | 0.0059 |

The remaining Fit time is the per-pixel color pipeline (DCP tables, profile tone
curve, `powf`), about 1.7 megapixels per render. 100% regions with local
adjustments (about 1.1 s on the X100F and 1.5 s on the A7CR for a 1600×1000
region) still recompute Clarity over the full image on every change.

## Stage caching

The desktop renderer keeps the results of the stages before the per-pixel color
pipeline (`src/develop/stage_cache.rs`), each keyed by the recipe fields it reads:

- local-tone blurs: log luminance and its box blurs, which depend on white balance,
  profile and lens vignetting but not on exposure (exposure shifts all of them
  equally);
- the local-tone image: the blurs with Clarity, Texture and, before engine 4,
  Shadows and Highlights applied;
- samples: each output pixel's camera value after geometry, lens correction and
  noise reduction, and its source position.

Exposure, curve, HSL, grading and Engine 4 Shadows/Highlights edits therefore rerun
only the per-pixel stage; Clarity and Texture edits reuse the blurs. Two entries are
kept per stage (Fit and a 100% view) within 512 MB per stage; larger results are
computed and not kept. Export uses no cache. A unit test checks that cached renders
equal uncached ones after each kind of edit.

Measured as above, but as the best of three interleaved runs of the before and after
builds, because other work was loading the machine (load average about 30 on 10
cores; unchanged export timings varied by up to 2×). "Clarity" changes Clarity on
every render instead of exposure.

| Photo | Render | Pyramid only | With stage cache |
| --- | --- | ---: | ---: |
| X100F | Fit | 358 ms | 264 ms |
| | Fit, local | 435 ms | 251 ms |
| | Fit, Clarity edits | 688 ms | 405 ms |
| | 100% region, local | 1429 ms | 366 ms |
| | 100% region, Clarity edits | 1296 ms | 546 ms |
| A7CR | Fit | 141 ms | 135 ms |
| | Fit, local | 187 ms | 120 ms |
| | 100% region, local | 1140 ms | 1386 ms |

The A7CR's full-resolution local-tone stages (61 megapixels) exceed the cache
budget, so its 100% view with Clarity is not faster yet; local tones on pyramid
levels are the next step.

## Slider responsiveness

The render worker used to answer each change with a 1024-pixel draft from the
legacy (engine 2) pipeline, wait 150 ms, then render the full-quality Fit, which
took one to two seconds. With the pyramid and stage cache, the worker renders the
current engine at Fit size straight away, with no draft and no wait, so every
frame shown while dragging is the real rendering. Photos with older engines (before
3) render Fit from a reduced copy of the camera image instead.

At 100%, each change first renders the visible region from the pyramid at half
resolution or less (at most 0.6 megapixels), which the viewport stretches over the
region, then the full-resolution region. The worker's mailbox keeps only the latest
job and a newer job cancels the running one, so while a slider moves the view
follows the reduced previews, and the sharp region appears when it stops. Pending
region or quality work never delays a newer slider job beyond the next cancellation
check (per row in blurs and per pixel in the color stage; a GPU command already
submitted finishes first).

Renderer times per change, same conditions as the stage cache table, but with the
machine even busier (load average 40–58), so these are upper bounds. Before this
change the first update was the 50 ms legacy draft, and the real rendering came
150 ms plus 0.8–1.8 s later.

| Photo | View | Real rendering per change |
| --- | --- | ---: |
| X100F | Fit | 170 ms |
| | Fit, local | 228 ms |
| | 100% preview (half resolution) | 62 ms |
| | 100% preview, local | 67 ms |
| | 100% preview, Clarity edits | 104 ms |
| | 100% full region, local | 221 ms |
| | 100% full region, Clarity edits | 325 ms |
| A7CR | Fit | 184 ms |
| | Fit, local | 223 ms |
| | 100% preview (half resolution) | 34 ms |
| | 100% preview, local | 41 ms |
| | 100% preview, Clarity edits | 116 ms |
| | 100% full region, local | 1469 ms |

## GPU develop stage

`src/develop/gpu/develop.wgsl` ports the per-pixel stage (`process_pixel`) of the
current engine: white balance, camera matrix, DCP HueSatMap (with its two-illuminant
blend), calibration, exposure and the DNG exposure ramp, LookTable, enhanced-look
table and curve, the profile tone curve, the engine 4 Shadows/Highlights map,
measured Basic curves, levels, parametric and point curves, color mixer, color
grading, Oklab Defringe/Monochrome and gamut compression. It runs on the samples in
the stage cache, which stay on the device while only the recipe changes; parameters
and tables (`pixel_params.rs`) are uploaded per render and the result is read back
for sharpening and spatial effects on the CPU.

The port covers engine 4 with reference curves, color and calibration and a profile
tone curve, which every new photo uses. Older engines, and color grading with
Blending or Balance outside the measured tables, render on the CPU, as do machines
without a usable adapter; a GPU failure disables the GPU for the session. Export
always uses the CPU, which remains the reference.

Profile tables are read with explicit trilinear interpolation from a storage buffer
rather than a hardware-filtered 3D texture, whose reduced-precision filter weights
would not match the CPU. A hardware test compares the two stages on 4000 samples over
seven recipes that exercise every table and branch: the 99.9th-percentile channel
difference is at most 0.0003 and the mean at most 0.00001 (0–1 scale). With
Monochrome, a few near-neutral pixels whose Oklab hue is unstable can land in
another band (largest difference 0.01).

Rendering into a texture that egui draws directly would need the compute work on
the UI's wgpu device, which the separate compute device deliberately avoids. The
readback of a 1600-pixel Fit is a few milliseconds, so it stays.

Best of two interleaved runs against the previous commit, load average 24–35:

| Photo | Render, per exposure change | CPU color stage | GPU color stage |
| --- | --- | ---: | ---: |
| X100F | Fit | 166 ms | 14 ms |
| | Fit, local | 437 ms | 39 ms |
| | Fit, Clarity edits | 461 ms | 149 ms |
| | 100% preview | 51 ms | 10 ms |
| | 100% region | 188 ms | 24 ms |
| | 100% region, local | 381 ms | 53 ms |
| | First Fit after opening | 392 ms | 198 ms |
| A7CR | Fit | 95 ms | 11 ms |
| | Fit, local | 149 ms | 26 ms |
| | 100% preview | 25 ms | 6 ms |
| | 100% region | 106 ms | 20 ms |
| | 100% region, local | 1327 ms | 1359 ms |

Fit differs from the resized export exactly as much as the CPU Fit (0.0016 X100F,
0.0059 A7CR). Clarity edits and the A7CR's 100% view with local adjustments are now
dominated by the full-resolution local-tone blurs on the CPU.

## GPU finishing measurements — 2026-09-26

Release build on Apple M1 Pro, private Fujifilm X100F RAW (6032×4032), installed
Adobe Standard DCP, 1600-pixel final fit. Each number is the median of three
exposure changes after one warm-up. Decoding, first highlight recovery, GPU device
initialization, the worker's 150 ms refinement debounce and UI presentation are
outside these timings. GPU timings include uploads, processing and readback.
These are renderer measurements, not end-to-end slider latency or a broad camera
benchmark. No source RAW, edit or export file was changed by the benchmark.

| Render | Before | After | Change |
| --- | ---: | ---: | ---: |
| Interactive draft | 134.4 ms | 49.2 ms | 63% less time |
| Full-quality fit, neutral local controls | 2713.7 ms | 1800.8 ms | 34% less time |
| Full-quality fit, local tone adjustments | 3518.3 ms | 2455.6 ms | 30% less time |

The draft comparison intentionally includes the reduction from 1600 to 1024
pixels; it is a responsiveness/temporary-detail tradeoff. Final-quality sizes and
processing remain the same. Local adjustments in this benchmark are shadows
+0.4, highlights −0.3 and clarity +0.2.

For the current code alone, CPU versus GPU quality-fit medians were 2473.0 versus
1800.8 ms with neutral local controls and 2906.2 versus 2455.6 ms with local tones.
The rest of the improvement comes from CPU work: parallel column processing in
local-tone blurs and skipping hue calculations for inactive color controls.
Exposure gain is prepared once per recipe rather than calculated per pixel.

Maximum CPU/GPU channel differences on the final benchmark images were
0.00000036 (neutral local controls) and 0.00000030 (local tones), in normalized
float RGB. GPU tests enforce a 0.00002 tolerance and cover different sharpening
radii, image edges, one-pixel dimensions, resizing, repeated buffer reuse, rotated
crops, regions, spatial effects, cancellation and fallback. The neutral-color CPU
fast path is also checked against its general processing path.

Full-quality rendering still takes seconds on this photograph. The GPU work does
not accelerate LibRaw decoding or the complete color pipeline. High-resolution
color processing, local effects and memory transfers remain targets for future
profiling. Cancellation now interrupts recovery, local-tone and sharpening CPU
work; an already-submitted GPU command must finish, after which a stale result is
discarded. The interactive draft remains an approximation of the final render.

## Reproduce

Standard tests do not require a GPU. Run hardware checks explicitly:

```sh
cargo test --locked develop::gpu::tests -- --ignored --nocapture
cargo run --release --locked --example preview_benchmark -- /path/to/photo.RAF 3
```

The benchmark prints the actual adapter and profile, and fails if a Fit render
differs from the resized export by a mean of 0.01 or more. It reads the supplied RAW and
installed profiles and writes no photographic files. Use the same RAW, profile,
release build and machine conditions for comparisons. Keep private photos outside
the repository.
