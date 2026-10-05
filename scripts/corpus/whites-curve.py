#!/usr/bin/env python3
"""Measure Camera Raw's positive Whites, which follows the photo's highlights.

`render` has Photoshop 2026 (Camera Raw) render synthetic-d65.dng at Exposure -2 to +1
(steps of 0.25) without and with Whites +25, +50, +75 and +100, keeping the patch
means in --refs (outside the repository). It refuses to start while Photoshop has
documents open.

`table` fits each render's curve (as scripts/corpus/parametric-curve.py fits its curves)
and prints `WHITES_ADAPTIVE` and `WHITES_EXPOSURES` for src/develop/basic_tone_data.rs
(+25, +50 and +100 at 64 bin centres in encoded ProPhoto RGB). With --rawmakase (a
built `rawmakase` binary) it also renders the chart at each exposure and prints
`WHITES_HIGHLIGHTS`, the 98th percentile of encoded luminance of a 512 px copy.

`offset` needs RAWMAKASE_CORPUS with the Camera Raw references of photos with Whites
cases (camera-raw-photos/**/Whites2012+*.json) and --rawmakase. For each photo it finds
the chart exposure whose Whites curves explain Camera Raw's best, renders the photo in
RAWmakase to measure its highlights, and prints how much brighter photos behave than
the chart with the same highlights (`WHITES_OFFSET`).

Requires numpy (and Pillow for `offset`). Run from the repository root:
  python3 scripts/corpus/whites-curve.py render|table|offset [--refs FILE] [--rawmakase BIN]
"""
import argparse
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True
HERE = Path(__file__).parent
_spec = importlib.util.spec_from_file_location('parametric', HERE / 'parametric-curve.py')
parametric = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(parametric)
_spec = importlib.util.spec_from_file_location('contrast', HERE / 'contrast-curve.py')
contrast = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(contrast)
sys.path.insert(0, str(HERE))
import tiff16  # noqa: E402

EXPOSURES = [-2 + 0.25 * i for i in range(13)]
AMOUNTS = [25, 50, 75, 100]
TABLE_AMOUNTS = [25, 50, 100]
CORPUS = parametric.CORPUS


def name(ev, amount=None):
    return f'ev{ev:+.2f}' + (f'-whites+{amount}' if amount else '')


def render(refs):
    cases = [{'name': 'default', 'settings': {}}]
    for ev in EXPOSURES:
        base = {'Exposure2012': f'{ev:+.2f}'}
        cases.append({'name': name(ev), 'settings': base})
        cases += [{'name': name(ev, a), 'settings': dict(base, Whites2012=f'+{a}')} for a in AMOUNTS]
    original = parametric.cases
    parametric.cases = lambda: cases
    try:
        parametric.render(refs)
    finally:
        parametric.cases = original


def curves(refs):
    data = json.loads(refs.read_text())
    layout = json.loads((CORPUS / 'charts/layout.json').read_text())['patches']
    out = {}
    for ev in EXPOSURES:
        for a in AMOUNTS:
            g = parametric.fit_curve(data[name(ev)], data[name(ev, a)], layout)
            out[ev, a] = contrast.from_gamma(np.interp(contrast.to_gamma(contrast.CENTRES), parametric.GRID, g))
    return out


def luminance_98(image):
    lum = contrast.encode(np.clip(parametric.decode(image.reshape(-1, 3)) @ np.array([0.2126, 0.7152, 0.0722]), 0, 1))
    return float(np.percentile(lum, 98))


def reduced(image, edge=512):
    h, w = image.shape[:2]
    k = edge / max(h, w)
    rh, rw = round(h * k), round(w * k)
    return np.array([[image[int(y * h / rh):int((y + 1) * h / rh), int(x * w / rw):int((x + 1) * w / rw)]
                      .reshape(-1, 3).mean(0) for x in range(rw)] for y in range(rh)])


def rawmakase_render(binary, source, out, exposure=None):
    """`source` rendered by RAWmakase into `out` (a .tif, or a .jpg for photos, whose
    metadata the TIFF writer may refuse), as encoded sRGB in 0-1."""
    args = [str(binary), 'render', str(source), str(out), '--overwrite', '--max-edge=2000']
    if exposure is not None:
        args.append(f'--exposure={exposure}')
    subprocess.run(args, check=True, capture_output=True)
    if out.suffix == '.jpg':
        from PIL import Image
        return np.asarray(Image.open(out).convert('RGB'), float) / 255
    return tiff16.read(out)


