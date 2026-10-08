#!/usr/bin/env python3
r"""Repeatable white-balance and tone parity against Camera Raw.

`reference` renders copies using Photoshop/Camera Raw and verifies the profile and
settings actually embedded in the TIFFs. `check` reuses these frozen references,
renders the exact resolved XMP with RAWmakase, and exits nonzero on a failed gate.
No references are replaced by `check`. All output belongs outside tracked source.
Requires Python numpy/Pillow, exiftool; `reference` additionally requires Photoshop.
Adobe profiles must be installed in RAWmakase for matching profiles to render.

  python3 scripts/cameras/parity.py reference --out /tmp/parity --raws sample.ARW
  python3 scripts/cameras/parity.py check --out /tmp/parity --rawmakase target/release/rawmakase

Use --look /path/to/Adobe\ Color.xmp for Adobe Color: a bare profile label is not
sufficient. Use multiple cameras and both low-key and high-key scenes. DNG Converter
is a calibration oracle (check-calibration.py), not a Camera Raw rendering oracle.
"""
import argparse
import hashlib
import importlib.util
import json
import subprocess
import sys
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts/corpus'))
import parity_metrics as metrics


def module(name, file):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'scripts/corpus' / file)
    m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
    return m


def digest(path):
    with Path(path).open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def cases():
    result = [('default', {})]
    for t, tint in [(3200, 0), (5600, 0), (7500, 0), (5600, 20)]:
        result.append((f'wb-{t}-{tint}', dict(WhiteBalance='Custom', Temperature=str(t), Tint=str(tint))))
    for key, label, amounts in [('Shadows2012', 'shadows', [50, 100]),
                                ('Dehaze', 'dehaze', [20, 40]),
                                ('Whites2012', 'whites', [50, 100])]:
        result.extend((f'{label}{v}', {key: str(v)}) for v in amounts)
    return result


def verify(tags, expected, profile, look):
    if tags.get('CameraProfile') != profile:
        raise ValueError(f"wrong CameraProfile: {tags.get('CameraProfile')!r}, expected {profile!r}")
    if tags.get('LookName', '') != look:
        raise ValueError(f"wrong LookName: {tags.get('LookName')!r}, expected {look!r}")
    for key, value in expected.items():
        actual = str(tags.get('ColorTemperature' if key == 'Temperature' else key, ''))
        try:
            tolerance = 50 if key == 'Temperature' else 1 if key == 'Tint' else 0.0001
            matches = abs(float(actual) - float(value)) <= tolerance
        except ValueError:
            matches = actual.casefold() == value.casefold()
        if not matches:
            raise ValueError(f'{key}: expected {value!r}, got {actual!r}')


def reference(args):
    photos = module('photos', 'camera-raw-photos.py')
    charts = module('charts', 'camera-raw-charts.py')
    out = args.out.resolve()
    if out.exists() and any(out.iterdir()):
        raise ValueError('reference output must be empty; use a new directory to retain existing evidence')
    if not photos.photoshop_idle():
        raise ValueError('Photoshop is not available or has open documents; reference capture cannot start')
    out.mkdir(parents=True, exist_ok=True)
    for name in ['copies', 'references', 'xmp']:
        (out / name).mkdir()
    base = json.loads((ROOT / 'tests/corpus/cases.json').read_text())['base']
    base.update({k: '0' for k in ['Exposure2012', 'Contrast2012', 'Highlights2012', 'Shadows2012',
                                 'Whites2012', 'Blacks2012', 'Dehaze', 'Clarity2012', 'Texture']})
    plan, jobs = [], []
    look_name = ''
    if args.look:
        import re
        look_name = re.search(r'xml:lang="x-default">([^<]*)<', args.look.read_text()).group(1)
    for index, raw in enumerate(args.raws):
        raw = raw.resolve()
        raw_hash = digest(raw)
        copy = out / 'copies' / f'{index}{raw.suffix}'
        for name, settings in cases():
            if args.cases and name not in args.cases and name != 'default':
                continue
            case = dict(settings=settings)
            if args.look:
                case['look'] = dict(file=str(args.look.resolve()), amount=100)
            xmp = charts.xmp(base, case, dict(CameraProfile=args.profile))
            target = out / 'references' / f'{index}-{name}.tif'
            jobs.append(dict(source=str(raw), raw=str(copy), xmp=str(copy.with_suffix('.xmp')),
                             fresh=True, settings=xmp, out=str(target)))
            plan.append(dict(sample=index, name=name, raw=str(raw), sha256=raw_hash,
                             reference=str(target.relative_to(out)), expected=dict(base, **settings),
                             profile=args.profile, look=look_name))
    script = out / 'render.jsx'
    script.write_text(photos.TEMPLATE % dict(jobs=json.dumps(jobs), edge=2048))
    (out / 'capture.json').write_text(json.dumps(dict(schema=1, camera_raw=photos.camera_raw_version(), cases=plan), indent=2))
    print(f'Rendering {len(jobs)} references', flush=True)
    subprocess.run(['osascript', '-e', f'with timeout of 36000 seconds\ntell application "{photos.PHOTOSHOP}" '
                    f'to do javascript file (POSIX file {json.dumps(str(script))})\nend timeout'], check=True)
    return verify_capture(out)


