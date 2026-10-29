# Rendering engine

RAWmakase renders every edit with one engine, whichever release saved it. Exposure and the Basic panel's tone controls (Whites, Blacks, Highlights, Shadows, Clarity, Texture and Dehaze) render on scene values before the camera profile's look and tone curve, where Camera Raw 18.7 applies them: the [scene tone stage](scene-tone-stage.md). Contrast, the parametric curve, the color mixer, Saturation/Vibrance, color grading, calibration, sharpening, colour noise reduction, grain and manual lens vignetting are responses measured against Camera Raw too: see [tone controls](tone-controls.md), [color mixer and grading](color-mixer.md) and [lens corrections](lens-corrections.md). Earlier engines and the earlier operators of each control are no longer rendered: an edit saved with them renders with the current engine, and the settings that chose them are dropped when it is read ([color pipeline](color-pipeline.md)). Saved recipes are schema/pipeline 11 today (see [imported Lightroom profiles](lightroom-profiles.md) for the embedded profile data). Older binaries reject newer files rather than misreading them.

See [preview performance](preview-performance.md) for how previews are rendered today, timing measurements and supported-hardware validation.

## Preview and detail

Every change renders the real pipeline at viewport size in physical pixels (including display scale), from a resolution pyramid for Fit and zoomed-out views. When a render takes more than a moment, an edit publishes a reduced render first, labeled **Draft • refining**, and the full render replaces it: at 100% a reduced render of the region (also when panning), and for the whole photo one with at most a megapixel. Fast renders, and views still cached, go straight to full resolution. While an edit continues, the running render finishes and shows before the next one starts; a new photo, view or size cancels it, and an older result never replaces a newer one ([architecture](architecture.md#preview-compute-backend)).

Sharpening (`develop/sharpening.rs`) is applied before resizing. New edits and Lightroom imports start at Lightroom Classic's raw defaults (Amount 40, Radius 1.0, Detail 25, Masking 0; every raw and DNG import in the catalogue sampled, process versions 2012 to 6) and render with an unsharp mask fitted to Camera Raw 18.7 on a synthetic chart (slanted edges, sine gratings of 0.04 to 0.3 cycles per pixel, texture, noisy flats; 45 renders). It is about 2.4 times the strength of RAWmakase's earlier operator (a luminance-only Gaussian unsharp mask, no longer rendered) on fine detail per Amount, shapes the high-pass as `d / (1 + (|d|/halo)³)` so strong edges get small halos, makes dark halos 0.57 of light ones, scales the strength with Detail (0.3× at 0, 2.1× at 100) and fades out detail below a Masking threshold. The Detail panel's reset gives the Lightroom defaults. Camera Raw's ringing at Detail above 50 and its gentler sharpening of noise in deep shadows are not modelled. Full-resolution region rendering includes the required sharpening halo, and its pixels match the full render. No separate Fit-only sharpening changes the result. Luminance noise reduction is described below.

Color noise reduction (`develop/color_noise.rs`) starts at Lightroom's raw default of 25 (Detail and Smoothness 50) and follows Camera Raw 18.7 as it renders photos, whose noise it knows from the camera's noise profile. The chroma of the camera image, in square-root values (where shot noise is about the same at every brightness), is split into a Laplacian pyramid. At each level the colour noise is measured in the photo itself: the RMS of the level's detail in its smoothest 10% of 8×8 blocks, leaving out blocks near the sensor's clipping and, unless too few others remain (a dark, noisy photo), blocks with black pixels; a coarser level never counts more noise than the finer one, and a level with fewer than 16 such blocks takes 0.6 of the finer level's. Each level's detail is then kept by the sixth power of its size over a threshold, a multiple of that noise set by Amount, Detail and Smoothness, so noise goes and strong colour edges stay sharp. On synthetic RGGB charts with the noise of a Nikon D7100 at ISO 2000 (from issue 465) and 1/16, 1/4 and 4 times it, each carrying its DNG noise profile, Camera Raw keeps the same share of the noise at each scale whatever its level, and at Amount 25 takes out about 95% of it at every scale; it keeps a little more of a clean photo's coarse colour, which the thresholds follow as a power of the finest level's noise. Fitted to those renders (Amount 10–100, Detail and Smoothness 0–100), the share of noise kept at each of four scales differs from Camera Raw's by a mean |log ratio| of 0.13–0.24 over the 80 cases, against 1.4–2.0 for the operator fitted earlier to charts without a noise profile, where Camera Raw assumes little noise (`scripts/corpus/color-noise.py`). On the D7100 photo the noise measured at the finest level is 0.0086, against 0.0083 predicted from the noise profile Adobe's DNG Converter writes for it. It runs once on the camera image before everything else, as spot removal does. The Detail panel's reset gives Lightroom's default. Edits from earlier releases render with it too; the operators they were saved with are not kept.

Luminance noise reduction (`develop/luminance_noise.rs`) runs on the full camera
image after Color noise reduction, before retouching, the preview pyramid and tone
mapping. A five-level Laplacian pyramid of square-root luminance separates fine
grain from multi-pixel blotches. Each band estimates its noise from the tenth
percentile of 8×8 block energies, excluding near-clipped and mostly black blocks;
small levels fall back to the finer band's estimate. Amount raises a soft threshold,
Detail protects fine structure and Contrast protects coarser structure. A band's
3×3 neighborhood energy distinguishes coherent detail from isolated noise specks. Color Detail
does not control luminance reduction. The result preserves square-root chroma.
Both amounts at zero bypass the stage; saved slider values and XMP units are
unchanged. The preview cache keys the source and all noise settings, and CPU and
GPU rendering sample the same denoised image. This is an approximation, not Adobe's
algorithm: texture-only scenes can overestimate noise, and high amounts can lose
faint detail.

## Tone and highlights

Profile-based recipes also store a camera exposure baseline independently of the Exposure slider. The verified X100F DR100 reference baseline is +0.15 EV; unverified camera/DR combinations use zero. Point curves use a natural cubic spline and the sRGB transfer function in ProPhoto primaries, with a hue-preserving master pass followed by independent RGB curves. Clipped input endpoints and 0–255 point editing are supported. Contrast preserves endpoints. See the comparison report for reference-derived metadata and limits.

Every resolved recipe has a camera profile (without an imported or embedded one, the camera matrix with the DNG default tone curve) and uses the profile tone curve. Before it, the scene tone stage keeps scene values above 1 and rolls them off to a white point measured from the sensor's white and the photo's maximum, as Camera Raw's default render does; Shadows and Highlights are a measured local operator there ([tone controls](tone-controls.md#shadows-and-highlights)).

Highlight reconstruction follows Camera Raw 18.7's measured rule in white-balanced camera space, before color conversion ([partially clipped highlights](#partially-clipped-highlights)): every channel is pulled up towards the mean of the other two by a weight that rises with its own sensor level, so a clipped channel becomes that mean and a channel near the sensor's white mostly follows the others. It cannot recreate fully clipped texture. The recovered image is lazily cached per decoded image. Extreme WB changes after demosaicing still have limitations.

Master point curves still have a saturated-color residual. The parametric curve is measured ([tone controls](tone-controls.md#parametric-curve)). Generated-ramp regression data checks neutral master and independent RGB curves separately from RAW/profile differences; see `tests/data/README.md`. Exact Lightroom rendering parity is **not** established.

## Color mixer and grading

The color mixer, Saturation, Vibrance, color grading and Point Color render Camera Raw 18.7's measured responses: see [color mixer and grading](color-mixer.md). Legacy split-tone XMP imports restore blending 100 and clear modern grading controls. The earlier approximations, and the `reference_color` choice between them, are no longer rendered; the [color validation table](macos-lightroom-validation.md) records how they compared.

## Camera calibration

Calibration renders neutral-preserving primary Hue/Saturation matrices plus an asymmetric green/magenta shadow tint. The Calibration panel groups Shadows, Red Primary, Green Primary and Blue Primary, using −100–100 values; its Process row shows the current version, the only one. Calibration does not modify the installed DCP.

The primary sliders render Camera Raw 18.7's measured changes (in `calibration.rs`), replacing the original coefficients, measured on X100F/Adobe Standard. Camera Raw rendered a dense synthetic chart (1,728 colors and a gray ramp) with each primary's Hue and Saturation at −100, −50, +50 and +100. For each position, the six numbers of a neutral-preserving matrix at RAWmakase's calibration step were fitted by rendering the same chart through RAWmakase's own pipeline (Gauss–Newton on CIELAB differences), and positions in between are interpolated. Camera Raw leaves grays alone under every calibration slider, as the matrices do. On the fitting chart the 24 renders went from mean ΔE00 1.0–3.4 to 0.66–0.80, against 0.80 for the default render there; on the corpus chart the `calibration-*` cases went from 1.11–2.02 to 0.62–0.68 (default 0.66), and calibration combined with the color mixer from 1.55 to 0.66. Several sliders add their changes, which was checked only on that pair. Shadow Tint keeps its earlier fit (0.71 and 0.91 on the chart). The chart is one synthetic camera with its embedded profile; other cameras and profiles are not measured.

## Partially clipped highlights

Highlight recovery runs once on the full decoded camera image, before the scene
tone controls, and feeds Fit, regions and export. It is Camera Raw 18.7's rule as
measured on flat patches rendered through a linear profile (`scripts/corpus/probe_dng.py`
with an As Shot neutral, so channels clip at different scene levels): in
white-balanced camera space each channel is pulled up towards the mean of the other
two, never down, by the weight `((s − 0.25) / 0.75)^1.8`, where `s` is the channel's
own fraction of the sensor's white. A clipped channel (`s` = 1) becomes the mean of the
other two outright; a channel at 0.9 of its white follows them by 0.77 of the gap, one
at half by 0.14, one in the lower quarter not at all. The
three pulls are solved together (each channel's target is the others' pulled values,
anchored at what the sensor recorded), so with two channels clipped the lower one lands
between the unclipped channel and the higher clip, and once the unclipped channel passes
the higher clip the pixel is neutral at that level, as Camera Raw renders it. Nothing
reads neighbouring pixels: on flat patches Camera Raw's result is exactly this per-pixel
rule, and a clipped sky keeps whatever gradient its unclipped channels carry.

Measured on 136 patches (six hue directions at two saturations and six levels of the
sensor's white, all unclipped; four hues at eight levels through every clip, under two
As Shot neutrals), the rule reproduces Camera Raw within 0.005 RMS of the level per
channel, the shoulder both renderers apply above white excluded. It replaces the
neighbourhood-ratio estimate and the connected-area colour map of earlier releases: those
reconstructed the patch's true colour (a warm patch with green clipped stayed warm,
where Camera Raw moves it towards neutral and brighter), which is why clipped skies
rendered duller and pinker than Camera Raw's. The rule also changes unclipped colours
whose channels approach the white, as Camera Raw does: a saturated red at 0.9 of its
white loses a third of its chroma. This infers Camera Raw's colour, not missing detail,
and is not Adobe's algorithm. Cancellation discards the recovered image.

## Camera profiles

DCP ColorMatrix1/2 now support DNG temperature/tint and camera-neutral inversion. Camera calibration is signature-matched; missing color matrices fall back to the original WB approximation. Profile matrices, signatures and rendering parameters are embedded in saved recipes.

RAWmakase supports a bounded DCP subset: three-channel forward matrices, one or two standard illuminants, reciprocal-temperature interpolation, interpolated HSV calibration tables, linear/sRGB-indexed look tables, profile tone curves and baseline exposure offsets. The profile curve processes the low/high channels and interpolates the middle channel to preserve hue. DCPs without an embedded curve use the Adobe DNG SDK default curve. The profile tone stage clips to the SDR domain. This is an independent implementation, not exact Adobe processing-order parity.

No Adobe profiles are bundled. New edits prefer a matching imported Adobe Color, then Adobe Standard profile, then a DNG's embedded profile; without those, every camera uses RAWmakase Color, our own look over its camera matrix (see [RAWmakase profiles](lightroom-profiles.md#rawmakase-profiles)). Published third-party DCPs, such as RawTherapee's, can be imported like any other profile. Profiles for another model are rejected, never substituted.

The **Profile** selector lists compatible DCPs in the local `camera-profiles` library. Each photo opened lists them again; the camera each DCP was made for is remembered while the app runs, keyed by the file's size, modification time and (on Unix) status change time, so after the first scan only the photo's camera's profiles are read. **Import profile…** loads a matching DCP file. Profile data and copyright are embedded into JSON recipes, so moving the profile file doesn't change an existing edit. Histories share immutable profile data. Matrix-only DCPs, HDR/triple-illuminant profiles and profiles forbidding embedding are rejected with an explanation. Compatible enhanced XMP look profiles can be explicitly imported together with their base DCP; see [supported look features and comparisons](lightroom-profiles.md). Profiles alone cannot reproduce Lightroom's demosaicing, local tone operators or detail algorithms.

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