def table(refs, binary):
    out = curves(refs)
    print(f'pub(super) const WHITES_ADAPTIVE: [[[f32; 64]; 3]; {len(EXPOSURES)}] = [')
    for ev in EXPOSURES:
        print('    [')
        for a in TABLE_AMOUNTS:
            print('        [')
            for i in range(0, 64, 11):
                print('            ' + ', '.join(f'{v:.4f}' for v in out[ev, a][i:i + 11]) + ',')
            print('        ],')
        print('    ],')
    print('];')
    print(f'pub(super) const WHITES_EXPOSURES: [f32; {len(EXPOSURES)}] = '
          f'[{", ".join(f"{e:.2f}" for e in EXPOSURES)}];')
    if binary:
        with tempfile.TemporaryDirectory() as work:
            chart = Path(work) / 'chart.dng'
            chart.write_bytes((CORPUS / 'charts/synthetic-d65.dng').read_bytes())
            levels = [luminance_98(reduced(rawmakase_render(binary, chart, Path(work) / 'out.tif', ev)))
                      for ev in EXPOSURES]
        # Past the exposure where the chart's highlights turn white they no longer tell
        # exposures apart.
        n = next((i + 1 for i, v in enumerate(levels) if v > 0.998), len(levels))
        print(f'pub(super) const WHITES_HIGHLIGHTS: [f32; {n}] = [{", ".join(f"{v:.4f}" for v in levels[:n])}];')


def offset(refs, binary, highlights):
    corpus = os.environ.get('RAWMAKASE_CORPUS')
    if not corpus or not binary:
        sys.exit('Set RAWMAKASE_CORPUS and --rawmakase.')
    out = curves(refs)
    root = Path(corpus) / 'camera-raw-photos'
    grid = np.arange(-2, 1.001, 0.05)
    found = []
    for default in sorted(root.rglob('default.json')):
        cases = [(default.parent / f'Whites2012+{a}.json', a) for a in TABLE_AMOUNTS]
        if not all(p.exists() for p, _ in cases):
            continue
        base = contrast.blocks(default)

        def at(ev, a):
            j = int(np.clip(np.searchsorted(EXPOSURES, ev) - 1, 0, len(EXPOSURES) - 2))
            w = np.clip((ev - EXPOSURES[j]) / 0.25, 0, 1)
            t = out[EXPOSURES[j], a] * (1 - w) + out[EXPOSURES[j + 1], a] * w
            return lambda v: np.interp(v, contrast.CENTRES, t)
        error = [sum(np.abs(contrast.rgb_tone(base, at(ev, a)) - contrast.blocks(p)).mean() for p, a in cases)
                 for ev in grid]
        best = grid[int(np.argmin(error))]
        raw = Path(corpus) / 'raws' / json.loads(default.read_text())['raw']
        with tempfile.TemporaryDirectory() as work:
            level = luminance_98(reduced(rawmakase_render(binary, raw, Path(work) / 'out.jpg')))
        predicted = float(np.interp(level, highlights, EXPOSURES[:len(highlights)]))
        found.append(best - predicted)
        print(f'{default.parent.relative_to(root)}: best {best:+.2f} EV, highlights {level:.3f} '
              f'give {predicted:+.2f} EV')
    found = np.array(found)
    print(f'offset {found.mean():+.2f} EV; photos then within {found.std():.2f} EV')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('step', choices=['render', 'table', 'offset'])
    p.add_argument('--refs', type=Path, default=Path(tempfile.gettempdir()) / 'whites-refs.json',
                   help='patch means of the chart renders (kept outside the repository)')
    p.add_argument('--rawmakase', type=Path, help='a built rawmakase binary')
    p.add_argument('--highlights', type=float, nargs='*', help='WHITES_HIGHLIGHTS, for offset')
    args = p.parse_args()
    if args.step == 'render':
        render(args.refs)
    elif args.step == 'table':
        table(args.refs, args.rawmakase)
    else:
        offset(args.refs, args.rawmakase, args.highlights)


if __name__ == '__main__':
    main()
