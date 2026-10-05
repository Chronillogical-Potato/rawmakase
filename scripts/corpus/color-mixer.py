#!/usr/bin/env python3
"""Measure Camera Raw's color mixer on a dense synthetic chart and write
src/develop/color_mixer_chart.bin.

`chart` writes the fitting chart (1,728 colors: 72 hues every 5° in Oklab, at six
lightnesses and four fractions of the most chroma sRGB holds there, plus a gray
ramp) as a DNG of the synthetic camera under D65, through the ignored test
`write_fit_chart` in tests/color. The chart is not committed (--work keeps it).

`render` has Photoshop 2026 (Camera Raw) render the chart at its default and with
each band's Hue, Saturation and Luminance at ±100 (Blue and Purple also at ±50) as
16-bit ProPhoto RGB, and keeps the mean of every patch in a JSON file outside the
repository (--refs); the TIFFs are deleted as they are read. It refuses to start
while Photoshop has documents open.

`fit` refits the 48 band tables of color_mixer.bin (the change each slider at ±100
makes, as hue shift, log2 saturation and log2 value factors, on 36 hues × 6
saturations × 6 values of linear ProPhoto RGB), with Camera Raw's default render as
the mixer's input, and writes them, with Saturation and Vibrance's tables unchanged,
to color_mixer_chart.bin. Cells the chart does not reach keep the photo tables'
values. It prints how far each slider's table is from Camera Raw on the patches left
out of the fit (every other hue), before and after.

What the renders show (see docs/color-mixer.md#chart-tables):
- No band's slider moves a gray (the gray ramp changes by under 0.0001), so the
  tables leave the lowest saturation row's value change at zero.
- At ±50, Blue and Purple's Hue and Saturation follow the scaling of the ±100
  change the photo tables assumed (hue shift and positive log saturation linearly,
  negative saturation's factor linearly). Luminance does not: −50 makes about a
  third of −100's log value change and +50 about 58% of +100's, so the chart tables
  scale it by the slider position to the power 1.6 (darkening) or 0.79.

Requires numpy. Run from the repository root:
  python3 scripts/corpus/color-mixer.py chart [--work DIR]
  python3 scripts/corpus/color-mixer.py render [--work DIR] [--refs FILE]
  python3 scripts/corpus/color-mixer.py fit [--work DIR] [--refs FILE]
"""
import argparse
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / 'tests/corpus'
_spec = importlib.util.spec_from_file_location('charts', Path(__file__).parent / 'camera-raw-charts.py')
charts = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(charts)

BANDS = ['Red', 'Orange', 'Yellow', 'Green', 'Aqua', 'Blue', 'Purple', 'Magenta']
KINDS = ['Hue', 'Saturation', 'Luminance']
HUES, SATS, VALS = 36, 6, 6
CELLS = HUES * SATS * VALS
TABLES = 52
SCALE = 8000
ORIGINAL = ROOT / 'src/develop/color_mixer.bin'
OUTPUT = ROOT / 'src/develop/color_mixer_chart.bin'
# Regularization: smoothness between neighbouring cells, and pull toward the photo
# tables (which is all that constrains cells without chart colors).
SMOOTH, PRIOR = 0.03, 0.02
# Band Luminance's slider curve (measured on Blue and Purple at ±50 and ±100: the
# log value change at −50 is 0.32-0.33 of −100's, at +50 0.58 of +100's).
LUMINANCE_DARKEN, LUMINANCE_LIGHTEN = 1.6, 0.79
TEMPLATE = charts.TEMPLATE.replace('"sRGB IEC61966-2.1"', '"ProPhoto RGB"')

# Oklab (Björn Ottosson) and sRGB / ProPhoto matrices.
M1 = np.array([[0.8189330101, 0.3618667424, -0.1288597137], [0.0329845436, 0.9293118715, 0.0361456387],
               [0.0482003018, 0.2643662691, 0.6338517070]])
M2 = np.array([[0.2104542553, 0.7936177850, -0.0040720468], [1.9779984951, -2.4285922050, 0.4505937099],
               [0.0259040371, 0.7827717662, -0.8086757660]])
