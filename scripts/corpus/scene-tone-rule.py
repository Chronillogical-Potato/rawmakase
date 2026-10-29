#!/usr/bin/env python3
"""The global rule of positive Shadows and negative Highlights (`local_tone_rule.rs`).

Camera Raw's gain for a base level is a function of where that level sits in the
photo's histogram of base levels, measured on flat-patch mosaics whose histograms are
controlled (docs/scene-tone-stage.md#the-global-rule). The rule is a small network
from 20 histogram descriptors of a level to the log2 gain at Shadows +100 and
Highlights −100, trained on those mosaics and on linear-profile training photos.

  probes ROOT                 writes the mosaic DNGs under ROOT/probes and renders them
                              in Camera Raw (camera_raw.py), skipping renders that exist
  photos ROOT RAW...          converts RAWs with a linear profile (linear_profile.py) into
                              ROOT/photos/dng and renders them in Camera Raw with that
                              profile into ROOT/photos/crlin (default, S+100, H−100)
  taps ROOT RAWMAKASE         dumps the engine's measurement map of every probe and photo
                              into ROOT/maps (RAWMAKASE_RULE_DUMP; needs RAWMAKASE_DATA_DIR)
  train ROOT [--cv] [--out]   builds the samples, optionally cross-validates by photo, fits
                              the network on everything and writes the JSON and
                              crates/rawmakase-engine/src/develop/local_tone_rule_data.rs

Mosaics are 1024-pixel scenes of flat square patches (W* = 1, DefaultBlackRender
None) with faint blob texture, since a perfectly flat maximum is a degenerate case
for Camera Raw: any texture on the brightest patch, even 0.03 stop, changes every
dark patch's lift by about 0.4 EV. Families: random histograms at a fixed and at a
random texture amplitude, histograms sampled from the training photos' base levels,
small bright minorities above a gap, a few tiny bright spots in a dark field, deep
ranges of 10.5 to 13.5 stops (the Shadows lift keeps growing to about 4.3 EV twelve
stops under the top), bimodal histograms with most of the frame 9 to 12 stops down
and the rest 2 to 5 stops down (Camera Raw lifts the deep cluster 3 to 5 EV and the
mid cluster only 0.1 to 0.3), and top patches clipped in every channel, in green
only or just under white (Highlights pulls them all alike), smooth skies of varying
cloud texture and extent over textured ground, dark regions of varying texture under a
fixed sky (Camera Raw lifts a smooth dark region far less than a rough one at the same
depth, which no histogram can see: hence the per-level texture inputs), and coloured
patches of equal luminance (the pull is colour-blind).

Targets count down to 13 stops under white (`targets`): with a 9-stop floor the
network never saw the deepest tones and under-lifted them by 1.5 EV on interiors.
Weighting the photos up, or a wider and longer-trained network, traded the deep
tones of dark-heavy photos against the mid-shadows of ordinary ones in both
directions on the stress sets; this configuration had the lowest mean error there
(2026-10-28, docs/parity-gaps.md).

Features (`Photo.features`, mirrored by `RuleStats::features`): top − x, x − bottom,
mean − x, five quantiles − x, the CDF at x, the range, the mass within eight bands
of stops around x, two global texture measures clamped so the network cannot
extrapolate on amplitude, and the fine and coarse texture of the pixels at the level
itself (median absolute residuals, so edge pixels do not count). Requires numpy, scipy, tifffile, Pillow and mlx.
"""
import argparse
import json
import os
import pickle
import subprocess
import sys
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True
HERE = Path(__file__).parent
sys.path.insert(0, str(HERE))
import camera_raw  # noqa: E402
import linear_profile  # noqa: E402
import probe_dng  # noqa: E402
import probe_scenes  # noqa: E402

