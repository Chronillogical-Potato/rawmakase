# Rendering engine 3

Engine 3 was the default for new photos before engine 4 ([color pipeline](color-pipeline.md)), and is what edits saved by those releases still render with. This page describes engine 3. Engine 4 kept its camera profiles, point curves, highlight reconstruction and detail processing, and replaced the Basic tone sliders, Shadows/Highlights, the color mixer, Saturation/Vibrance and color grading with responses measured against Camera Raw: see [tone controls](tone-controls.md) and [color mixer and grading](color-mixer.md) for what renders today, and treat the tone and color sections below as engine 3 only. Saved recipes carry their engine (schema/pipeline 6 today; see [imported Lightroom profiles](lightroom-profiles.md) for the embedded profile data), and saving preserves it; schema 1/2 recipes load with engine 2 and their original sharpening and color behavior. **Calibration → Process → Update** moves an older edit to the current engine as one undoable change. Older binaries reject newer files rather than misreading them.

See [preview performance](preview-performance.md) for how previews are rendered today, timing measurements and supported-hardware validation.

## Preview and detail

There is no reduced interactive draft: every change renders the real pipeline at viewport size in physical pixels (including display scale), from a resolution pyramid for Fit. At 100%, an edit or pan whose full-resolution region takes more than a moment publishes a reduced render of the region first, labeled **Draft • refining**, and the full-resolution region replaces it; fast renders and regions still cached go straight to full resolution. A resize schedules a new render; superseded jobs are canceled and stale results cannot replace current edits ([architecture](architecture.md#preview-compute-backend)).

Engine 3 applies luminance-only Gaussian unsharp masking before resizing, with amount, radius, detail sensitivity and edge masking. Defaults: amount 0.35, radius 0.8 sensor-output pixels, detail 0.25, masking 0.35. These are RAWmakase defaults, not Adobe-equivalent numbers. Since the measured sharpening (`Recipe::sharpening_model`, `develop/sharpening.rs`), new edits and Lightroom imports start at Lightroom Classic's raw defaults (Amount 40, Radius 1.0, Detail 25, Masking 0; every raw and DNG import in the catalogue sampled, process versions 2012 to 6) and render with an unsharp mask fitted to Camera Raw 18.7 on a synthetic chart (slanted edges, sine gratings of 0.04 to 0.3 cycles per pixel, texture, noisy flats; 45 renders). It is about 2.4 times the original's strength on fine detail per Amount, shapes the high-pass as `d / (1 + (|d|/halo)³)` so strong edges get small halos, makes dark halos 0.57 of light ones, scales the strength with Detail (0.3× at 0, 2.1× at 100) and fades out detail below a Masking threshold. Saved recipes keep the original operator; a Lightroom XMP or preset that sets a Sharpening value switches a recipe to the measured one, and the Detail panel's reset gives the Lightroom defaults with it. Camera Raw's ringing at Detail above 50 and its gentler sharpening of noise in deep shadows are not modelled. Full-resolution region rendering includes the required sharpening halo, and its pixels match the full render. No separate Fit-only sharpening changes the result. Noise reduction remains the existing simple edge-aware filter.

## Tone and highlights

New profile-based recipes also store a camera exposure baseline independently of the Exposure slider. The verified X100F DR100 reference baseline is +0.15 EV; unverified camera/DR combinations use zero. New reference point curves use a natural cubic spline and the sRGB transfer function in ProPhoto primaries, with a hue-preserving master pass followed by independent RGB curves. Clipped input endpoints and 0–255 point editing are supported. Legacy wide-gamut curves retain their gamma-2.2/monotone behavior. Contrast preserves endpoints. Stored recipes retain the previous curve domain and baseline until explicitly changed. See the comparison report for reference-derived metadata and limits.

The matrix-only and legacy profile paths use a scene-luminance shoulder. New profile-based edits bypass that shoulder and use the profile tone curve; applying both compressed tones twice. Spatial shadow/highlight adjustments derive gains from guided 16/64-pixel log-luminance neighborhoods to retain local detail. White/black range controls, perceptual color controls, levels and editable curve follow the shared pipeline.

Highlight reconstruction estimates clipped camera channels from nearby unclipped channel ratios, before color conversion. It retains unclipped channels and blends in recovery near the sensor ceiling. With no usable color evidence, it falls back to neutral; it cannot recreate fully clipped texture. The recovered image is lazily cached per decoded image. Extreme WB changes after demosaicing still have limitations.

Master point curves still have a saturated-color residual. Parametric curve controls remain an approximation. Generated-ramp regression data checks neutral master and independent RGB curves separately from RAW/profile differences; see `tests/data/README.md`. Exact Lightroom rendering parity is **not** established.

## Color mixer and grading

New edits and Lightroom imports enable `reference_color`: warm-hue protection in vibrance, bounded HSL luminance, and RGB-hue grading in gamma-2.2 ProPhoto. Range grading and the global finishing pass use separate weights; hue is not an Oklab angle. Missing `reference_color` preserves stored edits. **Reference color rendering** in the grading panel opts older edits in. Legacy split-tone XMP imports restore blending 100 and clear modern grading controls. These algorithms are independently approximated and calibrated on a small reference set; the [color validation table](macos-lightroom-validation.md) includes both improvements and regressions.

## Camera calibration

New edits and calibration-bearing Lightroom imports enable `reference_calibration`: neutral-preserving primary Hue/Saturation matrices plus asymmetric green/magenta shadow tint. The Calibration panel groups Shadows, Red Primary, Green Primary and Blue Primary, using −100–100 values. Older recipes keep their previous calibration unless explicitly enabled. These coefficients were measured on X100F/Adobe Standard; other cameras/profiles remain unverified. Calibration does not modify the installed DCP.

## Camera profiles

DCP ColorMatrix1/2 now support DNG temperature/tint and camera-neutral inversion. Camera calibration is signature-matched; missing color matrices fall back to the original WB approximation. Profile matrices, signatures and rendering parameters are embedded in saved recipes.

Engine 3 supports a bounded DCP subset: three-channel forward matrices, one or two standard illuminants, reciprocal-temperature interpolation, interpolated HSV calibration tables, linear/sRGB-indexed look tables, profile tone curves and baseline exposure offsets. With Profile tone rendering enabled, the profile curve processes the low/high channels and interpolates the middle channel to preserve hue. DCPs without an embedded curve use the Adobe DNG SDK default curve. The profile tone stage clips to the SDR domain. Legacy saved recipes retain their earlier value-scaling behavior until explicitly changed. This is an independent implementation, not exact Adobe processing-order parity.

No Adobe profiles are bundled. New edits prefer a matching imported Adobe Color, then Adobe Standard profile, then a DNG's embedded profile; without those, every camera uses RAWmakase Color, our own look over its LibRaw matrix (see [RAWmakase profiles](lightroom-profiles.md#rawmakase-profiles)). Published third-party DCPs, such as RawTherapee's, can be imported like any other profile. Profiles for another model are rejected, never substituted.

The **Profile** selector lists compatible DCPs in the local `camera-profiles` library. **Import profile…** loads a matching DCP file. Profile data and copyright are embedded into JSON recipes, so moving the profile file doesn't change an existing edit. Histories share immutable profile data. Matrix-only DCPs, HDR/triple-illuminant profiles and profiles forbidding embedding are rejected with an explanation. Compatible enhanced XMP look profiles can be explicitly imported together with their base DCP; see [supported look features and comparisons](lightroom-profiles.md). Profiles alone cannot reproduce Lightroom's demosaicing, local tone operators or detail algorithms.

Specification used: Adobe Digital Negative Specification 1.7.1.0, camera profile tags and chapter 6:
https://helpx.adobe.com/content/dam/help/en/photoshop/pdf/DNG_Spec_1_7_1_0.pdf

## Reference comparison

Export the same RAW from Lightroom at full resolution in sRGB, with matching orientation, WB, exposure, crop, and lens corrections. Use 16-bit TIFF for the least quantization loss. Then run:

```
rawmakase compare source.ARW lightroom.tiff /tmp/sony-comparison --origin 2400 1200
```

The output directory must not already exist. `--recipe preset.json` optionally supplies RAWmakase edits; without it the current engine defaults are used, independently of sidecars. The tool writes unscaled 512-pixel crops, a 4× difference visualization, the exact recipe, and full-image sRGB MAE/RMSE/PSNR. These metrics measure differences, not quality, and do not compensate for exposure or registration. A reference with different dimensions is rejected rather than stretched.

Five X100F Lightroom comparisons across three photographs were checked on macOS, including custom white balance, master/RGB point curves and nonzero tone sliders. The corrected profile tone substantially reduced preview error, but full Lightroom parity is not established. See [macOS and Lightroom validation](macos-lightroom-validation.md) for measurements and limitations. The supplied private archives installed 62 matching camera profiles and 603 Settings presets in the user data directory; none are bundled. See [XMP support](xmp-presets.md).

CLI profile selection: `rawmakase render source.RAF output.jpg --profile /path/to/profile.dcp`. Profile libraries are read from `$RAWMAKASE_DATA_DIR/camera-profiles` and the platform RAWmakase data directory (`~/Library/Application Support/RAWmakase` on macOS, XDG on Linux); camera matching is mandatory.
