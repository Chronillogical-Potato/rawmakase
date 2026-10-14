"""Measure a rendered file's Temp/Tint tables (model/white_balance.rs: RENDERED_TEMPERATURE,
RENDERED_TINT) from Lightroom renders of synthetic JPEG charts.
Usage: rendered-white-balance.py GAINS.json [GAINS.json ...]
Each GAINS.json maps a file name ending in `__temp+25.jpg`, `__tint-50.jpg` or
`__temp+25_tint-50.jpg` to its [r, g, b] ProPhoto log2 gains: Lightroom's export over
the file, each through F⁻¹ (develop::rendered::scene_from_rendered), averaged over the
chart's greys whose export channels are all between 0.002 and 0.5 (brighter greys reach
Lightroom's highlight compression). The renders themselves are not part of the
repository.

Lightroom treats the pseudo-scene as a camera whose RGB is linear ProPhoto, as the DNG
SDK would: the gains are the file's D50 white Bradford-adapted from the white Temp and
Tint name to D50, scaled by that white's largest ProPhoto channel. Each file's gains
therefore give its white; this prints that white as a DNG reciprocal temperature and
tint, both tables, and the tables' error against every file's gains. Greys need only
that; colours and the cool magenta corner need the whole of
white_balance::rendered_white_balance (a matrix with the SDK's limits, and a clip)."""
import json, re, sys
from pathlib import Path
import numpy as np
ROOT = Path(__file__).resolve().parents[2]
src = (ROOT / 'crates/rawmakase-model/src/camera_profiles/temperature.rs').read_text()
T = np.array([[float(v) for v in r] for r in re.findall(r'\[([-\d.]+), ([-\d.]+), ([-\d.]+), ([-\d.]+)\]', src.split('const TABLE')[1].split('= [',1)[1].split('];')[0])])
XYZ_TO_PRO = np.array([[1.345943, -0.255608, -0.051111], [-0.544599, 1.508167, 0.020535], [0, 0, 1.211813]])
BRADFORD = np.array([[0.8951, 0.2664, -0.1614], [-0.7502, 1.7135, 0.0367], [0.0389, -0.0685, 1.0296]])
D50 = np.linalg.solve(XYZ_TO_PRO, np.ones(3))
def xy(mired, tint):  # temperature::xy_reciprocal
    i = next((i for i in range(30) if mired < T[i + 1][0]), 29)
    a, b = T[i], T[i + 1]; f = (b[0] - mired) / (b[0] - a[0])
    d = lambda s: np.array([1, s]) / np.sqrt(1 + s * s)
    du, dv = d(a[3]) * f + d(b[3]) * (1 - f); L = np.hypot(du, dv)
    off = tint / -3000.
    u = a[1] * f + b[1] * (1 - f) + off * du / L; v = a[2] * f + b[2] * (1 - f) + off * dv / L
    den = u - 4 * v + 2; return np.array([1.5 * u / den, v / den])
def white_xyz(mired, tint):
    x, y = xy(mired, tint); return np.array([x / y, 1, (1 - x - y) / y])
def gains(white):  # white_balance::camera_white_balance, log2
    cone = BRADFORD @ white; d50 = BRADFORD @ D50
    adapted = np.linalg.solve(BRADFORD, d50 * d50 / cone)
    return np.log2((XYZ_TO_PRO @ white).max() * (XYZ_TO_PRO @ adapted))
def white_of(g):  # the white whose gains have the shape of g
    cone = BRADFORD @ np.linalg.solve(XYZ_TO_PRO, 2. ** np.asarray(g))
    return np.linalg.solve(BRADFORD, (BRADFORD @ D50) ** 2 / cone)
def mired_tint(white):  # nearest DNG reciprocal temperature and tint
    target = white[:2] / white.sum()
    err = lambda p: np.sum((xy(*p) - target) ** 2)
    p = min(((m, t) for m in np.arange(10, 600, 2.) for t in np.arange(-200, 200, 2.)), key=err)
    p, step = np.array(p, float), np.array([1., 1.])
    while step.max() > 1e-3:
        moved = False
        for i in range(2):
            for s in (-1, 1):
                q = p.copy(); q[i] += s * step[i]
                if err(q) < err(p): p, moved = q, True
        if not moved: step /= 2
    return p
files = {}
for fn in sys.argv[1:]:
    files.update(json.load(open(fn)))
def setting(name, kind):
    m = re.search(rf'{kind}([-+]\d+)', name); return int(m.group(1)) if m else 0
measured = {k: (setting(k, 'temp'), setting(k, 'tint'), mired_tint(white_of(g))) for k, g in files.items()}
d50_mired, d50_tint = mired_tint(D50)
temp, tint = {0: [(d50_mired, d50_tint)]}, {0: [d50_tint]}
for s, n, (m, t) in measured.values():
    if n == 0 and s: temp.setdefault(s, []).append((m, t))
    if s == 0 and n: tint.setdefault(n, []).append(t)
temp = {s: (np.mean([m for m, _ in v]), np.mean([t for _, t in v]) - d50_tint) for s, v in temp.items()}
tint = {n: np.mean(v) for n, v in tint.items()}
print(f'D50: mired {d50_mired:.2f}, DNG tint {d50_tint:.2f}')
print('RENDERED_TEMPERATURE [Temp, mired, tint drift]')
for s in sorted(temp): print(f'    [{s}., {temp[s][0]:.1f}, {temp[s][1] if abs(temp[s][1]) >= 0.2 else 0.:.1f}],')
print('RENDERED_TINT [Tint, DNG tint]')
for n in sorted(tint): print(f'    [{n}., {tint[n]:.2f}],')
def table_gains(s, n):
    ks = sorted(temp); m = np.interp(s, ks, [temp[k][0] for k in ks]); drift = np.interp(s, ks, [temp[k][1] for k in ks])
    kn = sorted(tint); t = np.interp(n, kn, [tint[k] for k in kn])
    return gains(white_xyz(m, t + drift)) - gains(white_xyz(d50_mired, d50_tint))
print('Tables against each file (largest |log2| error per channel)')
for k, (s, n, (m, t)) in sorted(measured.items(), key=lambda kv: kv[1][:2]):
    print(f'  {k:36s} mired {m:6.1f} tint {t:+7.2f}  {np.abs(table_gains(s, n) - np.array(files[k])).max():.3f}')