ROOT = HERE.resolve().parents[1]
DATA = ROOT / 'crates/rawmakase-engine/src/develop/local_tone_rule_data.rs'
YW = np.array([0.2880402, 0.7118741, 0.0000857])    # ProPhoto luminance
N = 1024
SETTINGS = {'default': {}, 'S+100': {'Shadows2012': '100'}, 'H-100': {'Highlights2012': '-100'}}
PHOTO_WEIGHT = 1    # photo samples against mosaic samples in the fit
HIDDEN = 64         # units in each of the two hidden layers
MEMBERS = 4         # networks in the ensemble (seeds 0..MEMBERS-1), averaged by the engine
# `local_tone.rs`: the measurement copy's long edge, the map's floor, the guided filter.
MAP, FLOOR, EPS, RADIUS = 512, 2.0 ** -14, 0.5, 0.032
TEX_CLAMP, TEX_TOP_CLAMP = 0.6, 0.05
# Per-level texture (`RuleStats::texture_at`): the median absolute fine (radius 2) and
# coarse (radius COARSE_RADIUS) residual of the pixels within LEVEL_WINDOW of the level.
LEVEL_WINDOW, COARSE_RADIUS, TEX_AT_CLAMP, LEVEL_MIN = 0.5, 12, 1.0, 50
FEAT = ['top-x', 'x-bot', 'mean-x', 'p50-x', 'p75-x', 'p90-x', 'p25-x', 'p10-x', 'cdf', 'range',
        'm(0,1]', 'm(1,2]', 'm(2,3]', 'm(3,5]', 'm(-1,0]', 'm(-2,-1]', 'm(-3,-2]', 'm(-5,-3]', 'tex', 'tex_top',
        'tex_at', 'coarse_at']


# --- the engine's measurement pipeline, in numpy -------------------------------------

def reduce(lum, edge=MAP):
    from PIL import Image
    h, w = lum.shape
    s = edge / max(h, w)
    if s >= 1:
        return lum
    size = (max(1, round(w * s)), max(1, round(h * s)))
    return np.asarray(Image.fromarray(lum.astype(np.float32), 'F').resize(size, Image.BOX), dtype=np.float64)


def box(x, r):
    from scipy.ndimage import uniform_filter
    return uniform_filter(x, size=2 * r + 1, mode='nearest')


def guided(logs):
    h, w = logs.shape
    r = max(1, round(RADIUS * max(h, w)))
    m, m2 = box(logs, r), box(logs * logs, r)
    var = np.maximum(m2 - m * m, 0)
    a = var / (var + EPS)
    return box(a, r) * logs + box(m - a * m, r)


class Photo:
    """Histogram statistics of one image's measurement copy, from linear luminance."""

    def __init__(self, lum=None, logs=None, base=None):
        if logs is None:
            small = reduce(lum)
            logs = np.log2(np.maximum(small, FLOOR))
            base = guided(logs)
        self.logs, self.base = np.asarray(logs, dtype=np.float64), np.asarray(base, dtype=np.float64)
        b = np.sort(self.base.ravel())
        self.sorted = b
        n = b.size
        q = lambda p: b[min(n - 1, int(p * (n - 1)))]
        self.top, self.bot, self.mean = b[-1], q(0.001), b.mean()
        self.q = {p: q(p) for p in (0.1, 0.25, 0.5, 0.75, 0.9)}
        d = self.logs - box(self.logs, 2)
        self.tex = float(np.sqrt(np.mean(d * d)))
        top = self.base > self.top - 2
        self.tex_top = float(np.sqrt(np.mean(d[top] ** 2))) if top.any() else self.tex
        # The residuals in order of base level, for the per-level medians; a median
        # ignores the few edge pixels whose residual spans stops.
        order = np.argsort(self.base.ravel(), kind='stable')
        self.fine_by_level = np.abs(d.ravel())[order]
        self.coarse_by_level = np.abs((self.logs - box(self.logs, COARSE_RADIUS)).ravel())[order]
        self.fine_median, self.coarse_median = float(np.median(self.fine_by_level)), float(np.median(self.coarse_by_level))

    def cdf(self, x):
        return np.searchsorted(self.sorted, x, side='right') / self.sorted.size

    def texture_at(self, x):
        """Fine and coarse texture of the pixels within LEVEL_WINDOW of each level in `x`."""
        x = np.atleast_1d(np.asarray(x, dtype=np.float64))
        lo, hi = np.searchsorted(self.sorted, x - LEVEL_WINDOW), np.searchsorted(self.sorted, x + LEVEL_WINDOW, side='right')
        fine, coarse = np.full(x.size, self.fine_median), np.full(x.size, self.coarse_median)
        for i, (l, h) in enumerate(zip(lo, hi)):
            if h - l >= LEVEL_MIN:
                fine[i], coarse[i] = np.median(self.fine_by_level[l:h]), np.median(self.coarse_by_level[l:h])
        return np.minimum(fine, TEX_AT_CLAMP), np.minimum(coarse, TEX_AT_CLAMP)

    def features(self, x):
        x = np.asarray(x, dtype=np.float64)
        c = self.cdf
        m = lambda lo, hi: c(x + hi) - c(x + lo)
        const = lambda v: np.full_like(x, v)
        fine, coarse = self.texture_at(x)
        return np.stack([self.top - x, x - self.bot, self.mean - x, self.q[0.5] - x, self.q[0.75] - x,
                         self.q[0.9] - x, self.q[0.25] - x, self.q[0.1] - x, c(x), const(self.top - self.bot),
                         m(0, 1), m(1, 2), m(2, 3), m(3, 5), m(-1, 0), m(-2, -1), m(-3, -2), m(-5, -3),
                         const(min(self.tex, TEX_CLAMP)), const(min(self.tex_top, TEX_TOP_CLAMP) / TEX_TOP_CLAMP),
                         fine, coarse], -1)


