#!/usr/bin/env python3
"""Audit camera white-balance calibration against Adobe DNG Converter.

Pass native RAWs with --converter PATH, or already converted DNGs without it.
Conversions use private temporary copies; original RAWs and sidecars are untouched.
The JSON report contains hashes and camera facts, never proprietary profile bytes.
Exit 1 means a missing/different table value or an unsupported calibration: review
`proposed` before adding a row. Samples of one model that no single model reference
explains are conflicts rather than silently selecting one. Requires exiftool;
conversion requires Adobe's app.

Adobe calibrates an individual unit from a white-balance preset that unit stored in
its RAW: the model reference divided by that preset, normalized to green and rounded
to four decimals. Native presets are read from the original RAW, so per-unit audits
need --converter. See docs/camera-parity.md.

  python3 scripts/cameras/check-calibration.py --report report.json sample.dng
  python3 scripts/cameras/check-calibration.py --converter /path/to/converter \
      --report report.json sample.ARW another.NEF
"""
import argparse
import hashlib
import json
import math
import shutil
import subprocess
import tempfile
import tomllib
from pathlib import Path

IDENTITY = [1., 0., 0., 0., 1., 0., 0., 0., 1.]
TAGS = ['Make', 'Model', 'DNGVersion', 'CameraCalibration1', 'CameraCalibration2',
        'CameraCalibrationSig', 'ProfileCalibrationSig', 'AnalogBalance']
# The native maker-note tags LibRaw reads as WB_Coeffs Daylight, Shade, D65 and
# Fine Weather (Olympus's 5300 K entry).
NATIVE_TAGS = ['WB_RGBLevelsDaylight', 'WB_RGGBLevelsDaylight', 'WB_RGGBLevelsShade',
               'WB_GRBLevelsStandard', 'WB_RBLevels5300K', 'WB_GLevel5300K']
PRESETS = ['daylight', 'shade', 'd65', 'fine-weather']
TABLE = Path(__file__).resolve().parents[2] / 'data/cameras.toml'


def numbers(tags, name, default):
    value = tags.get(name, default)
    values = value if isinstance(value, list) else str(value).split()
    values = [float(v) for v in values]
    if len(values) != len(default) or not all(math.isfinite(v) for v in values):
        raise ValueError(f'{name}: invalid shape or non-finite values')
    return values


def native_presets(native):
    """A native RAW's stored presets as R, G, B, as the native adapter reads them."""
    presets = {}

    def put(name, values):
        if len(values) == 3 and all(math.isfinite(v) and v > 0 for v in values):
            presets[name] = values

    def read(tag):
        return [float(v) for v in str(native[tag]).split()] if tag in native else []

    if len(v := read('WB_RGBLevelsDaylight')) == 3:
        put('daylight', v)
    if len(v := read('WB_RGGBLevelsDaylight')) == 4:
        put('daylight', [v[0], v[1], v[3]])
    if len(v := read('WB_RGGBLevelsShade')) == 4:
        put('shade', [v[0], v[1], v[3]])
    # Olympus: R, B, G, G, or R, B with green in its own tag on older bodies; LibRaw
    # takes 256 for the E-410 and E-510, which have none.
    if len(v := read('WB_RBLevels5300K')) == 4:
        put('fine-weather', [v[0], v[2], v[1]])
    elif len(v) == 2:
        put('fine-weather', [v[0], (read('WB_GLevel5300K') or [256.])[0], v[1]])
    # Fujifilm's standard-illuminant table: G, R, B and an EXIF light source per entry.
    v = read('WB_GRBLevelsStandard')
    for i in range(0, len(v) - 3, 4):
        if v[i + 3] == 21:
            put('d65', [v[i + 1], v[i], v[i + 2]])
    return presets


def calibration(tags):
    """The DNG's diagonal CameraCalibration. AnalogBalance is a separate per-file
    factor (DNG: XYZ-to-camera = AnalogBalance x CameraCalibration x ColorMatrix);
    Adobe writes it for linear DNGs of in-camera white-balanced modes such as
    Canon sRAW, whose calibration equals the same unit's mosaic RAW."""
    if not tags.get('DNGVersion'):
        raise ValueError('input is not a DNG')
    numbers(tags, 'AnalogBalance', [1.] * 3)
    if 'CameraCalibration1' not in tags and 'CameraCalibration2' not in tags:
        return [1.] * 3
    if tags.get('CameraCalibrationSig', '') != tags.get('ProfileCalibrationSig', ''):
        raise ValueError('camera and profile calibration signatures differ')
    if tags.get('CameraCalibrationSig', '') != 'com.adobe':
        raise ValueError('not an Adobe calibration; cannot apply to Adobe profiles')
    a = numbers(tags, 'CameraCalibration1', IDENTITY)
    b = numbers(tags, 'CameraCalibration2', IDENTITY)
    if a != b:
        raise ValueError('illuminants have different calibration matrices')
    if any(a[i] != 0. for i in [1, 2, 3, 5, 6, 7]):
        raise ValueError('non-diagonal calibration requires a full matrix implementation')
    diagonal = [a[i] for i in [0, 4, 8]]
    if not all(0. < v < 4. for v in diagonal):
        raise ValueError('invalid calibration diagonal')
    return diagonal


def unit_gains(reference, wb):
    """The calibration RAWmakase derives for one unit (cameras::neutral_calibration)."""
    return [math.floor((r / reference[1]) / (w / wb[1]) * 10000 + .5) / 10000
            for r, w in zip(reference, wb)]


