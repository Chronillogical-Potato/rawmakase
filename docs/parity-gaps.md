# Remaining Lightroom parity gaps

Status: 2026-09-26, engine 4. RAWmakase renders close to Lightroom but not identically. Each item below says what differs and, where measured, by how much. Errors are encoded-sRGB mean absolute error (0–1) against Camera Raw 18.6 or Lightroom Classic 15.5 exports, scored with `scripts/lightroom-scorecard.py`. For scale: Lightroom X100F references now average 0.0090, Sony A7 II Camera Raw references 0.0065. Details: [tone controls](tone-controls.md), [color mixer and grading](color-mixer.md), [lens corrections](lens-corrections.md), [transform](transform.md).

## Measured and matched (for reference)

These controls were fitted to Camera Raw renders and match within the default-render error, or close to it: default look without Adobe files (DNG ColorMatrix + ACR tone curve), exposure, black point, Contrast, Blacks, Whites (negative), Shadows, Highlights, Dehaze (±40), color mixer, Saturation, Vibrance, color grading at default Blending/Balance, Transform sliders, built-in Fujifilm/Sony and DNG lens corrections, imported Adobe lens profiles, DNG embedded profile/exposure/crop, and Fujifilm default crop.

## Tone

- **Whites above about +50** adapt to the photo's highlights in Camera Raw. RAWmakase uses a median curve: extra error +0.009 at +50 and +0.058 at +100 on dim-highlight photos.
- **Contrast pivot** moves with the photo in Camera Raw (0.41–0.51 of the range). RAWmakase uses the averaged curve, within about 0.005.
- **Dehaze at ±100** adapts per photo and has a spatial part. RAWmakase uses one averaged curve: extra error +0.036/+0.044, while ±40 is within +0.014.
- **Shadows +100 / Highlights −100** reach +0.010/+0.006 extra error, because the strength also adapts per photo.
- **Clarity** changes luminance only, but single- and multi-scale local models reproduce only about a third of it: +50 leaves about 0.019 unexplained. The earlier operator is still used. **Texture** is small (+0.001) and unchanged.
- **Parametric tone curve** (Highlights/Lights/Darks/Shadows regions) is not measured. **Point curves** match on ramps, but saturated colors under an S master curve still differ (ramp MAE 0.0024, peak 0.11).
- **Curve Refine Saturation** other than 100 is unsupported and reported on import.
- **Black point** level (0.0015) is fitted, not taken from Adobe; very deep shadows on some photos remain +0.17 EV.
- **Auto** (white balance and tone) is RAWmakase's own estimate, not Adobe's. It has been checked by eye on Nikon photos only and not yet compared with Lightroom's Auto values; see [tone controls](tone-controls.md#auto).

## Color