def targets(default_lum, setting_lum, lo=2 ** -13, hi=0.6, size=None):
    """log2 gain on the map (`size`: its height and width) and the pixels that count
    (Camera Raw neither near black nor in the white point's shoulder)."""
    from PIL import Image
    if size is None:
        d, s = reduce(default_lum), reduce(setting_lum)
    else:
        rs = lambda a: np.asarray(Image.fromarray(a.astype(np.float32), 'F').resize((size[1], size[0]), Image.BOX), dtype=np.float64)
        d, s = rs(default_lum), rs(setting_lum)
    return np.log2(np.maximum(s, FLOOR)) - np.log2(np.maximum(d, FLOOR)), (d > lo) & (d < hi)


# --- probes --------------------------------------------------------------------------

def blobs(sigma, seed):
    t = probe_scenes._blur(np.random.default_rng(seed).normal(0, 1, (N, N)), sigma)
    return t / t.std()


def mosaic(levels, patch, seed, amp, tex):
    n = N // patch
    order = np.random.default_rng(seed).permutation(n * n)
    log = np.empty((N, N))
    for k, idx in enumerate(order):
        i, j = divmod(k, n)
        sl = (slice(i * patch, (i + 1) * patch), slice(j * patch, (j + 1) * patch))
        log[sl] = levels[idx] + amp * tex[sl]
    return log


