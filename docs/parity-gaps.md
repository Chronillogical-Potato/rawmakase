# Remaining Lightroom parity gaps

Status: 2026-09-26, engine 4; lens, profile and catalog notes updated 2026-10-03. RAWmakase renders close to Lightroom but not identically. Each item below says what differs and, where measured, by how much. Errors are encoded-sRGB mean absolute error (0–1) against Camera Raw 18.6 or Lightroom Classic 15.5 exports, scored with `scripts/lightroom-scorecard.py`. For scale: Lightroom X100F references now average 0.0090, Sony A7 II Camera Raw references 0.0065. Details: [tone controls](tone-controls.md), [color mixer and grading](color-mixer.md), [lens corrections](lens-corrections.md), [transform](transform.md).

## Measured and matched (for reference)

These controls were fitted to Camera Raw renders and match within the default-render error, or close to it: default look without Adobe files (DNG ColorMatrix + ACR tone curve), exposure, black point, Contrast, Blacks, Whites (negative), Shadows, Highlights, Dehaze (±40), color mixer, Saturation, Vibrance, color grading at default Blending/Balance, Transform sliders, built-in Fujifilm and DNG lens corrections, imported Adobe lens profiles (on Sony A7 II photos), DNG embedded profile/exposure/crop, and Fujifilm default crop.

## Tone