def verify_capture(out):
    if (out / 'manifest.json').exists():
        raise ValueError('reference manifest is already frozen; capture into a new directory')
    capture = json.loads((out / 'capture.json').read_text())
    plan = capture['cases']
    for c in plan:
        target = out / c['reference']
        if digest(c['raw']) != c['sha256']:
            raise ValueError(f"source changed during reference capture: {Path(c['raw']).name}")
        if not target.is_file():
            raise ValueError(f'missing reference: {target.name}')
        tags = json.loads(subprocess.check_output(['exiftool', '-j', '-n', '-XMP-crs:all', str(target)]))[0]
        verify(tags, c['expected'], c['profile'], c['look'])
        xmp = out / 'xmp' / (target.stem + '.xmp')
        xmp.write_bytes(subprocess.check_output(['exiftool', '-b', '-XMP', str(target)]))
        c.update(xmp=str(xmp.relative_to(out)), reference_sha256=digest(target), xmp_sha256=digest(xmp))
    (out / 'manifest.json').write_text(json.dumps(dict(schema=1, camera_raw=capture['camera_raw'], cases=plan), indent=2))
    print(f'Verified {len(plan)} references: {out / "manifest.json"}')


def score(reference, actual):
    if reference.shape != actual.shape:
        raise ValueError(f'framing differs: {reference.shape} / {actual.shape}')
    a, b = reference[3:-3, 3:-3], actual[3:-3, 3:-3]
    la, lb = metrics.lab(a), metrics.lab(b)
    delta = lb - la
    bands = {}
    for lo in [10, 40, 50]:
        mask = (la[..., 0] >= lo) & (la[..., 0] < lo + 10)
        bands[f'{lo}-{lo + 10}'] = dict(fraction=float(mask.mean()), dL=float(delta[..., 0][mask].mean()) if mask.any() else None)
    return dict(median_de00=float(np.median(metrics.de00(la, lb))), mae=float(abs(a - b).mean()),
                mean_dL=float(delta[..., 0].mean()), p90_L=float(np.percentile(la[..., 0], 90)), bands=bands)


def effect_score(reference, actual, reference_default, actual_default):
    """Compare the edits themselves, so a small ignored edit cannot pass on baseline error."""
    expected = (reference - reference_default)[3:-3, 3:-3]
    produced = (actual - actual_default)[3:-3, 3:-3]
    strength = float(abs(expected).mean())
    error = float(abs(produced - expected).mean())
    return dict(reference_effect=strength, candidate_effect=float(abs(produced).mean()),
                effect_mae=error, effect_passed=error <= max(.003, .8 * strength))


