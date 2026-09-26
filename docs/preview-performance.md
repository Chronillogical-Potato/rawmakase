# Preview performance and GPU support

Fit and zoomed-out views render from a resolution pyramid of the photo (see
[below](#resolution-pyramid)); 100% regions and export use the full-resolution
image. The desktop uses GPU compute for sharpening and Lanczos3 resizing of full
resolution renders. RAW decoding, color/tone processing, grain/vignette and export
stay on the CPU. A 1024-pixel draft provides feedback while the quality pass runs.
The status line shows `GPU finish` when compute was used.

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
