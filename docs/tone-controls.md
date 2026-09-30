# Basic panel tone controls

## Measurement

References are Camera Raw 18.6 renders from Photoshop 2026, the same engine and process version (2012, PV 15.4) as Lightroom Classic 15.5. `scripts/camera-raw-sweep.py` writes a Photoshop script that renders each slider at several positions with Adobe Standard, no lens profile and As Shot white balance. `scripts/lightroom-scorecard.py` re-renders every reference in RAWmakase from the settings embedded in it and reports the error. The 2026-09-26 set is three Fujifilm X100F and two Sony A7 II photos, 8 sliders × 6 positions, 2000 px. It is private and not in the repository.

Each adjusted render was compared with the default render of the same photo. For Contrast, Blacks and Whites, a single curve applied to the brightest and darkest channel in ProPhoto primaries with the sRGB transfer function (DNG RGBTone) explains the change to within 0.0015 MAE. That is the 8-bit quantization level of the comparison, so these sliders are global tone curves in that domain. The same curve fits every photo for Blacks and negative Whites. Contrast pivots at a photo-dependent point (0.41–0.51), and positive Whites stretches further on photos whose highlights are dim. Highlights, Shadows, Clarity and Dehaze leave spatial residuals and are local operators.

## Black point

Adobe's rendering subtracts a small black level before the tone curve: the DNG SDK exposure ramp, whose default Shadows setting maps to a black point with a quadratic toe. Without it, RAWmakase's deep shadows (display luminance 0.02–0.08) were 0.4–0.55 EV brighter than Camera Raw on every camera. Engine 4 applies the ramp after exposure, with the black at 0.0015 × 2^exposure in scene-linear units. That value was fitted to the Camera Raw defaults of three photos; the SDK's nominal 0.005 crushes shadows by about 1.2 EV here. Scorecards: X100F Lightroom references 0.0120 → 0.0096 MAE, Sony Camera Raw references 0.0088 → 0.0065.

## Engine 4 implementation

`src/develop/basic_tone.rs` applies Contrast → Whites → Blacks as one composed curve after the camera profile's tone curve and before the user's point curve. The curves are the measured averages in `basic_tone_data.rs`, interpolated between slider positions with 0 as the identity. They replace the earlier power-S contrast and luminance-weighted Whites/Blacks for engine 4. Older recipes keep their operators.

Extra MAE over the default render, averaged across the five photos (previous operators in parentheses):

| Slider | −100 | −50 | −25 | +25 | +50 | +100 |
|---|---:|---:|---:|---:|---:|---:|
| Contrast | −0.0009 | −0.0004 (+0.0107) | −0.0001 | +0.0001 | +0.0002 (+0.0061) | +0.0002 |
| Blacks | −0.0008 | +0.0007 (+0.0189) | +0.0007 | −0.0005 | −0.0004 (+0.0009) | +0.0005 |
| Whites | −0.0011 | −0.0010 (+0.0031) | −0.0005 | +0.0019 | +0.0091 (+0.0152) | +0.0577 |

### Shadows and Highlights

Offline fits on the sweeps show both are local operators whose effect is best explained in log luminance of the toned image. The base level is a guided filter (radius 3.2% of the long edge, ε = 1.5 in log2 units squared), with the gain measured as a function of that base level relative to an image key. The key is the 99th luminance percentile for Shadows and the median for Highlights. `src/develop/local_tone.rs` computes the base level on a 512 px copy of the photo, so tiles, 100% regions and previews agree, and applies the measured tables in `local_tone_data.rs` after the profile tone curve.

| Slider | −100 | −60 | −30 | +30 | +60 | +100 |
|---|---:|---:|---:|---:|---:|---:|
| Shadows | +0.0025 | +0.0014 (+0.0343) | +0.0007 | +0.0006 | +0.0037 (+0.0389) | +0.0102 |
| Highlights | +0.0060 | +0.0025 (+0.0014) | +0.0000 | +0.0014 | +0.0032 (+0.0054) | — |

### Dehaze

Dehaze is mostly a per-photo tone curve with a spatial residual. A single curve per photo explains ±40 to 0.011–0.019 MAE (from 0.04–0.11 unchanged). Engine 4 applies the curve averaged across photos, before Contrast in the same composed curve. Extra MAE over the default render: +0.0078 / +0.0135 at +40 / −40 (previously +0.037 / +0.062), +0.0021 / +0.0045 at ±20, and +0.036 / +0.044 at ±100, where the per-photo adaptation dominates.

## Auto

The Basic panel's **Auto** (the button at the top of the Basic panel, beside the B&W toggle, or Cmd/Ctrl+Shift+U) sets the six Tone sliders and keeps white balance, including a manual one, as Lightroom's does; **Auto** in the WB menu sets white balance alone, and the menu shows Auto while the photo keeps that result. `rawmakase render --auto` applies Auto tone from the command line (it sets Exposure, so it does not combine with `--exposure`), and `--auto-wb` applies Auto white balance first. The implementation is `src/develop/auto.rs`; it keeps every other setting, and the app runs it off the UI thread and records one History step. Auto is greyed out (and its shortcut does nothing) while the photo is exactly as Auto left it; any edit, undoing it or opening another photo turns it back on.

- **White balance** starts from As Shot. Each of three passes averages the camera pixels that look nearly neutral under the previous estimate, with a tighter tolerance each time (normalised chromaticity distance 0.5, 0.25, 0.12), so a large coloured surface pulls less than in plain gray world. Plain gray world is the fallback when under 2% of the photo qualifies. Pixels near clipping or in the noise floor are ignored, and each channel's correction is limited to two stops.
- **Tone** is fitted by rendering a 256 px copy of the photo through the normal pipeline and measuring the display-encoded result, so the estimate follows the sliders as they actually render. Exposure puts the median luminance at 0.46 (18% gray), found by the secant method, then gives up at most 1 EV of that while it would clip more than 1% of pixels beyond those already clipped at Exposure 0 (highlights clipped in the camera clip at any exposure). Highlights (up to −60) and Shadows (up to +50) scale with the 97th and 5th luminance percentiles, and Contrast (−20 to +25) with the interquartile spread. Whites places the brightest channel of the brightest 0.2% at 0.97 and Blacks the darkest 0.2% at 0.015, each solved against renders; they are limited to −50…+35 and −50…+20 because strong positive Whites is not yet image-adaptive (below) and lifted Blacks looks flat. When highlights are clipped in the camera, Whites stays at 0 and Highlights does the recovery; likewise, when shadows are crushed to black and even the highest Blacks cannot reach the target, Blacks stays at 0.

A photo takes 10–20 small renders; on 45-megapixel Nikon Z files that was 0.2–0.6 s with 8 CPU threads, including reducing the photo. The targets were tuned by eye on Nikon Z photos and have not been measured against Lightroom's Auto values.

## Remaining

- Positive Whites needs the image-adaptive white point. The table is a median, which is poor on photos with dim highlights at +100.
- Contrast's photo-dependent pivot is not yet modeled. The averaged curve already matches within about 0.005.
- Clarity and Texture still use the earlier operators. Dehaze at ±100 needs its per-photo adaptation (airlight estimate) and spatial component.
