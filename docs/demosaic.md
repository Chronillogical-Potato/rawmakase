# Demosaicing

RAWmakase demosaics images itself (`crates/rawmakase-decode/src/demosaic.rs`) from the sensor data its own decoders unpack (`crates/rawmakase-raw`, see [RAW decoding](raw-decoding.md)). Black levels (including per-position patterns), white level and camera white balance are applied in RAWmakase. The colour pattern is the decoder's filter as a 48×48 tile, which covers Bayer and X-Trans periods.

Bayer uses two passes:

1. **Green** at red and blue sites, blending Hamilton–Adams horizontal and vertical estimates by inverse gradient, clamped to the neighbouring greens.
2. **Red and blue** from the difference to green of the nearest same-colour samples (radius 1, else 2), weighted by inverse distance.

X-Trans (`crates/rawmakase-decode/src/demosaic/xtrans.rs`) builds one full RGB candidate per direction (horizontal, vertical, both diagonals): green interpolated along that line between the nearest greens with a same-colour curvature correction, red and blue from their difference to that green along the same line. A fifth, isotropic candidate (greens from all eight neighbours, weighted by agreement) covers soft detail where no direction is better. Each pixel blends the five candidates by how smooth their R − G and B − G is in the surrounding 3×3 (weight = (smoothest ÷ own roughness)⁴), so a wrong direction, which leaves a colour pattern, gets little weight. The same-colour curvature correction of the directional green is kept small (0.2): at 0.5 it overshot on sharp edges and left a violet, stair-stepped fringe (X-S10). A single isotropic estimate turned fine periodic detail such as louvres and grilles into green and magenta moiré (X100F DSCF8231, DSCF8232); LibRaw's Markesteijn did not. Bands of 128 rows with a mirrored margin keep the working set small.

Borders are mirrored, which keeps the Bayer colour parity. Half-size drafts are ⌈W/2⌉ × ⌈H/2⌉: on Bayer data each pixel is a 2 × 2 block's red, blue and mean green; other mosaics (X-Trans) are demosaiced in full and averaged over 2 × 2 blocks. Small RAWs and linear DNGs, which are not mosaics, develop at full size in both modes. Files that are neither (Foveon, four-colour filters) are not developed.

LibRaw's AHD and Markesteijn, once offered under Preferences > Performance, are no longer offered; a session that chose them reads as RAWmakase's demosaic.

## Measurements — 2026-09-26 (M1 Pro, loaded machine)

| File | LibRaw develop | RAWmakase develop | 100% crop MAE vs Adobe (LibRaw → RAWmakase) | Fine-detail MAE |
|---|---:|---:|---:|---:|
| X100F DSCF7853 (X-Trans, 24 MP) | 2.33 s | 0.42 s | 0.0186 → 0.0187 | 0.0048 → 0.0050 |
| A7 II DSC00805 (Bayer, 24 MP) | 0.79 s | 0.37 s | 0.0105 → 0.0094 | 0.0072 → 0.0059 |
| A7CR (Bayer, 61 MP) | 1.62 s | 1.04 s | — | — |

The crop metrics compare three 800 px crops at 100% with Lightroom's X100F export and Camera Raw's A7 II render, allowing ±1 px alignment. Fine detail is the error of the image minus a 7 px box blur. Scorecards with the new default: X100F Lightroom references 0.0093 MAE, A7 II Camera Raw references 0.0063.