SRGB_TO_XYZ = np.array([[0.4124564, 0.3575761, 0.1804375], [0.2126729, 0.7151522, 0.0721750],
                        [0.0193339, 0.1191920, 0.9503041]])
BRADFORD_D65_D50 = np.array([[1.0478112, 0.0228866, -0.0501270], [0.0295424, 0.9904844, -0.0170491],
                             [-0.0092345, 0.0150436, 0.7521316]])
PRO_TO_XYZ = np.array([[0.7976749, 0.1351917, 0.0313534], [0.2880402, 0.7118741, 0.0000857],
                       [0.0, 0.0, 0.8252100]])
SRGB_TO_PRO = np.linalg.inv(PRO_TO_XYZ) @ BRADFORD_D65_D50 @ SRGB_TO_XYZ


def oklch_to_srgb(l, c, h):
    h = np.radians(h)
    lms = np.array([l, c * np.cos(h), c * np.sin(h)]) @ np.linalg.inv(M2).T
    return (lms ** 3) @ np.linalg.inv(M1).T @ np.linalg.inv(SRGB_TO_XYZ).T


def max_chroma(l, h):
    lo, hi = 0., 0.5
    for _ in range(40):
        m = (lo + hi) / 2
        v = oklch_to_srgb(l, m, h)
        lo, hi = (m, hi) if np.all(v >= 0) and np.all(v <= 1) else (lo, m)
    return lo


def chart_colors():
    """[(kind, linear sRGB)]: the gray ramp, then hue patches with their hue index."""
    colors = [('gray', [2 ** (-10 + 10.25 * i / 23)] * 3) for i in range(24)]
    for l in [0.3, 0.45, 0.6, 0.72, 0.84, 0.94]:
        for f in [0.2, 0.45, 0.7, 0.95]:
            for i in range(72):
                colors.append((f'hue{i}', oklch_to_srgb(l, f * max_chroma(l, 5 * i), 5 * i).tolist()))
    return colors


def chart(work):
    spec = work / 'mixer-chart.json'
    spec.write_text(json.dumps({'columns': 48, 'patch': 20, 'gap': 4, 'colors': [c for _, c in chart_colors()]}))
    subprocess.run(['cargo', 'test', '--locked', '--test', 'color', 'write_fit_chart', '--', '--ignored'],
                   cwd=ROOT, env=dict(os.environ, RAWMAKASE_FIT_CHART=str(spec)), check=True, stdout=subprocess.DEVNULL)
    print(f'Wrote {spec.with_suffix(".dng")}')


def cases():
    out = {'default': {}}
    for band in BANDS:
        amounts = [-100, -50, 50, 100] if band in ('Blue', 'Purple') else [-100, 100]
        for kind in KINDS:
            for a in amounts:
                out[f'{kind}-{band}{a:+d}'] = {f'{kind}Adjustment{band}': str(a)}
    return out


def render(work, refs):
    done = json.loads(refs.read_text()) if refs.exists() else {}
    todo = {k: v for k, v in cases().items() if k not in done}
    if not todo:
        return
    if not charts.photoshop_idle():
        sys.exit(f'{charts.PHOTOSHOP} is not running or has documents open; close them (another session may be using it).')
    base = json.loads((CORPUS / 'cases.json').read_text())['base']
    patches = json.loads((work / 'mixer-chart.patches.json').read_text())
    tmp = Path(tempfile.mkdtemp(prefix='color-mixer-'))
    jobs = [{'source': str(work / 'mixer-chart.dng'), 'dng': str(tmp / 'chart.dng'), 'xmp': str(tmp / 'chart.xmp'),
             'settings': charts.xmp(base, {'settings': s}, {}), 'out': str(tmp / 'out' / f'{k}.tif')}
            for k, s in todo.items()]
    script = tmp / 'render.jsx'
    script.write_text(TEMPLATE % {'jobs': json.dumps(jobs)})
    print(f'Rendering {len(jobs)} cases in {tmp}', flush=True)
    subprocess.run(['osascript', '-e', f'with timeout of 36000 seconds\ntell application "{charts.PHOTOSHOP}" '
                    f'to do javascript file (POSIX file "{script}")\nend timeout'], check=True, stdout=subprocess.DEVNULL)
    for k in todo:
        tiff = tmp / 'out' / f'{k}.tif'
        if tiff.exists():
            done[k] = charts.patch_values(tiff, patches)
        else:
            print(f'  missing render: {k}', file=sys.stderr)
    shutil.rmtree(tmp)
    refs.write_text(json.dumps(done))


