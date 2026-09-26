# Imported Lightroom camera and look profiles

RAWmakase reads profiles only from its own library and files the user explicitly selects. It does not scan Lightroom, Camera Raw, or other Adobe application directories. Proprietary profile assets are not shipped with the application.

## Import and select

Use **Edit → Import profiles…** or **Import profiles…** beside the Develop Profile selector. Select one or more `.dcp` / `.xmp` files. For Adobe Color and the other Adobe Raw looks, import both the camera model's **Adobe Standard DCP** and the desired **XMP look profiles**. You can select these together or import the DCP first. Missing dependencies produce an error; another camera's profile is never substituted.

The CLI uses the same importer:

```sh
rawmakase import-profiles '/selected/Fujifilm X100F Adobe Standard.dcp' '/selected/Adobe Color.xmp'
```

Selected files are validated and copied into `camera-profiles` under RAWmakase's application data directory (`~/Library/Application Support/RAWmakase` on macOS, `$XDG_DATA_HOME/rawmakase` on Linux). An identical reimport is harmless. A different file with the same filename is reported instead of overwritten. The entire selection is validated before copying starts. Source files remain unchanged.

Importing does not change the current edit. Choose the imported look in **Profile**. New unedited photos prefer a compatible imported Adobe Color, then imported Adobe Standard, then the bundled Sony A7 II profile where applicable, then LibRaw's camera matrix rendered with the DNG default tone curve (engine 4, see [color-pipeline.md](color-pipeline.md)). Saved edits retain their embedded profile and previous rendering flags.

## Rendering

An enhanced XMP profile is resolved against its matching camera DCP. The profile name remains, for example, **Adobe Color**, but the combined rendering contains that camera's matrices and calibration tables plus the XMP look's table and curve. Camera matching is mandatory.

Supported enhanced-profile features:

- Adobe DNG SDK-format HSV big tables, versions 1/2, decoded from Adobe base85 and zlib, with bounded input/output sizes and validation.
- Linear and sRGB value indexing, interpolated hue/saturation/value corrections.
- A profile-specific master tone curve, applied separately from the user's point curve. Identity per-channel profile curves are accepted; nonidentity profile channel curves are reported as unsupported.
- Profile-internal Highlights, Shadows and Clarity adjustments, plus monochrome conversion. These use RAWmakase's existing approximate operators without moving the user's sliders.
- The six Adobe Raw looks: Color, Portrait, Neutral, Landscape, Vivid and Monochrome, at their normal 100% strength.
- XMP sidecars/presets and Lightroom catalog `Look` records resolve imported profiles by name, UUID when supplied, and camera model.

The existing bounded DCP implementation continues to support imported camera-matching and third-party film profiles. RGB-table creative profiles, adaptive/AI profiles, nondefault profile Amount, unsupported profile settings, and unsupported DCP variants fail explicitly. This is not universal Lightroom profile support or pixel-identical Lightroom development.

Schema/pipeline 5 embeds the resolved camera profile, enhanced color table, sampled curve, identity and copyright in the recipe. Reopening does not require the source XMP or DCP to remain available. Old schema 1–4 recipes migrate without changing their prior look. Older RAWmakase versions reject version 5 instead of silently dropping enhanced-profile data.

## Lightroom comparison — 2026-09-26

Six full-size, 16-bit sRGB TIFF references were exported from Lightroom Classic using a separate comparison catalog and copied X100F RAWs. The XMP look parameters contain the actual selected Adobe profile's table and curve. Exported metadata and catalog records were checked to verify the selected look. No original catalog edits or original RAW changes were made.

Measurements are encoded-sRGB MAE at an 800-pixel long edge, using the existing preview comparison script, with no exposure/color fitting or geometric registration. Active-area/demosaicing differences prevent these from being pixel-aligned sensor comparisons.

| Profile | RAW | Adobe Standard baseline MAE | Imported profile MAE |
|---|---|---:|---:|
| Adobe Color | DSCF7853 | 0.03305 | 0.02643 |
| Adobe Portrait | DSCF7845 | 0.01869 | 0.01819 |
| Adobe Neutral | DSCF7866 | 0.03488 | 0.01667 |
| Adobe Landscape | DSCF7853 | — | 0.02786 |
| Adobe Vivid | DSCF7853 | — | 0.03074 |
| Adobe Monochrome | DSCF7853 | — | 0.03087 |

The baseline renders use the same recipe and camera DCP without the enhanced look layer. Color improves approximately 20%, Neutral 52%, and Portrait 3% on these examples. These results support similar overall looks on the tested X100F images, not exact slider equivalence across cameras. Monochrome mixing, local tones, clarity, saturation, lens corrections and detail processing remain independently implemented. A Julka-folder RAW was also rendered using the imported default Adobe Color as a separate loading/rendering smoke check.

Private comparison files remain in `target/profile-parity/` and a private local directory. The repository contains no private photos, exported references, or Adobe profile assets. Tests cover import persistence/conflicts, malformed and truncated table payloads, synthetic hue rotation, camera restrictions, default selection, XMP/catalog resolution, embedded-profile round trips, finite output, monochrome neutrality and full-image/region equivalence. The supplied 62 DCPs and all six imported Adobe Raw looks passed private checks.
