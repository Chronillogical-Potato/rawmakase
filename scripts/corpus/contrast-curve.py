#!/usr/bin/env python3
"""Measure Camera Raw's Contrast and its photo-dependent pivot.

`render` has Photoshop 2026 (Camera Raw) render synthetic-d65.dng with Contrast at
±25, ±50, ±75 and ±100 and keeps the patch means in --refs (outside the repository).
It refuses to start while Photoshop has documents open.

`table` fits the chart's Contrast curves (as scripts/corpus/parametric-curve.py fits its
curves: brightest and darkest ProPhoto channel of every patch, gamma-2.2 Hermite
spline) and prints them as `CONTRAST_CHART` and `CONTRAST_PIVOT` for
crates/rawmakase-engine/src/develop/basic_tone_data.rs: 64 bin centres in encoded ProPhoto RGB, at the
engine's slider positions (-100, -50, -25, 25, 50, 100).

`pivot` needs RAWMAKASE_CORPUS with the Camera Raw photo references
(camera-raw-photos/, Contrast cases and default.json per photo). For each photo it
finds the pivot that moves the chart's curve onto Camera Raw's (a power warp of
gamma-2.2 encoded values, the same for every amount), then fits the pivot to the mean
encoded luminance of the default render's blocks and the middle of their 1st and 99th
percentiles, and prints the fit with its leave-one-out error. The chart's own pivot is
one more point.

Requires numpy. Run from the repository root:
  python3 scripts/corpus/contrast-curve.py render|table|pivot [--refs FILE]
"""
import argparse
import importlib.util
import json
import os
import sys
import tempfile
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True
HERE = Path(__file__).parent
_spec = importlib.util.spec_from_file_location('parametric', HERE / 'parametric-curve.py')
parametric = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(parametric)
charts = parametric.charts
CORPUS = parametric.CORPUS

AMOUNTS = [-100, -75, -50, -25, 25, 50, 75, 100]
TABLE_AMOUNTS = [-100, -50, -25, 25, 50, 100]
CENTRES = (np.arange(64) + 0.5) / 64
CONTRAST_CASES = {'contrast+50': 50, 'Contrast2012+25': 25, 'Contrast2012-25': -25, 'Contrast2012+50': 50,
                  'Contrast2012-50': -50, 'Contrast2012+100': 100, 'Contrast2012-100': -100}


def encode(v):
    v = np.clip(v, 0, None)
    return np.where(v <= 0.0031308, v * 12.92, 1.055 * v ** (1 / 2.4) - 0.055)


def to_gamma(v):
    return parametric.decode(np.clip(np.asarray(v, float), 0, 1)) ** (1 / parametric.GAMMA)


def from_gamma(w):
    return encode(np.clip(w, 0, 1) ** parametric.GAMMA)


def render(refs):
    cases = [{'name': 'default', 'settings': {}}] + [
        {'name': f'contrast{a:+d}', 'settings': {'Contrast2012': f'{a:+d}'}} for a in AMOUNTS]
    original = parametric.cases
    parametric.cases = lambda: cases
    try:
        parametric.render(refs)
    finally:
        parametric.cases = original


def curves(refs):
    """The chart's Contrast at each amount, in encoded ProPhoto RGB at CENTRES."""
    data = json.loads(refs.read_text())
    layout = json.loads((CORPUS / 'charts/layout.json').read_text())['patches']
    out = {}
    for a in AMOUNTS:
        g = parametric.fit_curve(data['default'], data[f'contrast{a:+d}'], layout)
        out[a] = from_gamma(np.interp(to_gamma(CENTRES), parametric.GRID, g))
    d = out[50] - CENTRES
    i = np.where((d[:-1] < 0) & (d[1:] >= 0))[0][-1]
    pivot = CENTRES[i] - d[i] * (CENTRES[i + 1] - CENTRES[i]) / (d[i + 1] - d[i])
    return out, float(pivot)