def linear(v16):
    """16-bit ProPhoto RGB (gamma 1.8, as Photoshop's profile) to linear."""
    return (np.asarray(v16, float) / 65535) ** 1.8


def hsv(p):
    """As color_mixer::hsv: hue (turns), saturation and value of linear ProPhoto."""
    p = np.maximum(p, 0)
    mx, mn = p.max(1), p.min(1)
    d = mx - mn
    r, g, b = p.T
    with np.errstate(invalid='ignore', divide='ignore'):
        h = np.where(mx == r, ((g - b) / d) % 6, np.where(mx == g, (b - r) / d + 2, (r - g) / d + 4)) / 6
        s = np.where(mx > 1e-6, d / mx, 0)
    return np.stack([np.where(d < 1e-9, 0, h), s, mx], 1)


def from_hsv(h, s, v):
    h = (h % 1) * 6
    c = v * s
    x = c * (1 - np.abs(h % 2 - 1))
    z = np.zeros_like(h)
    i = np.floor(h).astype(int) % 6
    q = np.select([(i == k)[:, None] for k in range(5)],
                  [np.stack(t, 1) for t in [(c, x, z), (x, c, z), (z, c, x), (z, x, c), (x, z, c)]],
                  np.stack([c, z, x], 1))
    return q + (v - c)[:, None]


def weights(x):
    """Trilinear weights of each color over the grid cells, as color_mixer::lookup_with."""
    h, s, v = x.T
    fh = (h % 1) * HUES - 0.5
    fs = np.clip(np.sqrt(np.clip(s, 0, 1)) * SATS - 0.5, 0, SATS - 1)
    fv = np.clip(np.clip(v, 0, 1) ** 0.45 * VALS - 0.5, 0, VALS - 1)
    h0 = np.floor(fh)
    s0, v0 = fs.astype(int), fv.astype(int)
    s1, v1 = np.minimum(s0 + 1, SATS - 1), np.minimum(v0 + 1, VALS - 1)
    w = np.zeros((len(h), CELLS))
    rows = np.arange(len(h))
    for dh, wh in [(0, 1 - (fh - h0)), (1, fh - h0)]:
        hi = ((h0 + dh) % HUES).astype(int)
        for si, ws in [(s0, 1 - fs % 1), (s1, fs % 1)]:
            for vi, wv in [(v0, 1 - fv % 1), (v1, fv % 1)]:
                np.add.at(w, (rows, (hi * SATS + si) * VALS + vi), wh * ws * wv)
    return w


def strength(kind, amount, curve=True):
    """MixerModel::strength: band Luminance follows a power of the slider position
    (the photo tables, without `curve`, scale linearly)."""
    if kind == 'Luminance' and curve:
        return abs(amount) ** (LUMINANCE_DARKEN if amount < 0 else LUMINANCE_LIGHTEN)
    return abs(amount)


def scaled(table, kind, amount, curve=True):
    """A table at a slider position, as ColorMixer::new scales it."""
    w = strength(kind, amount, curve)
    t = table * w
    if amount < 0:
        t[1] = np.log2(np.maximum(1 + w * (2 ** table[1] - 1), 1e-3))
    return t


def apply(table, p):
    x = hsv(p)
    d = weights(x) @ table.T
    out = from_hsv(x[:, 0] + d[:, 0], np.clip(x[:, 1] * 2 ** d[:, 1], 0, 1), x[:, 2] * 2 ** d[:, 2])
    return np.where((x[:, 2] > 1e-6)[:, None], out, p)


def lab(pro):
    xyz = np.asarray(pro) @ PRO_TO_XYZ.T / np.array([0.96422, 1.0, 0.82521])
    f = np.where(xyz > (6 / 29) ** 3, np.cbrt(np.maximum(xyz, 0)), xyz / (3 * (6 / 29) ** 2) + 4 / 29)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]), 200 * (f[..., 1] - f[..., 2])], -1)


