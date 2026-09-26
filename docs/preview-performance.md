# Preview performance and GPU support

The desktop uses GPU compute for preview sharpening and Lanczos3 resizing.
RAW decoding, color/tone processing, grain/vignette and export stay on the CPU.
A 1024-pixel draft provides feedback while the full-resolution quality pass runs.
Final fit resolution still follows the physical viewport, and 100% regions retain
full-resolution detail. The status line shows `GPU finish` when compute was used.

The shaders are portable WGSL through wgpu, with no CUDA or Metal-specific code.
Metal is used on macOS; Linux NVIDIA/AMD devices can use Vulkan with a working
compatible driver and sufficient compute/storage limits. Unsupported adapters,
oversized images and GPU failures fall back to CPU. That is intended platform
support, not evidence of testing on every driver. **Hardware tested here: Apple
M1 Pro only. Linux/NVIDIA/AMD runtime validation remains outstanding.**

See [architecture](architecture.md#preview-compute-backend) for resource limits,
fallback behavior, cancellation and the division between CPU and GPU work.

## Measurements — 2026-09-26

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

The benchmark prints the actual adapter and profile. It fails if GPU finishing is
unavailable or if its output exceeds the CPU comparison tolerance, so CPU fallback
cannot silently be reported as GPU performance. It reads the supplied RAW and
installed profiles and writes no photographic files. Use the same RAW, profile,
release build and machine conditions for comparisons. Keep private photos outside
the repository.
