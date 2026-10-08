#!/usr/bin/env python3
"""Audit camera white-balance calibration against Adobe DNG Converter.

Pass native RAWs with --converter PATH, or already converted DNGs without it.
Conversions use private temporary copies; original RAWs and sidecars are untouched.
The JSON report contains hashes and camera facts, never proprietary profile bytes.
Exit 1 means a missing/different table value or an unsupported calibration: review
`proposed` before adding a row. Conflicting samples of one body are errors rather
than silently selecting one. Requires exiftool; conversion requires Adobe's app.

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
TABLE = Path(__file__).resolve().parents[2] / 'data/cameras.toml'


def numbers(tags, name, default):
    value = tags.get(name, default)
    values = value if isinstance(value, list) else str(value).split()
    values = [float(v) for v in values]
    if len(values) != len(default) or not all(math.isfinite(v) for v in values):
        raise ValueError(f'{name}: invalid shape or non-finite values')
    return values


def calibration(tags):
    if not tags.get('DNGVersion'):
        raise ValueError('input is not a DNG')
    if numbers(tags, 'AnalogBalance', [1.] * 3) != [1.] * 3:
        raise ValueError('non-identity AnalogBalance requires a full matrix implementation')
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


def table_row(rows, make, model):
    return next((r for r in rows if r['make'].casefold() == make.casefold()
                 and model.casefold() in [v.casefold() for v in [r['model'], *r.get('aliases', [])]]), None)


def audit(tags, rows):
    make, model = tags.get('Make', ''), tags.get('Model', '')
    make = {'nikon corporation': 'Nikon', 'ricoh imaging company, ltd.': 'Pentax',
            'olympus corporation': 'Olympus', 'olympus imaging corp.': 'Olympus',
            'om digital solutions': 'OM Digital', 'leica camera ag': 'Leica'}.get(make.casefold(), make)
    # Adobe's Model can repeat the make; LibRaw's model does not.
    if model.casefold().startswith(make.casefold() + ' '):
        model = model[len(make):].strip()
    model = {'EOS M50m2': 'EOS M50 Mark II', 'EOS R5m2': 'EOS R5 Mark II',
             'EOS R6m2': 'EOS R6 Mark II'}.get(model, model)
    result = {'make': make, 'model': model}
    if not make or not model:
        return dict(result, status='unsupported', reason='missing camera identity')
    try:
        actual = calibration(tags)
    except (ValueError, TypeError) as error:
        return dict(result, status='unsupported', reason=str(error))
    row = table_row(rows, make, model)
    if make.casefold() == 'sony' and 'NativeDaylightWB' not in tags:
        return dict(result, status='unsupported', reason='Sony calibration audit requires native RAW daylight metadata; pass the RAW with --converter')
    proposed = f'neutral_calibration = {json.dumps(actual)}'
    if make.casefold() == 'sony' and 'NativeDaylightWB' in tags:
        try:
            wb = numbers(tags, 'NativeDaylightWB', [0.] * 3)
            if any(v <= 0 for v in wb):
                raise ValueError('invalid native daylight preset')
        except (ValueError, TypeError) as error:
            return dict(result, status='unsupported', reason=str(error))
        reference = [round(g * w / wb[1] * 1024) for g, w in zip(actual, wb)]
        if any(abs(r / 1024 / (w / wb[1]) - g) > .00005 for r, w, g in zip(reference, wb, actual)):
            return dict(result, status='unsupported', reason='Sony daylight reference does not reproduce DNG calibration')
        result['proposed_reference'] = reference
        proposed = f'sony_daylight_reference = {json.dumps(reference)}'
    else:
        wb = None
    if row and 'sony_daylight_reference' in row:
        if wb is None:
            return dict(result, status='unsupported', reason='Sony calibration audit requires native RAW daylight metadata; pass the RAW with --converter')
        ref = row['sony_daylight_reference']
        expected = [math.floor((r / ref[1]) / (w / wb[1]) * 10000 + .5) / 10000
                    for r, w in zip(ref, wb)]
    else:
        expected = row.get('neutral_calibration', [1.] * 3) if row else [1.] * 3
    status = ('ok' if all(abs(a - b) <= 0.00005 for a, b in zip(actual, expected)) else 'mismatch') if row else 'missing'
    return dict(result, status=status, measured=actual, applied=expected,
                proposed=proposed)


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
                native = json.loads(subprocess.check_output(['exiftool', '-j', '-n', '-WB_RGBLevelsDaylight', str(raw)]))[0]
                if 'WB_RGBLevelsDaylight' in native:
                    tags['NativeDaylightWB'] = [float(v) for v in str(native['WB_RGBLevelsDaylight']).split()]
            record.update(audit(tags, rows))
            record['tags'] = {k: v for k, v in tags.items() if k != 'SourceFile'}
            results.append(record)
            if args.converter:
                shutil.rmtree(work)
    by_camera = {}
    for r in results:
        if 'measured' in r:
            by_camera.setdefault((r['make'].casefold(), r['model'].casefold()), []).append(r)
    for samples in by_camera.values():
        if len({tuple(r.get('proposed_reference', r['measured'])) for r in samples}) > 1:
            for r in samples:
                r.update(status='conflict', reason='samples from this body have different calibrations')
    report = {'schema': 1, 'samples': results}
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    for r in results:
        print(f"{r['make']} {r['model']}: {r['status']} {r.get('proposed', r.get('reason', ''))}")
    return int(any(r['status'] != 'ok' for r in results))


if __name__ == '__main__':
    raise SystemExit(main())