def de00(l1, l2):
    L1, a1, b1 = l1.T
    L2, a2, b2 = l2.T
    cb = (np.hypot(a1, b1) + np.hypot(a2, b2)) / 2
    g = 0.5 * (1 - np.sqrt(cb ** 7 / (cb ** 7 + 25 ** 7)))
    a1p, a2p = (1 + g) * a1, (1 + g) * a2
    c1p, c2p = np.hypot(a1p, b1), np.hypot(a2p, b2)
    h1p, h2p = np.degrees(np.arctan2(b1, a1p)) % 360, np.degrees(np.arctan2(b2, a2p)) % 360
    dhp = (h2p - h1p + 180) % 360 - 180
    dhp = np.where(c1p * c2p == 0, 0, dhp)
    dHp = 2 * np.sqrt(c1p * c2p) * np.sin(np.radians(dhp / 2))
    lbp, cbp = (L1 + L2) / 2, (c1p + c2p) / 2
    hs = h1p + h2p
    hbp = np.where(np.abs(h1p - h2p) > 180, (hs + 360) / 2, hs / 2)
    hbp = np.where(c1p * c2p == 0, hs, hbp)
    t = (1 - 0.17 * np.cos(np.radians(hbp - 30)) + 0.24 * np.cos(np.radians(2 * hbp))
         + 0.32 * np.cos(np.radians(3 * hbp + 6)) - 0.20 * np.cos(np.radians(4 * hbp - 63)))
    rt = -np.sin(np.radians(60 * np.exp(-((hbp - 275) / 25) ** 2))) * 2 * np.sqrt(cbp ** 7 / (cbp ** 7 + 25 ** 7))
    sl = 1 + 0.015 * (lbp - 50) ** 2 / np.sqrt(20 + (lbp - 50) ** 2)
    sc, sh = 1 + 0.045 * cbp, 1 + 0.015 * cbp * t
    dl, dc, dh = (L2 - L1) / sl, (c2p - c1p) / sc, dHp / sh
    return np.sqrt(dl ** 2 + dc ** 2 + dh ** 2 + rt * dc * dh)


def shown(pro):
    """Lab of a color as an sRGB output shows it (clipped per channel)."""
    s = np.clip(np.asarray(pro) @ np.linalg.inv(SRGB_TO_PRO).T, 0, 1)
    return lab(s @ SRGB_TO_PRO.T)


def table_index(band, kind, sign):
    return (BANDS.index(band) * 3 + KINDS.index(kind)) * 2 + int(sign > 0)


def smoothness():
    pairs = []
    for h in range(HUES):
        for s in range(SATS):
            for v in range(VALS):
                c = (h * SATS + s) * VALS + v
                pairs.append((c, (((h + 1) % HUES) * SATS + s) * VALS + v))
                if s + 1 < SATS:
                    pairs.append((c, (h * SATS + s + 1) * VALS + v))
                if v + 1 < VALS:
                    pairs.append((c, (h * SATS + s) * VALS + v + 1))
    d = np.zeros((len(pairs), CELLS))
    for i, (a, b) in enumerate(pairs):
        d[i, a], d[i, b] = 1, -1
    return d.T @ d


class Data:
    def __init__(self, refs):
        self.refs = json.loads(refs.read_text())
        kinds = [kind for kind, _ in chart_colors()]
        self.patches = np.array([i for i, kind in enumerate(kinds) if kind != 'gray'])
        self.hue_index = np.array([int(kinds[i][3:]) for i in self.patches])
        self.base = linear(self.refs['default'])[self.patches]
        self.x = hsv(self.base)
        self.w = weights(self.x)

    def render(self, name):
        return linear(self.refs[name])[self.patches]

    def observed(self, name):
        """Per patch: the hue shift, log2 saturation and log2 value factors Camera Raw
        applied, and how much each counts (hue and saturation are noise near gray;
        clipped channels hide the change)."""
        out = self.render(name)
        x = self.x
        ok = (x[:, 2] > 0.01) & (out.max(1) < 0.995) & (self.base.max(1) < 0.995)
        with np.errstate(divide='ignore', invalid='ignore'):
            change = np.stack([(hsv(out)[:, 0] - x[:, 0] + 0.5) % 1 - 0.5,
                               np.log2(np.maximum(hsv(out)[:, 1], 1e-4) / np.maximum(x[:, 1], 1e-4)),
                               np.log2(hsv(out)[:, 2] / x[:, 2])], 1)
        colorful = np.clip(x[:, 1] / 0.1, 0, 1)
        return change, [ok * colorful, ok * colorful * (x[:, 1] > 0.03), ok * 1.0]


