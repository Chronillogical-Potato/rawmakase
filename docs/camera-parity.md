# Checking camera and tone parity

The checks cover camera white-balance calibration (#353) and provide diagnostics
for the separate, unfinished tone-control work (#354). Adobe DNG Converter
supplies white-balance calibration facts. Photoshop with Camera Raw supplies
rendered references for tone controls; DNG Converter alone cannot do that.
Neither is a runtime dependency of RAWmakase.

## White-balance calibration

Install ExifTool and Adobe DNG Converter, then audit public RAW samples:

```sh
python3 scripts/cameras/check-calibration.py --converter /path/to/converter \
  --report /tmp/calibration.json sample.ARW another.NEF
```

For already converted DNGs omit `--converter`. Auditing Sony model references
also needs the original RAW daylight metadata, so use `--converter` for Sony. The script converts temporary
copies, checks calibration signatures and matrix shape, and compares exact camera
models and aliases against `data/cameras.toml`. A mismatch or unsupported matrix
returns a nonzero exit status. The report proposes gains or a Sony daylight
reference; it does not edit the
table. Check multiple samples, especially different ISO and recording modes,
before applying a proposal. Conflicting samples must be investigated.

Sony calibration is per individual camera unit, not a fixed model gain. The native
adapter reads `WB_Coeffs[Daylight]` from LibRaw (the RAW's `WB_RGBLevelsDaylight`,
not LibRaw's matrix-derived `pre_mul`). The model divides its measured
`sony_daylight_reference` by that per-file preset, normalizing both to green,
and rounds to the four decimals recorded by DNG Converter. Missing/invalid
presets retain identity rather than borrowing another unit's correction.
The 36 public Sony samples reproduce their converter gains exactly. Two A7CR
samples from different units additionally verify different corrections:
`[.9432, 1, .9994]` and `[.9284, 1, 1.0158]`, from the same model reference
`[2423, 1024, 1799]`. This independently measured relationship agrees with the
[RawSpeed calibration investigation](https://github.com/darktable-org/rawspeed/compare/develop...tdonovan4:rawspeed:camera-calibration-matrix).
Fresh Camera Raw 18.7 comparisons of these two A7CR samples pass both defaults
and all eight custom-WB cases (median ΔE00 .45–1.01 for custom WB).

Non-Sony converter facts remain diagnostic evidence; they are not installed as
fixed corrections for native RAWs. One sample cannot establish that a correction
applies to every unit, firmware or recording mode. The existing X100F correction,
previously measured from two DNG exports, is retained. DNG files use their own
supported calibration regardless of make.

The supported correction is equal, positive diagonal CameraCalibration1/2 with
identity AnalogBalance and matching Adobe signatures. Unknown cameras retain
identity calibration; there is no make-wide calibration fallback. Illuminant-
dependent matrices, off-diagonal matrices and non-identity AnalogBalance need a
full matrix implementation and are reported as unsupported. Do not flatten them
into gains or copy another body's values.

`data/camera-calibration-facts.json` records the converter version, public source
URLs and SHA-256 hashes, and numerical tags for 142 public samples plus two
anonymous local A7CR samples. Local entries contain no filenames or paths. CI validates
all measured tags and checks supported Sony/X100F native calibration against the
camera table without requiring Adobe software. Other native makes with non-identity
converter calibration remain reported mismatches. RAWs and
Adobe profiles are not committed. The existing `source` and `checked` fields in
camera rows describe baseline exposure; calibration provenance is in this separate
facts file.

```sh
python3 -m unittest discover -s scripts/cameras -p test_calibration.py
```

## Rendered white balance and tone controls

Install Python 3.11+, NumPy, Pillow and ExifTool. Reference capture currently uses
Photoshop's scripting interface on macOS and requires Photoshop to have no open
documents. Install matching Adobe profiles in RAWmakase through its usual profile
workflow. Use licensed local profile files; never commit them.

```sh
python3 scripts/cameras/parity.py reference --out /tmp/camera-parity \
  --raws sample.ARW another.NEF
python3 scripts/cameras/parity.py check --out /tmp/camera-parity \
  --rawmakase target/release/rawmakase
```

The default profile is Adobe Standard. For Adobe Color, also pass
`--look /path/to/Adobe\ Color.xmp`; a profile label alone does not select the actual
look. Use `--cases shadows100 dehaze40 whites100` for a shorter tone-only capture;
the default rendering is always included. The full capture also tests four
Temperature/Tint combinations and intermediate slider values.

Capture uses copies and validates the profile, look and settings actually embedded
in each reference TIFF. It saves resolved XMP and freezes source/reference/XMP
hashes in `manifest.json`. If rendering completes but verification is interrupted,
run `verify --out /tmp/camera-parity`. A frozen manifest is not overwritten.
`check` renders the frozen XMP, rejects profile substitutions and changed inputs,
and writes `report.json`, candidate TIFFs, recipes and diagnostic logs.

Comparison keeps 16-bit precision, reduces to a common 256-pixel long edge, and
uses the default render to select a small alignment offset reused for all edits.
Reports include median CIEDE2000, encoded RGB mean absolute error, and luminance
bands. Default color error must be at most 3; WB error may exceed the default by
at most 1. Tone edits may add at most .020 RGB MAE (.025 for Whites). These are
regression targets, not a promise of Adobe-identical rendering. The checker also
compares each edit’s change from default: its error must stay within 80% of the
reference effect, with a .003 MAE noise floor. This catches ignored small edits
that a baseline-error subtraction alone can miss. A failed gate
must be investigated; do not replace references or loosen limits to hide it.

Use diverse scenes, including dark images, isolated clipped highlights and large
bright areas, across multiple cameras. One sample per camera is not a test of
all scenes. Adobe's tone processing remains proprietary and this implementation
is an approximation; strong Whites and high-key Shadows still need particular
attention. Baseline profile/demosaicing differences can also dominate an edit.

```sh
python3 -m unittest discover -s scripts/cameras
```

## Compatibility and ownership

Camera calibration belongs to the model's camera facts and DNG metadata, and is
applied by the profile's Temperature/Tint conversion only when signatures match.
A DNG's own calibration takes precedence over the native-camera table.
`WhiteBalanceModel` preserves the previous calibration for saved edits and old
RAWmakase XMP; new edits and Adobe WB imports use `Calibrated`. This keeps old
gains and the next Temperature/Tint change continuous. The original X100F
correction remains part of the legacy conversion. Choosing As Shot also retains
the edit’s version; resetting the whole photo or pasting calibrated White Balance
settings selects the new conversion.
