# Lens corrections

## Built-in (camera-embedded) corrections — engine 4

Many cameras store per-shot lens corrections in the RAW file. Lightroom applies them as the "built-in lens profile" without any Adobe profile. RAWmakase reads them in `src/lens/embedded.rs` when a file is opened (`Metadata::lens`) and applies them while rendering, so no user import is needed.

| Camera | Tables read | Default |
|---|---|---|
| Fujifilm RAF | FujiIFD 0xF00B distortion, 0xF00F lateral CA, 0xF010 vignetting | On |
| Sony ARW | raw SubIFD 0x7032 vignetting, 0x7035 lateral CA, 0x7037 distortion | Off (not yet compared with Lightroom) |

Radius is normalized to the half diagonal of the decoded image. Rendering order:

1. Highlight reconstruction in camera space.
2. Vignetting gain in linear camera space, evaluated at the source pixel. Shadows, Highlights, Clarity and Texture measure local luminance after this gain.
3. Distortion and lateral CA as a per-channel radial remap while sampling, scaled so corrected corners stay inside the sensor.

The recipe field `lens_builtin` controls it. New recipes enable it when the file's correction is marked `default_on`. Recipes saved before engine 4 have no field and never apply it.

### Fujifilm vignetting strength

Applied at full strength, the Fujifilm table leaves corners about 0.08 EV brighter than Lightroom on three X100F photos, while the centre matches. Raising the gain to the power 0.85 brings corners within ±0.03 EV. The Sony and LCP paths match at full strength. The X100F Lightroom reference scorecard goes from 0.0148 to 0.0120 mean MAE.

### Validation — 2026-09-26

The Fujifilm vignetting table for DSCF7853 (X100F) matches the `FixVignetteRadial` opcode Adobe wrote into Lightroom's DNG of the same file. The gain is 1.085 / 1.319 / 1.656 at radius 0.5 / 0.8 / 1.0, against Adobe's 1.087 / 1.326 / 1.670. Distortion is zero for this lens. Adobe's `WarpRectilinear` holds only a red-plane radial term under one pixel, the same order as the Fujifilm CA table.

Full renders against Lightroom's Adobe Standard exports, encoded sRGB at 1200 px, no fitting:

| Photo | Without | With built-in correction |
|---|---:|---:|
| DSCF7853 overall MAE | 0.0304 | 0.0196 |
| DSCF7853 corner MAE | 0.0571 | 0.0167 |
| DSCF7845 overall MAE | 0.0426 | 0.0300 |
| DSCF7845 corner MAE | 0.0546 | 0.0241 |

Sony's embedded vignetting for the FE 55mm F1.8 ZA at f/1.8 restores 1.95× at the corner. Adobe's LCP profile for that lens predicts 1.81×. Lightroom does not enable profile corrections by default, and it has not been checked whether it applies Sony's embedded data, so Sony corrections start disabled until a Lightroom reference is available.

## DNG files

A DNG records the corrections Lightroom applies to its raw image. `src/dng.rs` reads FixVignetteRadial from OpcodeList2 and WarpRectilinear from OpcodeList3 (radial terms, per plane, when centred) into the same correction model, enabled by default. It also reads the embedded camera profile (offered as the file's own profile, as in Lightroom), BaselineExposure and DefaultCrop. The Lightroom-made DNG of DSCF7853 renders at 0.020 MAE against Lightroom's export with no imported files, against 0.030 before. GainMap opcodes (used by phone DNGs for lens shading) are not applied yet.

## Adobe LCP profiles

Lightroom's Enable Profile Corrections uses an Adobe lens profile. RAWmakase reads the same `.lcp` files when the user imports them (`rawmakase import-lens-profiles FILE…`, or the app's import command); they are copied to `lens-profiles` in the data directory and never read from a Lightroom installation. `src/lens/lcp.rs` matches the photo's lens model, preferring raw profiles, and interpolates the model in focal length and aperture, taking the farthest focus distance. It converts distortion, vignetting and chromatic models to the correction above. When the profile sets PreferMetadataDistort, the camera's own distortion is kept, as Lightroom does.

`Recipe::lens_profile` enables it, from `crs:LensProfileEnable`. `lens_distortion` and `lens_vignetting` are the profile's Distortion and Vignetting amounts (`crs:LensProfileDistortionScale` / `VignettingScale`, 0–200%). With a matching imported profile, the profile replaces the built-in correction. Without one, the built-in correction applies when `lens_builtin` is set.

Against Camera Raw 18.6 renders with profile corrections on (8 A7 II photos, FE 55mm F1.8 ZA at f/1.8), RAWmakase with the imported Adobe profile averages 0.0088 MAE, with centre and corner exposure within ±0.02 EV. Without correction the corners were 1 EV darker.