def fit(samples, preset):
    """The integer model reference that reproduces every (measured, presets) sample
    with `preset`, or None."""
    wbs = [p.get(preset) for _, p in samples]
    if not samples or any(w is None for w in wbs):
        return None
    green = wbs[0][1]
    scaled = [[v * green / w[1] for v in w] for w in wbs]
    channels = []
    for c in (0, 2):
        values = [m[c] * w[c] for (m, _), w in zip(samples, scaled)]
        middle = round(sum(values) / len(values))
        channels.append(range(middle - 3, middle + 4))
    for red in channels[0]:
        for blue in channels[1]:
            reference = [red, green, blue]
            if all(unit_gains(reference, w) == m for (m, _), w in zip(samples, scaled)):
                return [int(v) for v in reference]
    return None


def fits(samples):
    """Every preset and reference that reproduces all samples of one model."""
    return {preset: ref for preset in PRESETS if (ref := fit(samples, preset))}


def table_row(rows, make, model):
    return next((r for r in rows if r['make'].casefold() == make.casefold()
                 and model.casefold() in [v.casefold() for v in [r['model'], *r.get('aliases', [])]]), None)


def identify(tags):
    make, model = tags.get('Make', ''), tags.get('Model', '')
    make = {'nikon corporation': 'Nikon', 'ricoh imaging company, ltd.': 'Pentax',
            'olympus corporation': 'Olympus', 'olympus imaging corp.': 'Olympus',
            'om digital solutions': 'OM Digital', 'leica camera ag': 'Leica',
            'fujifilm': 'Fujifilm', 'sony': 'Sony'}.get(make.casefold(), make)
    # Adobe's Model can repeat the make; LibRaw's model does not.
    if model.casefold().startswith(make.casefold() + ' '):
        model = model[len(make):].strip()
    model = {'EOS M50m2': 'EOS M50 Mark II', 'EOS R5m2': 'EOS R5 Mark II',
             'EOS R6m2': 'EOS R6 Mark II'}.get(model, model)
    return make, model


def audit(tags, rows):
    make, model = identify(tags)
    result = {'make': make, 'model': model}
    if not make or not model:
        return dict(result, status='unsupported', reason='missing camera identity')
    try:
        actual = calibration(tags)
    except (ValueError, TypeError) as error:
        return dict(result, status='unsupported', reason=str(error))
    if numbers(tags, 'AnalogBalance', [1.] * 3) != [1.] * 3:
        result['analog_balance'] = numbers(tags, 'AnalogBalance', [1.] * 3)
    presets = tags.get('NativeWhiteBalance', {})
    row = table_row(rows, make, model)
    unit = row.get('unit_calibration') if row else None
    if unit:
        if unit['preset'] not in presets:
            return dict(result, status='unsupported', measured=actual,
                        reason=f"needs the native RAW's {unit['preset']} preset; pass the RAW with --converter")
        expected = unit_gains(unit['reference'], presets[unit['preset']])
    else:
        expected = [1.] * 3
    if actual != [1.] * 3:
        candidates = fits([(actual, presets)])
        result['proposed'] = [f'unit_calibration = {{ preset = "{p}", reference = {json.dumps(r)} }}'
                              for p, r in candidates.items()] or ['no stored preset reproduces it']
    status = ('ok' if all(abs(a - b) <= 0.00005 for a, b in zip(actual, expected)) else 'mismatch') if row else 'missing'
    return dict(result, status=status, measured=actual, applied=expected)


def check_models(results):
    """Marks samples of one model that no common reference explains as conflicts."""
    by_camera = {}
    for r in results:
        if 'measured' in r:
            by_camera.setdefault((r['make'].casefold(), r['model'].casefold()), []).append(r)
    for samples in by_camera.values():
        measured = [(r['measured'], r['tags'].get('NativeWhiteBalance', {})) for r in samples]
        if len({tuple(m) for m, _ in measured}) > 1 and not fits(measured):
            for r in samples:
                r.update(status='conflict', reason='no model reference explains these units')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('raws', type=Path, nargs='+')
    parser.add_argument('--converter', type=Path)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    rows = tomllib.loads(TABLE.read_text())['camera']
    results = []
    with tempfile.TemporaryDirectory(prefix='rawmakase-calibration-') as tmp:
        for index, raw in enumerate(args.raws):
            with raw.open('rb') as input_file:
                sha = hashlib.file_digest(input_file, 'sha256').hexdigest()
            record = {'file': raw.name, 'sha256': sha}
            source = raw.resolve()
            if args.converter:
                work = Path(tmp) / str(index)
                work.mkdir()
                source = work / ('source' + raw.suffix)
                shutil.copyfile(raw, source)
                subprocess.run([str(args.converter.resolve()), '-c', '-d', str(work), str(source)],
                               check=True, capture_output=True)
                converted = list(work.glob('*.dng'))
                if len(converted) != 1:
                    raise RuntimeError(f'{raw.name}: converter did not produce exactly one DNG')
                source = converted[0]
            tags = json.loads(subprocess.check_output(['exiftool', '-j', '-n', *['-' + t for t in TAGS], str(source)]))[0]
            if args.converter:
                native = json.loads(subprocess.check_output(['exiftool', '-j', '-n', *['-' + t for t in NATIVE_TAGS], str(raw)]))[0]
                if presets := native_presets(native):
                    tags['NativeWhiteBalance'] = presets
            record.update(audit(tags, rows))
            record['tags'] = {k: v for k, v in tags.items() if k != 'SourceFile'}
            results.append(record)
            if args.converter:
                shutil.rmtree(work)
    check_models(results)
    report = {'schema': 2, 'samples': results}
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    for r in results:
        print(f"{r['make']} {r['model']}: {r['status']} {r.get('proposed', r.get('reason', ''))}")
    return int(any(r['status'] != 'ok' for r in results))


if __name__ == '__main__':
    raise SystemExit(main())