- **Whites above about +50** adapt to the photo's highlights in Camera Raw. RAWmakase uses a median curve: extra error +0.009 at +50 and +0.058 at +100 on dim-highlight photos.
- **Contrast pivot** moves with the photo in Camera Raw (0.41–0.51 of the range). RAWmakase uses the averaged curve, within about 0.005.
- **Dehaze at ±100** adapts per photo and has a spatial part. RAWmakase uses one averaged curve: extra error +0.036/+0.044, while ±40 is within +0.014.
- **Shadows +100 / Highlights −100** reach +0.010/+0.006 extra error, because the strength also adapts per photo.
- **Clarity** changes luminance only, but single- and multi-scale local models reproduce only about a third of it: +50 leaves about 0.019 unexplained. The earlier operator is still used. **Texture** is small (+0.001) and unchanged.
- **Parametric tone curve** (Highlights/Lights/Darks/Shadows regions) is not measured. **Point curves** match on ramps, but saturated colors under an S master curve still differ (ramp MAE 0.0024, peak 0.11).
- **Curve Refine Saturation** renders as Camera Raw 18.7 does on the chart ([tone controls](tone-controls.md#refine-saturation)): the `curve-*-refine-saturation-*` cases sit at mean ΔE00 1.1–1.5, against 1.4 for the same S curve at 100. Values above 100 render as 100, as in Camera Raw. It imports, is written back, and has a Refine › Saturation slider (0–100) under the RGB point curve.
- **Black point** level (0.0015) is fitted, not taken from Adobe; very deep shadows on some photos remain +0.17 EV.
- **Auto** tone is RAWmakase's own estimate, checked by eye on Nikon photos only and not yet compared with Lightroom's Auto values. The WB menu's Auto white balance is fitted to Lightroom's Auto values (a median of 2.7 mired off on 133 photos); see [tone controls](tone-controls.md#auto).

## Color

- **Color mixer bands with little test data:** blue and purple were barely present in the nine sweep photos, so those bands are the least reliable. Measured on photos from three cameras with Adobe Standard; other profiles are untested.
- **Color mixer slider positions** between the measured extremes are interpolated. Negative saturation scales linearly (16ff805; before that, Blue −25 removed twice Camera Raw's color, e.g. DSCF8200). Hue, luminance and positive saturation scale the ±100 changes linearly, which is not verified at intermediate values. Several bands combine by adding their changes, which is not verified either.
- **Color grading at non-default Blending or Balance** still uses the earlier operator (+0.015 to +0.034 extra error on the sweeps). This includes legacy split-toning records, which imply Blending 100. Grading hues between the six measured ones and Saturation above 50 are interpolated: shadows at H30/S100 leave about 0.017.
- **Black & white mix** renders each band's change in proportion to the color's chroma, brightening more gently than it darkens, fitted to Camera Raw 18.7 on the synthetic chart ([color mixer](color-mixer.md#black--white)). Mean ΔE00 on the chart's `bw-*` cases is 1.1–2.5, down from 1.4–3.0, and neutrals no longer move with the Red and Magenta sliders. Camera Raw's strongest darkening (Blue or Yellow −60 takes a sky nearly to black) is larger than this model gives: p95 ΔE00 10–13 on those cases. A mix of zero matches Camera Raw's conversion as closely as the color render does (0.91).
- **Auto black & white mix** is fitted to Camera Raw 18.7's Auto on synthetic scenes and is within 2 slider steps of it on the chart it was not fitted to; on 30 held-out scenes it is off by 1.3 steps on average (worst band 10). Scenes of two flat colors are predicted worst (up to 30 steps off on a band). It has not been compared with Lightroom on the same real photos.
- **Point Color** and its range controls are not implemented.
- **Camera Calibration** sliders (primaries, shadow tint) are earlier approximations, validated on two X100F photos only.
- **Camera exposure offsets** are known for X100F DR100 (from Adobe's DNG) and measured for Sony A7 II and A7CR (0.3 EV). Other cameras use 0 unless the file is a DNG. X100F renders are still about 0.04 EV brighter in midtones.

## RAW processing and profiles

- **Demosaic, highlight reconstruction, noise reduction and sharpening** are not Adobe's algorithms. At 100% the detail error is about 0.007 on X100F. X-Trans uses 1-pass Markesteijn, which measures the same as 3-pass.
- **DCP support** is a bounded subset. Triple-illuminant, HDR and other unsupported profile structures are rejected. Enhanced XMP looks (Adobe Color etc.), creative HSV looks (B&W 01, 03–12, Modern 01), RGB-table looks (Artistic, Vintage, Modern, camera-matching XMPs that carry their table), and looks carrying Exposure, Saturation, colour mixer, parametric curve, split toning or vignette settings ([settings inside looks](lightroom-profiles.md#settings-inside-looks)) are supported; adaptive/AI profiles, camera-matching XMPs whose table Camera Raw keeps elsewhere (Fujifilm film simulations) and looks with settings RAWmakase doesn't apply inside a profile (B&W 02's white balance, the B&W filters' mix, the Premium film looks' colour grading) are not. RGB tables match Camera Raw 18.7 on synthetic looks within RAWmakase's default-render error ([RGB tables](lightroom-profiles.md#rgb-tables)). Profile Amount follows Camera Raw 18.7's rule, measured on synthetic looks ([Profile Amount](lightroom-profiles.md#profile-amount)); the remaining error is that of Shadows, Clarity and Contrast with Blacks, which a look's internal settings use.
- **RAWmakase Color** is our own look and is not meant to match Adobe Color exactly. It hasn't yet been compared with Camera Raw renders; built-in presets made with Adobe Standard render with RAWmakase Standard (the camera matrix) when Adobe Standard isn't imported.
- **White balance** at extreme values, and the exact order of profile, WB and calibration, are not verified.
- **Other cameras** (Canon, Nikon, Panasonic, …) render through the same generic path but have not been compared with Lightroom, for lack of sample files.

## Lens corrections

- **Fujifilm built-in vignetting** is applied at 85% log strength to match Lightroom (fitted on three X100F photos).
- **Sony built-in corrections** are available but not measured against Lightroom, and whether Lightroom uses Sony's stored data at all has not been checked. They are off by default and follow Enable Profile Corrections in the Lens Corrections panel. A Lightroom edit or preset with profile corrections on turns them on as the checkbox does: an imported Adobe profile when one matches, else Sony's built-in correction, and the import notice and the panel say the Adobe profile isn't imported. With an imported Adobe profile, Sony matches Camera Raw within ±0.02 EV in the corners ([lens corrections](lens-corrections.md#sony-built-in-corrections)).
- **LCP** interpolation uses the farthest focus distance, since focus distance is not read from the files. Tangential distortion terms and off-centre optical centres are ignored.
- **DNG GainMap opcodes** (phone lens shading) are not applied.
- **Remove Chromatic Aberration** measures lateral CA radially from the image centre; Camera Raw's own estimate is not reproduced exactly, and off-centre (decentred) CA is not corrected ([lens corrections](lens-corrections.md#remove-chromatic-aberration)).
- **Defringe** matches Camera Raw's hue ranges and strength, but not its extra reduction next to strong edges ([lens corrections](lens-corrections.md#defringe)).
- **Manual lens vignetting** is not measured against Camera Raw.
- **Manual Distortion** matches Camera Raw's radial map and order on the chart ([lens corrections](lens-corrections.md#manual-distortion)); Lens Corrections has a Distortion amount for it, and Constrain Crop crops out the white it uncovers.

## Geometry

- **Crop & Straighten** has Lightroom's aspect presets with X to swap orientation, the Straighten ruler (Cmd-drag), Auto straighten from Upright's Level analysis, and the Grid, Thirds, Diagonal, Triangle, Golden Ratio and Golden Spiral overlays (O, Shift+O). The Aspect Ratios overlay is not implemented, and Auto straighten measures the photo without its Transform ([transform](transform.md#crop-and-straighten)). Rotation and flips are not written to XMP.
- **Upright** renders imported edits from the corrections Lightroom stores, exactly. For new edits and mode-only presets RAWmakase analyses the photo itself; on 160 photos its Level is within a median 13 px of Lightroom's at 2000 px, Vertical and Auto about 60 px (mostly framing), Full about 180 px ([transform](transform.md)). Guided draws two to four guides and solves them on the photo ([transform](transform.md#guided-upright)); how close its framing comes to Lightroom's is not measured. Constrain Crop keeps the white out of the crop; its rule could not be measured in Camera Raw, which renders the stored crop as it is ([transform](transform.md#constrain-crop)).

## Local and finishing adjustments

- **Spot removal and masks are experimental and early** ([retouching](retouching.md), [masking](masking.md)). Heal and Clone, and brush, linear, radial, color range and luminance range masks with local sliders render, but none of it is measured against Camera Raw:
  - Heal's algorithm, feather profile and automatic source choice are our own; results look alike but are not compared numerically.
  - Local Contrast, Highlights, Shadows, Whites, Blacks and Dehaze reuse the measured global responses. Local Temp, Tint, Hue, Saturation, Color, Texture, Clarity, Sharpness and Noise are approximations; Noise only reduces noise, and local Moiré, Defringe, Grain and tone curves are not implemented.
  - Gradient transition and brush feather profiles, Flow build-up, Auto Mask edges and range-mask Refine/Smoothness are approximations.
  - AI selections (Subject, Sky, Background, Objects, People, Depth), the AI Remove mode, AI Denoise, Enhance and Lens Blur are not implemented.
- **Red Eye** is experimental ([retouching](retouching.md#red-eye)): its correction is fitted to Camera Raw 18.7 (mean ΔE76 1.8 on in-gamut colours and grey ramps), and Lightroom's `RedEyeInfo` imports with its measured coordinates. Camera Raw also desaturates strongly red pixels joined to the pupil, such as a red-orange iris; RAWmakase keeps to the ellipse. The pupil detection is our own. Pet Eye with Add Catchlight renders and imports too, fitted to Camera Raw's falloff and catchlight.
- **Lightroom import of spots and masks:** positions (default crop, unrotated sensor frame), long-edge sizes and spot sources were checked exactly against Camera Raw renders. Gradient Full/Zero points, the radial `Flipped` flag (read as inside the ellipse; not flipped applies outside, as the old Radial Filter default), the radial angle's sign, mask blend modes (0 add, 1 subtract, 2 intersect), brush feather from `CenterWeight`, local Hue's scale and legacy range-mask feathering are assumptions: Camera Raw ignored the test files for those. Color range masks and AI masks are reported and left out.
- **Post-crop vignetting** reads all three Lightroom styles (codes 1 Highlight Priority, 2 Color Priority, 3 Paint Overlay; Camera Raw 18.7 renders 0 and an omitted style as Highlight Priority). Highlights applies only to Highlight and Color Priority with a negative Amount, as in Lightroom. Each style has its own operator fitted to Camera Raw 18.7 (Paint Overlay blends linear output, Highlight Priority changes exposure before the tone curve, Color Priority sits between them), with Camera Raw's mask shape for Midpoint, Feather and Roundness. The `vignette-*` cases in `tests/corpus` sit at mean ΔE00 0.8–1.6 from Camera Raw (RAWmakase's default render is 0.9); Amount −100 Highlight Priority (2.0) and Highlight or Color Priority with a low Midpoint (2.8 at Midpoint 20) are further off. Highlight and Color Priority take pixels back through Camera Raw's default tone curve, so with strong tone settings their highlight response is approximate.
- Grain, Glow and Reshape are not measured. Glow and Reshape are rejected when non-zero.
- HDR editing and output are not implemented.

## Catalog and interaction

- The RAWmakase catalog is separate from Lightroom's, with no write-back or sync. Custom color-label text is kept, but custom labels display white.
- Rating, flag and label changes, adding to or taking out of the Quick Collection, and title, caption, creator, copyright, location and keyword edits apply to every photo selected in the Grid. Each is one step that Cmd+Z undoes and Cmd+Shift+Z redoes, in one undo sequence shared with Develop, as in Lightroom. That sequence is kept in memory only (up to 100 steps) and is cleared when another catalog opens.
- Copy Settings, Paste Settings, Paste from Previous and Sync Settings transfer the groups chosen in Lightroom's Copy Settings dialog, worked out for each photo's camera. Sync applies to the photos selected with the open one (Cmd or Shift in the filmstrip) in one transaction and one Undo. Settings › Match Total Exposures (Cmd+Option+Shift+M) sets the other selected photos' Exposure so their aperture, shutter speed and ISO end up as bright as the open photo, the same way. Auto Sync is missing.
- Unsupported develop settings are kept and reported, but not rendered.
- The histogram's clipping triangles work as Lightroom's (click for shadows or highlights alone, hover to preview, J for both), but its thresholds are RAWmakase's own (encoded sRGB 0.001 and 0.999, [color pipeline](color-pipeline.md#display)), not measured against Lightroom. Dragging in the histogram moves Blacks, Shadows, Exposure, Highlights or Whites, in equal fifths of its width, which Lightroom's regions only approximate.
- Raw defaults (Adobe Default, RAWmakase Default or a preset, with per-camera overrides) follow Lightroom Classic, except that photos without an edit follow the current defaults instead of having them written in at import, and Auto Tone, Auto white balance or Upright in a default preset is not applied. Lightroom's "Camera Settings" master choice is missing. See [raw defaults](xmp-presets.md#raw-defaults).

## Validation still needed

- More cameras, profiles (Adobe Color and other looks), illuminants and clipped highlights.
- Combinations of sliders: every measurement above varies one slider at a time, and composition order is assumed.
- A repeatable test set that isn't private photos; Piotr plans to design the testing pipeline.