def histograms(rng, k, patch):
    """A random histogram of (N/patch)² levels: uniform, clustered, skewed or quantised."""
    n = (N // patch) ** 2
    top = np.log2(rng.uniform(0.1, 0.5))
    r = rng.uniform(2, 10)
    kind = ['uniform', 'clusters', 'skew', 'quantised'][k % 4]
    if kind == 'uniform':
        lv = rng.uniform(top - r, top, n)
    elif kind == 'clusters':
        c = rng.integers(2, 4)
        centers, widths = np.sort(rng.uniform(top - r, top, c)), rng.uniform(0.2, 1.5, c)
        which = rng.integers(0, c, n)
        lv = np.clip(centers[which] + rng.normal(0, 1, n) * widths[which], top - r, top)
    elif kind == 'skew':
        a = rng.uniform(0.3, 3)
        lv = top - r * rng.beta(a, 1 / a, n)
    else:
        lv = rng.choice(np.sort(rng.uniform(top - r, top, rng.integers(3, 9))), n)
    lv[0] = top
    return lv


def minority(rng, patch, spots):
    """A dark textured majority under a gap, with a small bright minority (`spots`: a few
    32-pixel patches) or a minority of up to 12% of the frame."""
    n = (N // patch) ** 2
    top = np.log2(rng.uniform(0.15, 0.5))
    gap = rng.uniform(4, 9) if spots else rng.uniform(2, 8)
    nb = int(rng.integers(1, 11)) if spots else int(rng.integers(1, max(2, int(n * 0.12)) + 1))
    dark_w = rng.uniform(0.5, 3) if spots else rng.uniform(1, 4)
    lv = np.clip(top - gap - dark_w / 2 + rng.normal(0, dark_w / 2, n), top - 12, top - gap)
    if rng.random() < (0.5 if spots else 0.4):
        mid = int(rng.integers(n // 20, n // 3)) if spots else int(rng.integers(1, n // 4))
        lv[nb:nb + mid] = rng.uniform(top - gap, top - (1 if spots else 0.5), mid)
    lv[:nb] = top - rng.uniform(0, 0.5 if spots else rng.uniform(0.1, 1.0), nb)
    return lv


def rgb(log):
    """A log2 luminance image as the neutral linear image the DNG is written from."""
    return np.repeat((2.0 ** log)[:, :, None], 3, 2)


def deep(rng, k):
    """64 levels over 10.5 to 13.5 stops, uniform or skewed to the top."""
    top, r = rng.uniform(-1.0, -0.3), rng.uniform(10.5, 13.5)
    lv = top - r * (rng.beta(1, 1, 64) if k % 2 == 0 else rng.beta(0.6, 1.4, 64))
    lv[0], lv[1] = top, top - r
    return lv


def bimodal(rng):
    """A deep cluster (9 to 12 stops under the top, 30 to 70% of the frame) and a mid
    cluster 2 to 5 stops under it."""
    top, nd = rng.uniform(-1.0, -0.3), int(round(64 * rng.uniform(0.3, 0.7)))
    dc, dw, mc, mw = rng.uniform(9, 12), rng.uniform(0.5, 2), rng.uniform(2.5, 4.5), rng.uniform(0.3, 1.0)
    lv = np.concatenate([top - np.clip(dc + rng.normal(0, dw, nd), 8, 13.5),
                         top - np.clip(mc + rng.normal(0, mw, 64 - nd), 1.5, 5.5)])
    lv[0] = top
    return lv


def clipped(rng, k):
    """64 levels over 6 stops below -1, with 5, 15 or 30% of the patches clipped in every
    channel, in green only, or set just under white."""
    kind, frac = ['all', 'green', 'near'][k % 3], [0.05, 0.15, 0.30][(k // 3) % 3]
    lv = -1 - 6 * rng.uniform(0, 1, 64)
    lv[0] = -1.0
    log, where = mosaic_where(lv, 128, k, 0.05, blobs(4, 5))
    img = rgb(log)
    sel = np.isin(where, np.arange(1, 1 + max(1, int(round(frac * 64)))))
    img[sel] = {'all': 1.3, 'green': [0.6, 1.3, 0.6], 'near': 0.95}[kind]
    return img


def sky_scene(a, f, d, seed):
    """A smooth bright region (fraction `f` of the frame, cloud texture of amplitude `a`)
    over dark textured ground `d` stops below it."""
    rng = np.random.default_rng(seed)
    top, h = rng.uniform(-0.8, -0.3), int(N * f)
    log = np.empty((N, N))
    log[:h] = top + a * blobs(32, seed + 1)[:h]
    log[h:] = top - d - 1.0 + 0.45 * blobs(4, seed + 2)[h:] + 1.0 * np.clip(blobs(24, seed + 3)[h:], -1, 1)
    return np.clip(log, -12, 0)


def ground_scene(a, d, broad, seed):
    """A fixed textured sky (30% of the frame) over a dark region `d` stops down whose fine
    texture amplitude is `a`, with or without broad shapes."""
    rng = np.random.default_rng(seed)
    top, h = rng.uniform(-0.8, -0.3), int(N * 0.3)
    log = np.empty((N, N))
    log[:h] = top + 0.1 * blobs(32, seed + 1)[:h]
    log[h:] = top - d + a * blobs(3, seed + 2)[h:] + broad * np.clip(blobs(24, seed + 3)[h:], -1, 1) * 0.7
    return np.clip(log, -13, 0)


CHROMA = {'neutral': (1, 1, 1.), 'blue': (0.55, 0.75, 1.6), 'blue2': (0.3, 0.6, 2.2), 'orange': (1.6, 0.9, 0.4),
          'green': (0.6, 1.3, 0.5), 'cyan': (0.5, 1.1, 1.4)}


def chroma_scene(k):
    """64 patches over 6 stops, each coloured at equal luminance (the pull is colour-blind;
    the family keeps the fit honest about that)."""
    rng = np.random.default_rng(8000 + k)
    top = rng.uniform(-1.0, -0.3)
    lv = top - 6 * rng.uniform(0, 1, 64)
    lv[0] = top
    names = list(CHROMA)
    log, where = mosaic_where(lv, 128, k, 0.05, blobs(4, 5))
    img = rgb(log)
    for i in range(64):
        c = np.array(CHROMA[names[(i + k) % len(names)]])
        img[where == i] *= c / (c @ YW) * (np.ones(3) @ YW)
    return np.clip(img, 0, 0.98)


def mosaic_where(levels, patch, seed, amp, tex):
    """`mosaic` and the patch index of every pixel."""
    n = N // patch
    order = np.random.default_rng(seed).permutation(n * n)
    log, where = np.empty((N, N)), np.empty((N, N), int)
    for k, idx in enumerate(order):
        i, j = divmod(k, n)
        sl = (slice(i * patch, (i + 1) * patch), slice(j * patch, (j + 1) * patch))
        log[sl], where[sl] = levels[idx] + amp * tex[sl], idx
    return log, where


def probe_jobs(root, photos_dir=None):
    """Every mosaic family: (name, patch, linear image)."""
    out = []
    for k in range(20):
        out.append((f'deep-{k:02d}', 128, rgb(mosaic(deep(np.random.default_rng(5000 + k), k), 128, k, 0.05, blobs(4, 5)))))
    for k in range(18):
        out.append((f'clip-{k:02d}', 128, clipped(np.random.default_rng(6000 + k), k)))
    for k in range(16):
        out.append((f'bimodal-{k:02d}', 128, rgb(mosaic(bimodal(np.random.default_rng(7000 + k)), 128, k, 0.05, blobs(4, 5)))))
    k = 0
    for amps, fracs, depths in (([0.0, 0.02, 0.05, 0.1, 0.2, 0.4], [0.25, 0.45, 0.65], [3.0, 5.0]),
                                ([0.6, 0.9, 1.3], [0.15, 0.35, 0.55], [2.0, 4.0])):
        for a in amps:
            for f in fracs:
                for d in depths:
                    out.append((f'sky-{k:02d}', 0, rgb(sky_scene(a, f, d, 9000 + k))))
                    k += 1
    k = 0
    for a in [0.02, 0.1, 0.3, 0.7]:
        for d in [4.0, 7.0, 10.0]:
            for broad in [0.0, 1.0]:
                out.append((f'ground-{k:02d}', 0, rgb(ground_scene(a, d, broad, 9500 + k))))
                k += 1
    for k in range(6):
        out.append((f'chroma-{k:02d}', 128, chroma_scene(k)))
    for k in range(60):
        rng = np.random.default_rng(2026 + k)
        patch = 256 if k % 2 == 0 else 128
        out.append((f'rand-{k:02d}', patch, rgb(mosaic(histograms(rng, k, patch), patch, k, 0.05, blobs(4, 5)))))
    for k in range(60):
        rng = np.random.default_rng(3030 + k)
        patch = 256 if k % 2 == 0 else 128
        amp = float(np.exp(rng.uniform(np.log(0.02), np.log(1.0))))
        sigma = int(rng.choice([2, 4, 8, 16]))
        out.append((f'amp-{k:02d}', patch, rgb(mosaic(histograms(rng, k, patch), patch, 1000 + k, amp, blobs(sigma, 100 + sigma)))))
    for k in range(40):
        rng = np.random.default_rng(909 + k)
        patch = 64 if k % 2 == 0 else 128
        amp = float(np.exp(rng.uniform(np.log(0.05), np.log(0.6))))
        sigma = int(rng.choice([2, 4, 8]))
        out.append((f'minority-{k:02d}', patch, rgb(mosaic(minority(rng, patch, False), patch, k, amp, blobs(sigma, 500 + sigma)))))
    for k in range(30):
        rng = np.random.default_rng(4242 + k)
        amp = float(np.exp(rng.uniform(np.log(0.05), np.log(0.5))))
        sigma = int(rng.choice([2, 4]))
        out.append((f'spots-{k:02d}', 32, rgb(mosaic(minority(rng, 32, True), 32, k, amp, blobs(sigma, 700 + sigma)))))
    # Histograms of the training photos' base levels, two mosaics each.
    for npz in sorted((photos_dir or root / 'maps').glob('photo*.npz'))[:26]:
        m = np.load(npz)
        ph = Photo(logs=m['logs'], base=m['base'])
        q = np.sort(ph.base.ravel())
        rng = np.random.default_rng(77 + int(npz.stem[5:]))
        for amp, sigma, patch in [(0.1, 3, 128), (0.3, 6, 256)]:
            n = (N // patch) ** 2
            levels = np.quantile(q, (np.arange(n) + 0.5) / n) - ph.top + np.log2(rng.uniform(0.15, 0.5))
            out.append((f'photo-{npz.stem[5:]}-a{amp}', patch, rgb(mosaic(levels, patch, int(npz.stem[5:]), amp, blobs(sigma, 300 + sigma)))))
    return out


def probes(args):
    d = args.root / 'probes'
    d.mkdir(parents=True, exist_ok=True)
    jobs = []
    for name, _, image in probe_jobs(args.root):
        source = d / f'{name}.dng'
        if not source.exists():
            probe_dng.write(source, image, baseline_exposure=0., black_render=1)
        jobs += [dict(source=str(source), settings=s, out=str(d / f'{name}-{n}.tif')) for n, s in SETTINGS.items()]
    failures = camera_raw.render(jobs, work=d / 'work')
    print(f'{len(jobs)} renders, {len(failures)} failed')


def photos(args):
    dng_dir, cr_dir = args.root / 'photos' / 'dng', args.root / 'photos' / 'crlin'
    dng_dir.mkdir(parents=True, exist_ok=True)
    cr_dir.mkdir(exist_ok=True)
    jobs = []
    for i, raw in enumerate(args.raws):
        dng = dng_dir / f'{i:02d}.dng'
        if not dng.exists():
            tmp = dng_dir / 'tmp'
            tmp.mkdir(exist_ok=True)
            subprocess.run([sys.executable, '-I', str(HERE / 'linear_profile.py'), str(tmp), str(raw)], check=True)
            next(tmp.glob('*.dng')).rename(dng)
        for n, s in SETTINGS.items():
            jobs.append(dict(source=str(dng), settings={'CameraProfile': linear_profile.NAME, **s},
                             out=str(cr_dir / f'{i:02d}-{n}.tif'), edge=2048))
    failures = camera_raw.render(jobs, work=args.root / 'photos' / 'work')
    print(f'{len(jobs)} renders, {len(failures)} failed')


def engine_map(rawmakase, dng, out):
    """The engine's own measurement map of `dng` (log luminance and base level, 512 px),
    dumped by `local_tone_rule.rs` under RAWMAKASE_RULE_DUMP, as `out` (npz)."""
    if out.exists():
        return
    tmp = Path(f'/tmp/rule-map-{os.getpid()}')
    for p in (tmp, tmp.with_suffix('.map')):
        p.unlink(missing_ok=True)
    env = dict(os.environ, RAWMAKASE_RULE_DUMP=str(tmp))
    subprocess.run([str(rawmakase), 'render', str(dng), str(tmp.with_suffix('.tif')), '--tap', 'scene-input',
                    '--overwrite', '--xmp', str(tmp.with_suffix('.xmp'))], env=env, capture_output=True)
    raw = np.fromfile(tmp.with_suffix('.map'), dtype=np.float32)
    w, h = int(raw[0]), int(raw[1])
    n = w * h
    np.savez(out, logs=raw[2:2 + n].reshape(h, w), base=raw[2 + n:2 + 2 * n].reshape(h, w))


def taps(args):
    """The engine's measurement maps of every probe and photo (what the rule reads)."""
    out = args.root / 'maps'
    out.mkdir(parents=True, exist_ok=True)
    xmp = Path(f'/tmp/rule-map-{os.getpid()}.xmp')
    xmp.write_text(camera_raw.xmp({'Shadows2012': '100'}))
    for name, _, _ in probe_jobs(args.root):
        engine_map(args.rawmakase, args.root / 'probes' / f'{name}.dng', out / f'{name}.npz')
    for dng in sorted((args.root / 'photos' / 'dng').glob('*.dng')):
        engine_map(args.rawmakase, dng, out / f'photo{dng.stem}.npz')


# --- training ------------------------------------------------------------------------

def cr_linear(path):
    import tifffile
    a = tifffile.imread(str(path)).astype(np.float64)[..., :3] / 65535.
    return (a ** 1.8) @ YW


def samples(key, root, d0, dS, dH, rng, n=2500):
    m = np.load(root / 'maps' / f'{key}.npz')
    ph = Photo(logs=m['logs'], base=m['base'])
    gS, vS = targets(d0, dS, size=ph.base.shape)
    gH, vH = targets(d0, dH, size=ph.base.shape)
    idx = np.flatnonzero((vS & vH).ravel())
    idx = rng.choice(idx, min(n, idx.size), replace=False)
    return dict(key=key, X=ph.features(ph.base.ravel()[idx]), Y=np.stack([gS.ravel()[idx], gH.ravel()[idx]], 1))


def load(root):
    cache = root / 'samples.pkl'
    if cache.exists():
        return pickle.loads(cache.read_bytes())
    rng = np.random.default_rng(0)
    data = []
    d = root / 'probes'
    for name, _, _ in probe_jobs(root):
        if not (d / f'{name}-H-100.tif').exists() or not (root / 'maps' / f'{name}.npz').exists():
            continue
        d0 = cr_linear(d / f'{name}-default.tif')
        data.append(dict(samples(name, root, d0, cr_linear(d / f'{name}-S+100.tif'), cr_linear(d / f'{name}-H-100.tif'), rng), kind='mosaic'))
    for npz in sorted((root / 'maps').glob('photo*.npz')):
        i = npz.stem[5:]
        cr = root / 'photos' / 'crlin'
        if not (cr / f'{i}-H-100.tif').exists():
            continue
        data.append(dict(samples(f'photo{i}', root, cr_linear(cr / f'{i}-default.tif'), cr_linear(cr / f'{i}-S+100.tif'), cr_linear(cr / f'{i}-H-100.tif'), rng), kind='photo'))
    cache.write_bytes(pickle.dumps(data))
    return data


def fit(X, Y, mu, sd, steps=6000, seed=0, lr=2e-3):
    import mlx.core as mx
    import mlx.nn as nn
    import mlx.optimizers as optim

    class MLP(nn.Module):
        def __init__(s, n, h=HIDDEN):
            super().__init__()
            s.l1, s.l2, s.l3 = nn.Linear(n, h), nn.Linear(h, h), nn.Linear(h, 2)

        def __call__(s, x):
            return s.l3(nn.silu(s.l2(nn.silu(s.l1(x)))))

    mx.random.seed(seed)
    m, opt = MLP(X.shape[1]), optim.Adam(lr)
    x, y = mx.array(((X - mu) / sd).astype(np.float32)), mx.array(Y.astype(np.float32))
    g = nn.value_and_grad(m, lambda m, xb, yb: ((m(xb) - yb) ** 2).mean())
    rng = np.random.default_rng(seed)
    for _ in range(steps):
        b = mx.array(rng.integers(0, x.shape[0], 8192))
        _, grads = g(m, x[b], y[b])
        opt.update(m, grads)
        mx.eval(m.parameters(), opt.state)
    predict = lambda Xn: np.array(m(mx.array(((Xn - mu) / sd).astype(np.float32))))
    weights = {k: np.array(v).tolist() for k, v in [('l1.w', m.l1.weight), ('l1.b', m.l1.bias), ('l2.w', m.l2.weight),
                                                     ('l2.b', m.l2.bias), ('l3.w', m.l3.weight), ('l3.b', m.l3.bias)]}
    return predict, weights


def report(predict, ds, label):
    r = np.array([np.sqrt(np.mean((predict(d['X']) - d['Y']) ** 2, 0)) for d in ds])
    print(f'  {label}: S rms median {np.median(r[:, 0]):.3f} mean {r[:, 0].mean():.3f} | '
          f'H rms median {np.median(r[:, 1]):.3f} mean {r[:, 1].mean():.3f} (n={len(ds)})')


def train(args):
    data = load(args.root)
    mos, pho = [d for d in data if d['kind'] == 'mosaic'], [d for d in data if d['kind'] == 'photo']
    stack = lambda ds: (np.concatenate([d['X'] for d in ds]), np.concatenate([d['Y'] for d in ds]))
    Xall = np.concatenate([d['X'] for d in data])
    mu, sd = Xall.mean(0), Xall.std(0) + 1e-6
    if args.cv:
        folds = np.array_split(np.random.default_rng(1).permutation(len(pho)), 4)
        for f in folds:
            held = [pho[i] for i in f]
            predict, _ = fit(*stack(mos + [pho[i] for i in range(len(pho)) if i not in f] * PHOTO_WEIGHT), mu, sd, steps=4000)
            report(predict, held, 'held-out photos')
    members = [fit(*stack(mos + pho * PHOTO_WEIGHT), mu, sd, seed=seed) for seed in range(MEMBERS)]
    predict = lambda Xn: np.mean([p(Xn) for p, _ in members], 0)
    report(predict, pho, 'photos (in-sample, ensemble)')
    report(predict, mos, 'mosaics (in-sample, ensemble)')
    rule = dict(features=FEAT, mu=mu.tolist(), sd=sd.tolist(), members=[w for _, w in members])
    (args.out or args.root / 'global_rule.json').write_text(json.dumps(rule))
    DATA.write_text(rust(rule))
    print(f'-> {DATA.relative_to(ROOT)}')


def rust(g):
    members, hidden, features = len(g['members']), len(g['members'][0]['l1.b']), len(g['mu'])
    def arr(name, v, ty):
        v = np.array(v, dtype=np.float32)
        row = lambda r: '[' + ', '.join(f'{x:.7}' for x in r) + ']'
        if v.ndim == 1:
            return f'pub(super) static {name}: {ty} = {row(v)};\n'
        if v.ndim == 2:
            return f'pub(super) static {name}: {ty} = [\n    ' + ',\n    '.join(row(r) for r in v) + ',\n];\n'
        mats = ',\n    '.join('[\n        ' + ',\n        '.join(row(r) for r in m) + ',\n    ]' for m in v)
        return f'pub(super) static {name}: {ty} = [\n    {mats},\n];\n'
    head = ('//! The global Shadows/Highlights rule: an ensemble of small networks from histogram and\n'
            '//! per-level texture descriptors of a base level to the log2 gain at Shadows +100 and\n'
            '//! Highlights −100, fitted from different seeds to Camera Raw 18.7 on textured flat-patch\n'
            "//! mosaics and linear-profile training photos, from the engine's own measurement maps\n"
            '//! (scripts/corpus/scene-tone-rule.py). Generated; do not edit.\n'
            '#![allow(clippy::excessive_precision, clippy::approx_constant, clippy::unreadable_literal)]\n'
            f'/// Networks in the ensemble.\npub(super) const MEMBERS: usize = {members};\n'
            f'/// Units in each hidden layer.\npub(super) const HIDDEN: usize = {hidden};\n'
            f'/// Inputs the networks read.\npub(super) const FEATURES: usize = {features};\n')
    stack = lambda k: [m[k] for m in g['members']]
    return head + arr('MU', g['mu'], '[f32; FEATURES]') + arr('SD', g['sd'], '[f32; FEATURES]') + \
        arr('W1', stack('l1.w'), '[[[f32; FEATURES]; HIDDEN]; MEMBERS]') + arr('B1', stack('l1.b'), '[[f32; HIDDEN]; MEMBERS]') + \
        arr('W2', stack('l2.w'), '[[[f32; HIDDEN]; HIDDEN]; MEMBERS]') + arr('B2', stack('l2.b'), '[[f32; HIDDEN]; MEMBERS]') + \
        arr('W3', stack('l3.w'), '[[[f32; HIDDEN]; 2]; MEMBERS]') + arr('B3', stack('l3.b'), '[[f32; 2]; MEMBERS]')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='step', required=True)
    s = sub.add_parser('probes')
    s.add_argument('root', type=Path)
    s = sub.add_parser('photos')
    s.add_argument('root', type=Path)
    s.add_argument('raws', type=Path, nargs='+')
    s = sub.add_parser('taps')
    s.add_argument('root', type=Path)
    s.add_argument('rawmakase', type=Path)
    s = sub.add_parser('train')
    s.add_argument('root', type=Path)
    s.add_argument('--cv', action='store_true')
    s.add_argument('--out', type=Path)
    args = p.parse_args()
    args.root = args.root.expanduser().resolve()
    {'probes': probes, 'photos': photos, 'taps': taps, 'train': train}[args.step](args)


if __name__ == '__main__':
    main()