def table(refs):
    out, pivot = curves(refs)
    print('pub(super) const CONTRAST_CHART: [[f32; 64]; 6] = [')
    for a in TABLE_AMOUNTS:
        print('    [')
        for i in range(0, 64, 11):
            print('        ' + ', '.join(f'{v:.4f}' for v in out[a][i:i + 11]) + ',')
        print('    ],')
    print('];')
    print(f'pub(super) const CONTRAST_PIVOT: f32 = {pivot:.4f};')


def blocks(path):
    return np.asarray(json.loads(Path(path).read_text())['blocks'], float) / 65535


def pro(rgb):
    return encode(np.clip(parametric.decode(rgb) @ parametric.SRGB_TO_PRO.T, 0, 1))


def rgb_tone(base, f):
    p = pro(base)
    lo, hi = p.min(1, keepdims=True), p.max(1, keepdims=True)
    a, b = f(lo), f(hi)
    q = np.where(hi - lo > 1e-8, a + (b - a) * (p - lo) / np.maximum(hi - lo, 1e-8), a)
    back = np.linalg.inv(parametric.SRGB_TO_PRO)
    return encode(np.clip(parametric.decode(np.clip(q, 0, 1)) @ back.T, 0, 1))


def moved(curve, chart_pivot, pivot):
    k = np.log(to_gamma(chart_pivot)) / np.log(to_gamma(pivot))
    return lambda v: from_gamma(to_gamma(np.interp(from_gamma(to_gamma(v) ** k), CENTRES, curve)) ** (1 / k))


def statistics(rgb):
    lum = encode(np.clip(parametric.decode(rgb) @ np.array([0.2126, 0.7152, 0.0722]), 1e-5, 1))
    return lum.mean(), (np.percentile(lum, 1) + np.percentile(lum, 99)) / 2


def pivot_fit(refs):
    corpus = os.environ.get('RAWMAKASE_CORPUS')
    if not corpus:
        sys.exit('Set RAWMAKASE_CORPUS: the photo references stay outside the repository.')
    out, chart_pivot = curves(refs)
    root = Path(corpus) / 'camera-raw-photos'
    rows = []
    for default in sorted(root.rglob('default.json')):
        found = [(default.parent / f'{c}.json', a) for c, a in CONTRAST_CASES.items()
                 if (default.parent / f'{c}.json').exists()]
        if not found:
            continue
        base = blocks(default)
        candidates = np.arange(0.33, 0.8, 0.005)
        error = [sum(np.abs(rgb_tone(base, moved(out[a], chart_pivot, p)) - blocks(path)).mean()
                     for path, a in found) for p in candidates]
        rows.append((default.parent.relative_to(root), candidates[int(np.argmin(error))], *statistics(base)))
    data = json.loads(refs.read_text())
    chart = np.asarray(data['default'], float) / 65535
    x = np.array([[r[2], r[3], 1] for r in rows] + [[*statistics(chart), 1]])
    y = np.array([r[1] for r in rows] + [chart_pivot])
    fit = np.linalg.lstsq(x, y, rcond=None)[0]
    loo = [y[i] - x[i] @ np.linalg.lstsq(np.delete(x, i, 0), np.delete(y, i), rcond=None)[0] for i in range(len(y))]
    print(f'{len(rows)} photos and the chart; pivots {y.min():.3f}-{y.max():.3f} (spread {y.std():.3f})')
    print(f'pivot = {fit[2]:.3f} + {fit[0]:.3f} mean + {fit[1]:.3f} middle; '
          f'leave-one-out error {np.sqrt(np.mean(np.square(loo))):.3f}')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('step', choices=['render', 'table', 'pivot'])
    p.add_argument('--refs', type=Path, default=Path(tempfile.gettempdir()) / 'contrast-refs.json',
                   help='patch means of the chart renders (kept outside the repository)')
    args = p.parse_args()
    {'render': render, 'table': table, 'pivot': pivot_fit}[args.step](args.refs)


if __name__ == '__main__':
    main()