def fit_table(data, prior, band, kind, sign, use, dtd):
    table = prior.copy()
    names = [(f'{kind}-{band}{sign * a:+d}', strength(kind, sign * a / 100)) for a in (50, 100)
             if f'{kind}-{band}{sign * a:+d}' in data.refs]
    for c in range(3):
        rows, ys, ws = [], [], []
        for name, f in names:
            change, weight = data.observed(name)
            y, scale = change[:, c], f
            if c == 1 and sign < 0:
                # Negative saturation scales the factor itself: 2^t = 1 + (2^y - 1) / f.
                with np.errstate(invalid='ignore', divide='ignore'):
                    y, scale = np.log2(np.maximum(1 + (2 ** y - 1) / f, 1e-3)), 1.
            keep = use & (weight[c] > 0) & np.isfinite(y)
            rows.append(data.w[keep] * scale)
            ys.append(y[keep])
            ws.append(weight[c][keep])
        a, y, w = np.vstack(rows), np.concatenate(ys), np.concatenate(ws)
        normal = a.T @ (a * w[:, None]) + SMOOTH * dtd + PRIOR * np.identity(CELLS)
        table[c] = np.linalg.solve(normal, a.T @ (w * y) + PRIOR * prior[c])
    # No slider moves a gray: the lowest saturation row keeps value.
    table[2].reshape(HUES, SATS, VALS)[:, 0, :] = 0
    return table


def fit(refs):
    data = Data(refs)
    original = np.frombuffer(ORIGINAL.read_bytes(), '<i2').astype(float).reshape(TABLES, 3, CELLS) / SCALE
    dtd = smoothness()
    held_out = data.hue_index % 2 == 1
    tables = original.copy()
    print(f'{"slider":24s} {"photo tables":>18s} {"chart tables":>18s}   mean ΔE00 (p95) on held-out patches')
    for band in BANDS:
        for kind in KINDS:
            for sign in (-1, 1):
                t = table_index(band, kind, sign)
                check = fit_table(data, original[t], band, kind, sign, ~held_out, dtd)
                tables[t] = fit_table(data, original[t], band, kind, sign, np.ones_like(held_out), dtd)
                for a in (50, 100):
                    name = f'{kind}-{band}{sign * a:+d}'
                    if name not in data.refs:
                        continue
                    ref = shown(data.render(name))[held_out]
                    errors = [de00(shown(apply(scaled(tb, kind, sign * a / 100, curve), data.base))[held_out], ref)
                              for tb, curve in ((original[t], False), (check, True))]
                    print(f'{name:24s} ' + ' '.join(f'{e.mean():10.2f} ({np.percentile(e, 95):4.2f})' for e in errors))
    values = np.clip(np.round(tables * SCALE), -32768, 32767).astype('<i2')
    OUTPUT.write_bytes(values.tobytes())
    print(f'Wrote {OUTPUT.relative_to(ROOT)}')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('command', choices=['chart', 'render', 'fit'])
    p.add_argument('--work', type=Path, default=Path(tempfile.gettempdir()) / 'rawmakase-color-mixer')
    p.add_argument('--refs', type=Path, help='patch means (default: WORK/refs.json)')
    args = p.parse_args()
    args.work.mkdir(parents=True, exist_ok=True)
    refs = args.refs or args.work / 'refs.json'
    if args.command == 'chart':
        chart(args.work)
    elif args.command == 'render':
        render(args.work, refs)
    else:
        fit(refs)


if __name__ == '__main__':
    main()