def check(args):
    out = args.out.resolve()
    manifest = json.loads((out / 'manifest.json').read_text())
    render = out / 'candidate'; render.mkdir(exist_ok=True)
    results, originals = [], {}
    for c in manifest['cases']:
        try:
            raw = Path(c['raw']); ref = out / c['reference']; xmp = out / c['xmp']
            for path, expected in [(raw, c['sha256']), (ref, c['reference_sha256']), (xmp, c['xmp_sha256'])]:
                if digest(path) != expected:
                    raise ValueError(f'input changed since reference capture: {path.name}')
            target = render / ref.name
            target.with_suffix('.json').unlink(missing_ok=True)
            process = subprocess.run([str(args.rawmakase.resolve()), 'render', str(raw), str(target), '--xmp', str(xmp),
                            '--max-edge', '2048', '--overwrite', '--save-recipe', str(target.with_suffix('.json'))],
                           capture_output=True, text=True)
            target.with_suffix('.log').write_text(process.stdout + process.stderr)
            if process.returncode:
                results.append(dict(sample=c['sample'], name=c['name'], passed=False, error=process.stderr.strip()))
                print(f"{c['sample']}/{c['name']}: render failed: {process.stderr.strip()}", flush=True)
                continue
            recipe = json.loads(target.with_suffix('.json').read_text())
            used = (recipe['recipe'].get('profile') or {}).get('name')
            if used != (c['look'] or c['profile']):
                raise ValueError(f'RAWmakase substituted profile {used!r}')
            a, b = metrics.load(ref), metrics.load(target)
        except (ValueError, OSError, KeyError) as error:
            message = str(error)
            results.append(dict(sample=c['sample'], name=c['name'], passed=False, error=message))
            print(f"{c['sample']}/{c['name']}: {message} FAIL", flush=True)
            continue
        key = c['sample']
        if a.shape != b.shape or (key in originals and a.shape != originals[key]['reference'].shape):
            message = f'framing differs: reference {a.shape}, candidate {b.shape}'
            if key in originals:
                message += f", default {originals[key]['reference'].shape}"
            results.append(dict(sample=key, name=c['name'], passed=False, error=message))
            print(f"{key}/{c['name']}: {message} FAIL", flush=True)
            continue
        if c['name'] == 'default':
            # Keep alignment fixed for all edits of the same photo.
            best = min((float(((metrics.lab(metrics.shift(b, y, x))[3:-3, 3:-3, 0] - metrics.lab(a)[3:-3, 3:-3, 0])**2).mean()), y, x)
                       for y in [-1., -.5, 0., .5, 1.] for x in [-1., -.5, 0., .5, 1.])
            originals[key] = dict(offset=best[1:])
        if key not in originals:
            results.append(dict(sample=key, name=c['name'], passed=False, error='default render unavailable'))
            continue
        offset = originals[key]['offset']
        aligned = metrics.shift(b, *offset)
        measured = score(a, aligned)
        if c['name'] == 'default':
            originals[key].update(measured, reference=a, actual=aligned)
        extra = measured['mae'] - originals[key]['mae']
        limit = .025 if c['name'].startswith('whites') else .020
        failed = measured['median_de00'] > 3 if c['name'] == 'default' else (
            measured['median_de00'] > originals[key]['median_de00'] + 1.0 if c['name'].startswith('wb') else extra > limit)
        effect = {}
        if c['name'] != 'default' and not c['name'].startswith('wb'):
            effect = effect_score(a, aligned, originals[key]['reference'], originals[key]['actual'])
            failed |= not effect['effect_passed']
        results.append(dict(sample=key, name=c['name'], alignment=offset, **measured, **effect, extra_mae=extra, passed=not failed))
        print(f"{key}/{c['name']}: ΔE00 {measured['median_de00']:.2f}, extra MAE {extra:+.4f} {'FAIL' if failed else 'PASS'}", flush=True)
    report = dict(schema=1, binary_sha256=digest(args.rawmakase), camera_raw=manifest['camera_raw'], results=results)
    (out / 'report.json').write_text(json.dumps(report, indent=2))
    return int(any(not r['passed'] for r in results))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('step', choices=['reference', 'verify', 'check'])
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--raws', type=Path, nargs='+')
    parser.add_argument('--profile', default='Adobe Standard')
    parser.add_argument('--look', type=Path)
    parser.add_argument('--cases', nargs='+', choices=[name for name, _ in cases()],
                        help='reference cases to capture; default is always included')
    parser.add_argument('--rawmakase', type=Path)
    args = parser.parse_args()
    if args.step == 'reference' and not args.raws or args.step == 'check' and not args.rawmakase:
        parser.error('reference needs --raws; check needs --rawmakase')
    try:
        if args.step == 'verify':
            return verify_capture(args.out.resolve())
        return reference(args) if args.step == 'reference' else check(args)
    except (ValueError, subprocess.CalledProcessError, OSError) as e:
        print(f'Parity check failed: {e}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    raise SystemExit(main())
