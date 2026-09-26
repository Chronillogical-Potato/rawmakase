# Remaining Lightroom parity gaps

Status: 2026-09-26. RAWmakase renders substantially closer to Lightroom, but does not yet produce identical results. Measurements and test conditions are recorded in [macOS validation](macos-lightroom-validation.md).

## Tone curves and global tone

- Saturated colors under the master point curve still differ. Generated-ramp MAE is 0.00240 for the tested S curve (peak 0.112) and 0.00333 for a clipped-endpoint curve, on a 0–1 scale.
- Neutral master curves and independent RGB curves closely match measured ramps, but small ICC/quantization and interpolation differences remain. Tests do not establish equivalence for every possible curve.
- Parametric Highlights, Lights, Darks and Shadows remain approximations. Their interaction with point curves and movable range boundaries needs isolated reference testing.
- Non-default Curve Refine Saturation is unsupported and reported during import.
- Basic contrast, highlights, shadows, whites and blacks use independent operators. Spatial tone response and operation order still differ, especially around bright windows and skin in shadow.
- RAW baseline differences persist even with linear user curves. Identical curve points therefore do not guarantee identical complete photographs.

## Color and camera calibration

- Primary Hue/Saturation and shadow tint are measured approximations, validated narrowly on two Fujifilm X100F photographs with Adobe Standard. Other cameras, profiles, white balances and extreme combinations need visual validation.
- Vibrance, HSL luminance, split toning and modern three-way/global grading remain approximations. Some split-tone comparisons are unchanged or slightly worse; slider-number equivalence is not guaranteed.
- Color grading range weights, balance, blending and luminance need broader reference coverage.
- Point Color, its selection/range controls, and other unsupported color settings are not implemented.

## RAW processing and profiles

- Demosaicing, highlight recovery, noise reduction and sharpening do not match Adobe algorithms. Fine detail, edges and clipped highlights can differ.
- The camera exposure baseline is verified only for X100F DR100. Sony and other Fuji dynamic-range modes lack equivalent visual validation.
- DCP support is a bounded subset. Enhanced Adobe XMP profiles and their dependent lookup tables, HDR/triple-illuminant profiles, and unsupported profile structures are not implemented.
- White balance after demosaicing has limitations at extreme adjustments. Profile/WB/calibration processing order is not proven identical.
- Source active-area and geometric differences limit pixel-aligned full-resolution comparisons.

## Lens, local and finishing adjustments

- Built-in Fujifilm lens corrections (vignetting, distortion, lateral CA) are applied as Lightroom does; Sony's are read but off by default until verified. There is no LCP engine yet, so imported Adobe lens profiles are not applied. See [lens corrections](lens-corrections.md).
- Lightroom masks, local adjustments, AI selections and healing/removal are not reproduced.
- Texture, clarity, dehaze, grain, vignetting and detail controls need isolated numerical and visual parity checks.
- HDR editing/output and Adobe AI denoise/enhance are not equivalent or implemented.

## Catalog and interaction

- Ratings, pick/reject flags and exact color-label text import into the RAWmakase catalog. Both modules now provide controls and Lightroom-style shortcuts. Custom label-set text is preserved, but automatic custom text-to-color mapping remains missing: unmapped labels display white. Multi-photo metadata edits and metadata undo are not implemented.
- The RAWmakase catalog is separate. There is no write-back to the original `.lrcat` and no bidirectional synchronization with Lightroom.
- Unsupported develop settings are retained/reported; retaining their source text does not mean RAWmakase renders them.
- Lightroom's full catalog organization, workflow shortcuts, filtering and batch-editing behavior are not feature-complete.

## Validation still needed

Expand controlled comparisons across cameras, illuminants, skin tones, highly saturated subjects and clipping. Test isolated controls and combinations, separating RAW/profile baseline error from each adjustment. Keep the original Lightroom catalog and photo files untouched; use copied catalogs/photos and generated fixtures. Report regressions as well as improvements, and do not describe visual closeness as exact parity.
