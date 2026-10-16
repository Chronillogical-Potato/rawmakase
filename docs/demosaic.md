# Demosaicing

By default RAWmakase demosaics full-size images itself (`crates/rawmakase-native/src/demosaic.rs`). LibRaw only unpacks the sensor data (`ora_cfa_open` / `ora_cfa_copy` in `crates/rawmakase-native/native/raw.cpp`), and still provides metadata, white balance, crops and previews. Black levels (including per-position patterns), white level and camera white balance are applied in RAWmakase. The colour pattern comes from LibRaw's `COLOR()` as a 48×48 tile, which covers Bayer and X-Trans periods.

Bayer uses two passes:

1. **Green** at red and blue sites, blending Hamilton–Adams horizontal and vertical estimates by inverse gradient, clamped to the neighbouring greens.
2. **Red and blue** from the difference to green of the nearest same-colour samples (radius 1, else 2), weighted by inverse distance.

X-Trans (`crates/rawmakase-native/src/demosaic/xtrans.rs`) builds one full RGB candidate per direction (horizontal, vertical, both diagonals): green interpolated along that line between the nearest greens with a same-colour curvature correction, red and blue from their difference to that green along the same line. Each pixel then blends the four candidates by how smooth their R − G and B − G is in the surrounding 3×3, so a wrong direction, which leaves a colour pattern, gets almost no weight. A single isotropic estimate turned fine periodic detail such as louvres and grilles into green and magenta moiré (X100F DSCF8231, DSCF8232); LibRaw's Markesteijn did not. Bands of 128 rows with a mirrored margin keep the working set small.

Borders are mirrored, which keeps the Bayer colour parity. Half-size drafts and files that aren't single-channel Bayer or X-Trans (e.g. Foveon, linear DNG) still use LibRaw.

Preferences > Performance, or `Decode::full(Demosaic::Libraw)` in code, switches to LibRaw's AHD (Bayer) and 1-pass Markesteijn (X-Trans); the environment variable `RAWMAKASE_LIBRAW_DEMOSAIC=1` forces the same for the whole run.

## Measurements — 2026-09-26 (M1 Pro, loaded machine)

| File | LibRaw develop | RAWmakase develop | 100% crop MAE vs Adobe (LibRaw → RAWmakase) | Fine-detail MAE |
|---|---:|---:|---:|---:|
| X100F DSCF7853 (X-Trans, 24 MP) | 2.33 s | 0.42 s | 0.0186 → 0.0187 | 0.0048 → 0.0050 |
| A7 II DSC00805 (Bayer, 24 MP) | 0.79 s | 0.37 s | 0.0105 → 0.0094 | 0.0072 → 0.0059 |
| A7CR (Bayer, 61 MP) | 1.62 s | 1.04 s | — | — |

The crop metrics compare three 800 px crops at 100% with Lightroom's X100F export and Camera Raw's A7 II render, allowing ±1 px alignment. Fine detail is the error of the image minus a 7 px box blur. Scorecards with the new default: X100F Lightroom references 0.0093 MAE, A7 II Camera Raw references 0.0063.
