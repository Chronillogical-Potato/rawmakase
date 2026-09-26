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

## Adobe LCP profiles

Not implemented yet. Like camera profiles, LCP files will be explicitly imported by the user and never read from a Lightroom installation.
