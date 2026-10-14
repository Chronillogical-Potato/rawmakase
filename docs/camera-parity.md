# Checking camera and tone parity

The checks cover camera white-balance calibration (#353) and provide diagnostics
for the separate, unfinished tone-control work (#354). Adobe DNG Converter
supplies white-balance calibration facts. Photoshop with Camera Raw supplies
rendered references for tone controls; DNG Converter alone cannot do that.
Neither is a runtime dependency of RAWmakase.

## White-balance calibration

Install ExifTool and Adobe DNG Converter, then audit RAW samples:

```sh
python3 scripts/cameras/check-calibration.py --converter /path/to/converter \
  --report /tmp/calibration.json sample.ARW another.NEF
```

The script converts temporary copies, reads the calibration DNG Converter
records and the white-balance presets stored in the original RAW, and compares
exact camera models and aliases against `data/cameras.toml`. Already converted
DNGs can be audited without `--converter`, but then the native presets are
missing and a per-unit reference cannot be checked or proposed. A mismatch,
missing row, conflict or unsupported calibration returns a nonzero exit status.
The report proposes `unit_calibration` rows; it does not edit the table.

### Per-unit calibration

Adobe calibrates each camera unit, not each model. DNG Converter records a
diagonal CameraCalibration, equal for both illuminants, that is a model reference
divided by a white-balance preset the unit stored in its own RAW, both normalized
to green and rounded to four decimals. The camera table holds the reference and
which preset it divides:

```toml
unit_calibration = { preset = "daylight", reference = [1893, 1024, 1697] }
```

| Make | Preset | RAW tag (ExifTool) | LibRaw |
|---|---|---|---|
| Sony | `daylight` | `WB_RGBLevelsDaylight` | `WB_Coeffs[Daylight]` |
| Canon (most bodies) | `daylight` | `WB_RGGBLevelsDaylight` | `WB_Coeffs[Daylight]` |
| Canon EOS 200D, 5D Mark IV, 5DS R, 6D Mark II | `shade` | `WB_RGGBLevelsShade` | `WB_Coeffs[Shade]` |
| Fujifilm | `d65` | `WB_GRBLevelsStandard`, the entry for light source 21 | `WB_Coeffs[D65]` |

The native adapter exports these presets for every native RAW, not LibRaw's
matrix-derived `pre_mul`; `cameras::neutral_calibration` applies the row. A
missing or invalid preset, or a camera without a row, keeps identity rather than
borrowing another unit's correction, and there is no make-wide fallback.

Evidence (Adobe DNG Converter 18.7, 2026-10-14): 544 samples, 354 public
raw.pixls.us files and 190 anonymous local ones, including bodies of the same
model identified by serial number. For 59 models, bodies with different
calibrations reproduce Adobe's four-decimal values exactly from one integer
reference: 23 Sony and 31 Canon models with the Daylight preset, 3 Canon models
with Shade and 2 Fujifilm models with D65. Different ISO settings and recording
modes of one unit (RAW and C-RAW, compressed and uncompressed, sRAW and mRAW)
record one calibration. Separately measured models that share a sensor or
processor generation share their reference (Fujifilm X-T3, X-T30, X-T4, X-S10,
X-E4, X-Pro3 and X100V; Canon 250D and 850D, R6 Mark II and R8, R10 and R50),
which further supports single-sample rows. The A7CR, A7 II and X100F rows have
been checked against Lightroom or Camera Raw on the owner's bodies. This agrees
with the [RawSpeed calibration investigation](https://github.com/darktable-org/rawspeed/compare/develop...tdonovan4:rawspeed:camera-calibration-matrix)
for Sony.

Rows are installed when the samples identify one preset. Unresolved, so identity:

- Canon EOS-1D X Mark III: its one sample fits both Daylight and Shade.
- Canon EOS R6 Mark III, R50 V and PowerShot V1: no stored preset reproduces the
  sample; the R6 Mark III Daylight preset even varies between files of one body.
- Canon EOS M3 and Olympus E-M5 Mark III: units differ in a way no preset
  explains. Older PowerShots and FinePix bodies without the tag are likewise
  unexplained, but have no table rows.
- Nikon, Panasonic, Pentax and the Fujifilm GFX 50R, X-E5 and X-T50 get identity
  calibration from Adobe, which RAWmakase matches.

The EOS 5D Mark IV, 5DS R, 6D Mark II, 7D Mark II, 80D and 1D X Mark II have
verified references in the facts but no table row, which needs a baseline
exposure checked against Camera Raw renders first (see [camera table](cameras.md)).

AnalogBalance is a separate per-file factor (XYZ-to-camera = AnalogBalance x
CameraCalibration x ColorMatrix). DNG Converter writes it for linear DNGs of
in-camera white-balanced modes (Canon sRAW and mRAW, Nikon small RAW, Sony's
non-mosaic modes), and their CameraCalibration matches mosaic RAWs of the same
unit, so the audit records it apart. RAWmakase opens native sRAW files but not
linear DNGs, so it never applies an AnalogBalance.

Off-diagonal CameraCalibration and different matrices for the two illuminants,
which phone DNGs carry, are not applied; such DNGs keep identity. Do not flatten
them into gains or copy another body's values. A DNG's own supported calibration
applies regardless of make.

`data/camera-calibration-facts.json` records the converter version, public source
URLs and SHA-256 hashes, the numerical DNG tags and the native presets. Local
entries contain no filenames or paths. RAWs and Adobe profiles are not committed.
CI checks without Adobe software that every row's reference reproduces each of its
samples, that units of one model share a reference unless listed as unresolved,
and that every reference has committed evidence. The camera rows' `source` and
`checked` fields describe baseline exposure.

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
A DNG's own calibration takes precedence over the native-camera table. A
matrix-only embedded profile retains its signature through the fallback profile;
matching signatures enable the supported correction, and mismatches keep identity.
This includes RAWmakase Standard and Color, which use that same camera matrix.

Calibration changes which gains a Temperature and Tint mean, not a saved edit's
gains, which rendering uses. A changed reference therefore leaves existing edits'
colors as they were, while imported Lightroom and Camera Raw settings, including
their resolved As Shot values, get the gains they have in Lightroom. A saved edit
keeps the Temperature and Tint it shows; its next Temperature or Tint change uses
the current calibration.