- **Color mixer bands with little test data:** blue and purple were barely present in the nine sweep photos, so those bands are the least reliable. Measured on photos from three cameras with Adobe Standard; other profiles are untested.
- **Color mixer slider positions** between the measured extremes are interpolated. Negative saturation scales linearly (16ff805; before that, Blue −25 removed twice Camera Raw's color, e.g. DSCF8200). Hue, luminance and positive saturation scale the ±100 changes linearly, which is not verified at intermediate values. Several bands combine by adding their changes, which is not verified either.
- **Color grading at non-default Blending or Balance** still uses the earlier operator (+0.015 to +0.034 extra error on the sweeps). This includes legacy split-toning records, which imply Blending 100. Grading hues between the six measured ones and Saturation above 50 are interpolated: shadows at H30/S100 leave about 0.017.
- **Point Color** and its range controls are not implemented.
- **Camera Calibration** sliders (primaries, shadow tint) are earlier approximations, validated on two X100F photos only.
- **Camera exposure offsets** are known for X100F DR100 (from Adobe's DNG) and measured for Sony A7 II and A7CR (0.3 EV). Other cameras use 0 unless the file is a DNG. X100F renders are still about 0.04 EV brighter in midtones.

## RAW processing and profiles

- **Demosaic, highlight reconstruction, noise reduction and sharpening** are not Adobe's algorithms. At 100% the detail error is about 0.007 on X100F. X-Trans uses 1-pass Markesteijn, which measures the same as 3-pass.
- **DCP support** is a bounded subset. Triple-illuminant, HDR and other unsupported profile structures are rejected. Enhanced XMP looks (Adobe Color etc.) are supported; creative RGB-table profiles, adaptive/AI profiles and profile Amount other than 100 are not.
- **RAWmakase Color** is our own look and is not meant to match Adobe Color exactly. It hasn't yet been compared with Camera Raw renders; built-in presets made with Adobe Standard render with RAWmakase Standard (the camera matrix) when Adobe Standard isn't imported.
- **White balance** at extreme values, and the exact order of profile, WB and calibration, are not verified.
- **Other cameras** (Canon, Nikon, Panasonic, …) render through the same generic path but have not been compared with Lightroom, for lack of sample files.

## Lens corrections

- **Fujifilm built-in vignetting** is applied at 85% log strength to match Lightroom (fitted on three X100F photos).
- **Sony built-in corrections** are read but off by default: Lightroom uses them only with profile corrections on. With an imported Adobe profile, Sony matches Camera Raw within ±0.02 EV in the corners.
- **LCP** interpolation uses the farthest focus distance, since focus distance is not read from the files. Tangential distortion terms and off-centre optical centres are ignored.
- **DNG GainMap opcodes** (phone lens shading) are not applied.
- **Manual lens vignetting, Defringe and Remove Chromatic Aberration** are not measured against Camera Raw.

## Geometry

- **Upright** (Auto, Level, Vertical, Full, Guided) is not implemented; XMP files that use it are rejected.
- The order in which Transform sliders compose was not measured separately. Aspect ±50 shows a slightly higher error than the other sliders.

## Local and finishing adjustments

- **Spot removal and masks are experimental and early** ([retouching](retouching.md), [masking](masking.md)). Heal and Clone, and brush, linear, radial, color range and luminance range masks with local sliders render, but none of it is measured against Camera Raw:
  - Heal's algorithm, feather profile and automatic source choice are our own; results look alike but are not compared numerically.
  - Local Contrast, Highlights, Shadows, Whites, Blacks and Dehaze reuse the measured global responses. Local Temp, Tint, Hue, Saturation, Color, Texture, Clarity, Sharpness and Noise are approximations; Noise only reduces noise, and local Moiré, Defringe, Grain and tone curves are not implemented.
  - Gradient transition and brush feather profiles, Flow build-up, Auto Mask edges and range-mask Refine/Smoothness are approximations.
  - AI selections (Subject, Sky, Background, Objects, People, Depth), the AI Remove mode, AI Denoise, Enhance and Lens Blur are not implemented.
- **Lightroom import of spots and masks:** positions (default crop, unrotated sensor frame), long-edge sizes and spot sources were checked exactly against Camera Raw renders. Gradient Full/Zero points, the radial `Flipped` flag (read as inside the ellipse; not flipped applies outside, as the old Radial Filter default), the radial angle's sign, mask blend modes (0 add, 1 subtract, 2 intersect), brush feather from `CenterWeight`, local Hue's scale and legacy range-mask feathering are assumptions: Camera Raw ignored the test files for those. Color range masks and AI masks are reported and left out.
- Post-crop vignetting, grain, Glow and Reshape are not measured. Glow and Reshape are rejected when non-zero.
- HDR editing and output are not implemented.

## Catalog and interaction

- The RAWmakase catalog is separate from Lightroom's, with no write-back or sync. Custom color-label text is kept, but custom labels display white. Multi-photo metadata edits and metadata undo are missing.
- Unsupported develop settings are kept and reported, but not rendered.

## Validation still needed

- More cameras, profiles (Adobe Color and other looks), illuminants and clipped highlights.
- Combinations of sliders: every measurement above varies one slider at a time, and composition order is assumed.
- A repeatable test set that isn't private photos; Piotr plans to design the testing pipeline.
