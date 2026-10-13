"""Fit a rendered file's Temp/Tint tables (model/white_balance.rs: RENDERED_TEMPERATURE,
RENDERED_TINT) from per-file ProPhoto log2 gains that reproduce Lightroom renders of
synthetic JPEG charts. Usage: rendered-white-balance.py FITS.json [FITS.json ...]
Each FITS.json maps a file name ending in `__temp+25.jpg`, `__tint-50.jpg` or
`__temp+25_tint-50.jpg` to its [r, g, b] log2 gains (smallest 0). Prints both tables
and the combined model's error on the Temp x Tint files. The renders themselves are
not part of the repository."""
import json, re, sys
from pathlib import Path
import numpy as np
ROOT = Path(__file__).resolve().parents[2]
src = (ROOT / 'crates/rawmakase-model/src/camera_profiles/temperature.rs').read_text()
T = np.array([[float(v) for v in r] for r in re.findall(r'\[([-\d.]+), ([-\d.]+), ([-\d.]+), ([-\d.]+)\]', src.split('const TABLE')[1].split('= [',1)[1].split('];')[0])])
XYZ_TO_PRO = np.array([[1.345943, -0.255608, -0.051111], [-0.544599, 1.508167, 0.020535], [0, 0, 1.211813]])
def xy(mired, tint):  # temperature::xy with the reciprocal given directly (no clamp)
    r = mired
    i = next((i for i in range(30) if r < T[i + 1][0]), 29)
    a, b = T[i], T[i + 1]; f = (b[0] - r) / (b[0] - a[0])
    d = lambda s: np.array([1, s]) / np.sqrt(1 + s * s)
    d1, d2 = d(a[3]), d(b[3]); du, dv = d1 * f + d2 * (1 - f); L = np.hypot(du, dv)
    off = tint / -3000.
    u = a[1] * f + b[1] * (1 - f) + off * du / L; v = a[2] * f + b[2] * (1 - f) + off * dv / L
    den = u - 4 * v + 2; return 1.5 * u / den, v / den
def neutral(m, t):
    x, y = xy(m, t); return XYZ_TO_PRO @ np.array([x / y, 1, (1 - x - y) / y])
REF = neutral(200., 0.)
def gains(m, t):
    g = np.log2(REF / neutral(m, t)); return g - g.min()
def fit_mt(f, tint_fixed=None):
    f = np.array(f); best = (1e9, 0, 0)
    ms = np.arange(0, 560, 0.5)
    ts = [tint_fixed] if tint_fixed is not None else np.arange(-150, 150.5, 0.5)
    for t in ts:
        e = [np.abs(gains(m, t) - f).sum() for m in ms]; i = int(np.argmin(e))
        if e[i] < best[0]: best = (e[i], ms[i], t)
    return best
d = {}
FILES = sys.argv[1:]
for fn in FILES:
    for k, v in json.load(open(fn)).items(): d[(fn, k)] = v
def pick(kind, val):
    for fn in reversed(FILES):
        for (f, k), v in d.items():
            if f == fn and re.search(re.escape(f'__{kind}{val:+d}.jpg') + '$', k): return v
temp = {}
for s in (-100, -90, -75, -60, -50, -35, -25, -15, -10, -5, 5, 10, 15, 25, 35, 40, 45, 50, 55, 60, 65, 70, 75, 80, 90, 100):
    g = pick('temp', s)
    if g is None: continue
    # tint drift is fitted too, then mired refined with it
    e, m, t = fit_mt(g)
    temp[s] = (m - 200., t, e)
tint = {}
for n in (-100, -75, -50, -35, -25, -10, 10, 25, 35, 50, 75, 100):
    g = pick('tint', n)
    if g is None: continue
    best = min(((np.abs(gains(200., t) - np.array(g)).sum(), t) for t in np.arange(-200, 200.5, 0.25)))
    tint[n] = (best[1], best[0])
print('TEMP  s: mired_offset tint_drift resid'); [print(f'{s:+4d}: {m:+7.1f} {t:+6.1f} {e:.3f}') for s, (m, t, e) in sorted(temp.items())]
print('TINT  n: dng_tint resid'); [print(f'{n:+4d}: {t:+7.2f} {e:.3f}') for n, (t, e) in sorted(tint.items())]
# validate the grid: combine tables, compare with fitted gains of combo files
def interp(tab, x):
    ks = sorted(tab); xs = [0] + ks; 
    pts = sorted([(0, tab0 := ([0, 0] if isinstance(next(iter(tab.values())), list) else 0))] + [(k, tab[k]) for k in ks])
    X = [p[0] for p in pts]; Y = np.array([p[1] for p in pts], dtype=float)
    return np.array([np.interp(x, X, Y[:, j]) for j in range(Y.shape[1])]) if Y.ndim > 1 else np.interp(x, X, Y)
tt = {s: [m, t] for s, (m, t, e) in temp.items()}; nn = {n: t for n, (t, e) in tint.items()}
print('GRID check (sum |log2| error, combined model)')
for (fn, k), g in sorted(d.items()):
    mm = re.search(r'__temp([-+]\d+)_tint([-+]\d+)\.jpg$', k)
    if not mm: continue
    s, n = int(mm.group(1)), int(mm.group(2))
    mo, drift = interp(tt, s); tn = interp(nn, n)
    print(f'  {k:34s} {np.abs(gains(200 + mo, drift + tn) - np.array(g)).sum():.3f}')
