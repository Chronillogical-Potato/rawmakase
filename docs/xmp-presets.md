# XMP presets

The left Presets pane groups installed presets, searches names/groups, remembers favorites, and shows unavailable entries dimmed with an explanation on hover. The optional “Compatible only” filter hides entries that cannot be applied to the current camera. Hover for a temporary preview; click to apply as one undo step. Previewing does not save edits. Import accepts individual XMP files; the supplied archive has already been imported with its directory structure preserved.

User library: `xmp-presets` in the data directory (`~/Library/Application Support/RAWmakase` on macOS, `~/.local/share/rawmakase` on Linux). Original XMP files remain unchanged. Favorites are stored separately in the application data directory. The supplied archive contains 926 XMP files: 921 presets/curves and five application preference/cache files excluded from the browser. No Adobe presets or profiles are bundled with RAWmakase; its own built-in presets are described below.

Presets are sparse patches: omitted settings retain the current edit; explicit zero/false resets the corresponding control. Matching camera DCP profiles are resolved by name and camera model. Supported enhanced Look records resolve an explicitly imported XMP profile by name/UUID and camera. See [imported profiles](lightroom-profiles.md). The resulting recipe embeds the selected DCP and enhanced rendering data and stores applied settings, so reopening an edited photo does not require reapplying its XMP. Sidecar/preset schema 4 adds these effects; older recipes load with neutral defaults.

## Built-in presets

RAWmakase ships 26 presets of its own, listed before imported ones in Lightroom's group order: Color, Creative, B&W, Curve, Grain and Vignetting. They are ordinary Lightroom XMP presets in `assets/presets`, MIT-licensed like the rest of the code, embedded in the binary, and read-only in the app. Favorites of a built-in preset follow its UUID, so renaming or regrouping one keeps them.

Only the Creative looks name a camera profile: Adobe Standard, which is what they were made with. Imported presets need the exact profile they name. Built-in presets, and a photo's own Lightroom edit (from a catalog or photo sidecar), fall back instead, so they work with nothing imported:

- Adobe Standard: the imported Adobe Standard for this camera, else the DNG's embedded profile, else RAWmakase Standard.
- Adobe Color: the imported Adobe Color, else RAWmakase Color.

The hover text and status line say when a fallback is used. Another camera's profile is never used, and one Adobe look never stands in for another. See [RAWmakase profiles](lightroom-profiles.md#rawmakase-profiles).

To add one, drop an `.xmp` into a group folder, list it in `src/presets/builtin.rs`, give it a new `crs:UUID` and `crs:Copyright="RAWmakase contributors, MIT licence"`, and run the preset tests; they check that every file is listed, parses, applies without imported profiles and names no other profile.

## Implemented settings

- White balance, exposure, contrast, highlights, shadows, whites, blacks, saturation and vibrance.
- Master and RGB point curves, parametric curves and region boundaries.
- Eight-band HSL, black-and-white mixing, split toning and modern color grading.
- Primary calibration and shadow tint, clarity, texture and dehaze.
- Sharpening, luminance/chroma noise controls and hue-based defringing.
- Grain, post-crop vignette and manual lens vignette.
- Lens profile corrections (Enable Profile Corrections with its Distortion and Vignetting amounts), Remove Chromatic Aberration, Transform sliders and Upright. See [lens corrections](lens-corrections.md) and [transform](transform.md).
- Declared crop/straighten and automatic tone/white-balance estimates.

These use RAWmakase's rendering algorithms. Adobe's proprietary operators are not reproduced exactly; the same numerical preset values are not evidence of identical Lightroom pixels. Auto adjustments, calibration, local contrast, noise reduction, grain, vignette and defringe are independent implementations. Post-crop vignette styles follow Lightroom's codes (1 Highlight Priority, 2 Color Priority, 3 Paint Overlay; 0 or none reads as Highlight Priority). Highlight Priority renders as an exposure change; Color Priority and Paint Overlay share one blend toward black or white until Color Priority has its own operator. Validate critical appearance against Lightroom exports when available.

## Compatibility checks

Spot removal (`RetouchAreas`, `RetouchInfo`) and brush, gradient, radial and luminance-range masks convert to RAWmakase's experimental spots and masks ([masking](masking.md)); AI and color-range masks are reported. Unknown active settings, missing DCPs or imported enhanced-profile dependencies, Point Color/Color Variance, a Profile Amount other than 0 or 100%, manual lens distortion, Glow and Reshape mark a preset unavailable, with the reason on hover. Lens-profile correction, Transform and Upright apply (as of 2026-10-03): an Upright mode without Lightroom's stored correction is analysed on each photo, except Guided, which needs the stored one. Unavailable entries stay visible by default and can still be clicked: as in Lightroom, each group of settings (profile, basic tone, geometry, white balance and so on) that applies does, the others keep the photo's current values, and the status line lists what was skipped. Hover previews do the same. A camera-specific Sony preset is not substituted with an unrelated Fuji profile.

All 921 supplied presets parse. When counted on 2026-09-26, 785 were applicable to Sony ILCE-7M2 and 534 to Fujifilm X100F; these sets overlap. Presets with lens-profile correction or perspective settings were rejected then and apply now, so these counts are out of date until the audit is rerun on the same archive. Most unavailable entries reference profiles for the other camera or dependencies absent from the archive. Complete Adobe XMP/rendering parity is not implemented.

The parser uses XML namespaces, supports RDF names/groups and both attribute and scalar-element settings. It narrowly repairs a repeated closing Group tag found in some supplied VSCO files in memory and reports that repair. Curve endpoints missing from older preset curves are extended constantly. Malformed numbers, invalid curves and out-of-range values are rejected.

## CLI and verification

```sh
rawmakase render photo.ARW output.jpg --xmp /path/to/preset.xmp --max-edge 2400
cargo test --test xmp_presets -- --ignored --nocapture
```

The private audit requires the installed library. Unit tests cover sparse application, explicit resets, namespace aliases, RGB curves, missing-dependency refusal, malformed input, serialization and full-frame/tile agreement with spatial effects.
