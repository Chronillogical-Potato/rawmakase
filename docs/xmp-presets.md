# XMP presets

The left Presets pane groups installed presets, searches names/groups, remembers favorites, and shows unavailable entries dimmed with an explanation on hover. The optional “Compatible only” filter hides entries that cannot be applied to the current camera. Hover for a temporary preview; click to apply as one undo step. Previewing does not save edits. Import accepts individual XMP files; the supplied archive has already been imported with its directory structure preserved.

User library: `~/.local/share/rawmakase/xmp-presets`. Original XMP files remain unchanged. Favorites are stored separately in the application data directory. The supplied archive contains 926 XMP files: 921 presets/curves and five application preference/cache files excluded from the browser. No private preset or profile assets are bundled with RAWmakase.

Presets are sparse patches: omitted settings retain the current edit; explicit zero/false resets the corresponding control. Matching camera DCP profiles are resolved by name and camera model. Supported enhanced Look records resolve an explicitly imported XMP profile by name/UUID and camera. See [imported profiles](lightroom-profiles.md). The resulting recipe embeds the selected DCP and enhanced rendering data and stores applied settings, so reopening an edited photo does not require reapplying its XMP. Sidecar/preset schema 4 adds these effects; older recipes load with neutral defaults.

## Implemented settings

- White balance, exposure, contrast, highlights, shadows, whites, blacks, saturation and vibrance.
- Master and RGB point curves, parametric curves and region boundaries.
- Eight-band HSL, black-and-white mixing, split toning and modern color grading.
- Primary calibration and shadow tint, clarity, texture and dehaze.
- Sharpening, luminance/chroma noise controls and hue-based defringing.
- Grain, post-crop vignette and manual lens vignette.
- Declared crop/straighten and automatic tone/white-balance estimates.

These use RAWmakase's rendering algorithms. Adobe's proprietary operators are not reproduced exactly; the same numerical preset values are not evidence of identical Lightroom pixels. Auto adjustments, calibration, local contrast, noise reduction, grain, vignette and defringe are independent implementations. Vignette highlight/color-priority styles currently share a gain-based operator; paint-overlay has a separate operator. Validate critical appearance against Lightroom exports when available.

## Compatibility checks

Spot removal (`RetouchAreas`, `RetouchInfo`) and brush, gradient, radial and luminance-range masks convert to RAWmakase's experimental spots and masks ([masking](masking.md)); AI and color-range masks are reported. Unknown active settings, missing DCPs or imported enhanced-profile dependencies, Point Color/Color Variance, automatic lateral chromatic aberration, lens-profile correction and active perspective correction are rejected with a reason. No partially applied recipe is committed on failure. These entries remain visible by default; hover to inspect the reason. A camera-specific Sony preset is not substituted with an unrelated Fuji profile.

All 921 supplied presets parse. On the supplied cameras, 785 are currently applicable to Sony ILCE-7M2 and 534 to Fujifilm X100F; these sets overlap. Most unavailable entries reference profiles for the other camera or dependencies absent from the archive. Complete Adobe XMP/rendering parity is not implemented.

The parser uses XML namespaces, supports RDF names/groups and both attribute and scalar-element settings. It narrowly repairs a repeated closing Group tag found in some supplied VSCO files in memory and reports that repair. Curve endpoints missing from older preset curves are extended constantly. Malformed numbers, invalid curves and out-of-range values are rejected.

## CLI and verification

```sh
rawmakase render photo.ARW output.jpg --xmp /path/to/preset.xmp --max-edge 2400
cargo test --test xmp_presets -- --ignored --nocapture
```

The private audit requires the installed library. Unit tests cover sparse application, explicit resets, namespace aliases, RGB curves, missing-dependency refusal, malformed input, serialization and full-frame/tile agreement with spatial effects.
