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

The Basic panel's **Auto** (the button at the top of the Basic panel, beside the B&W toggle, or Cmd/Ctrl+Shift+U) sets the six Tone sliders and Vibrance, and keeps white balance, including a manual one, as Lightroom's does; **Auto** in the WB menu sets white balance alone, and the menu shows Auto while the photo keeps that result. `rawmakase render --auto` applies Auto tone from the command line (it sets Exposure, so it does not combine with `--exposure`), and `--auto-wb` applies Auto white balance first. The implementation is `src/develop/auto.rs`; it keeps every other setting, and the app runs it off the UI thread and records one History step. Like Lightroom's, Auto tone measures the photo before its adjustments: as the profile, white balance, calibration, lens corrections and crop render it, without the tone sliders, curves and Levels, presence, color mixer, B&W, grading, detail, effects, spots and masks. So a film-look curve that lifts the blacks, or a color edit, does not change what Auto chooses. In a Lightroom catalog's history, photos whose point curve lifted black to 30/255 or more still got ordinary Auto Blacks (median −10, as against −17 without such a curve), which measuring through the curve could not give. Like Lightroom's, it also sets Vibrance (below). Auto is greyed out (and its shortcut does nothing) while running it again would change nothing: the six Tone sliders and Vibrance are as Auto set them and nothing Auto measures (profile, white balance, calibration, lens corrections, crop) has changed. Moving a Tone slider or Vibrance, changing one of those, undoing Auto or opening another photo turns it back on; adjustments Auto ignores, such as a curve, Clarity or the color mixer, leave it greyed out.

- **White balance** follows Lightroom's Auto as measured: gray world (the average of the camera pixels in the crop made neutral; pixels near clipping or in the noise floor are ignored), then 23 mired warmer and 3 Tint greener, limited to 2850–7500 K and Tint 0 to +30, the range Lightroom's Auto keeps to. The camera pixels are the decoded ones, before highlight recovery invents colour. XMP presets and settings with `WhiteBalance="Auto"` and no resolved Temperature/Tint use the same estimate. On 133 photos from three cameras with Lightroom's or Camera Raw 18.6's Auto values (A7 II from a Lightroom catalog, A7CR and X100F from Camera Raw, all with Adobe Standard), the estimate is within a median of 2.7 mired of Adobe's (90% within 5.5, worst 27, on an A7CR neon night scene) and 1 Tint; the earlier near-neutral search was 24 mired off (worst 183) and averaged 20 mired cooler. The tuning photos are private.
- **Tone** is predicted from one render of the photo before its adjustments, reduced so the crop's long edge is about 1024 px, with every tone slider at 0. Lightroom's Auto behaves like a learned estimate rather than a target it solves for: it lifts a dark photo only part of the way to middle gray, holds Exposure back for bright highlights, and nearly always pulls Highlights down (median −65) and opens Shadows (+47). So each slider is a linear fit, to Lightroom Classic's own Auto values, of the one or two display-encoded percentiles of luminance (L) or of the brightest channel (P) that predicted it best on held-out photos:

  | Slider | Fit |
  |---|---|
  | Exposure | 2.22 − 2.13 L40 − 1.52 L99 (EV) |
  | Contrast | −10.8 + 54.8 L1 |
  | Highlights | −34.6 − 51.1 L90 + 27 P25 |
  | Shadows | 52 − 40.2 P10 |
  | Whites | 67.5 − 52.2 P99.8 |
  | Blacks | −35.3 − 1.91 log2(linear L1) |
  | Vibrance | +15 |

  Lightroom's Auto overwrites Vibrance and Saturation too. Vibrance is nearly constant (median +15, half of photos within ±2, all within +7…+21), so Auto sets +15 and leaves Saturation alone: Lightroom's Saturation splits between about −1 and +4 in a way no percentile or colourfulness measure predicted (a constant +2 would cut its error only from 2.9 to 2.4).

  The fit uses 253 Auto Settings steps on 246 photos from a Lightroom Classic catalog (mostly two cameras, process versions 10 and 11), comparing each step's result with Auto run on the settings just before it; steps followed directly by a preset, paste or reset are left out, as is any slider the next step changed. Every fifth photo was held out of the fit. Mean absolute error on those 49 held-out photos, before (the earlier solve-for-targets Auto) and after (the fits applied to each photo's measured render; Auto run on such a photo gives the same values):

  | Slider | Before | After |
  |---|---|---|
  | Exposure | 0.53 EV | 0.25 EV |
  | Contrast | 14.2 | 13.4 |
  | Highlights | 33.3 | 6.5 |
  | Shadows | 25.7 | 8.0 |
  | Whites | 19.7 | 12.2 |
  | Blacks | 16.3 | 4.7 |
  | Vibrance | 15.1 | 2.2 |

  On the photos it was fitted to, the errors are 0.27 EV, 11.1, 6.0, 7.6, 11.5, 4.8 and 2.0, close to the held-out ones, so the fit carries over to photos it has not seen. Contrast is barely predictable (a constant does as well), and Whites only somewhat. The earlier Auto placed the median at 18% gray, Whites and Blacks at fixed end points, and capped Highlights at −60 and Shadows at +50, which made it 32 too weak on Highlights, 21 on Shadows and 16 on Blacks on average. The tuning photos are private.

A photo takes one render of the reduced copy, after highlight recovery and the reduction itself.

## Remaining

- Positive Whites needs the image-adaptive white point. The table is a median, which is poor on photos with dim highlights at +100.
- Contrast's photo-dependent pivot is not yet modeled. The averaged curve already matches within about 0.005.
- Auto's Contrast and Saturation: Lightroom's choices did not follow any measured percentile or colourfulness, so Contrast is close to a constant and Saturation is left alone.
- Clarity and Texture still use the earlier operators. Dehaze at ±100 needs its per-photo adaptation (airlight estimate) and spatial component.
