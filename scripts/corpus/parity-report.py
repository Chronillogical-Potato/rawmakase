#!/usr/bin/env python3
"""Per-control comparison of RAWmakase with Camera Raw on the synthetic charts.

Reads the Camera Raw references in tests/corpus/camera-raw/ and RAWmakase's renders
of the same cases, groups the cases by Develop control and writes a JSON summary
and a self-contained HTML page: per control the mean and p95 CIEDE2000 over every
patch of its cases, how far that is above the default render's own distance, and
the worst cases and patches.

RAWmakase's renders come from the parity test:
  RAWMAKASE_PARITY_DUMP=<dir> cargo test --release --test color camera_raw_parity -- --nocapture
(the dump is written before the test compares with its baseline, so new cases
without a baseline still produce values).

--photos adds the default exposure of real photos per camera, from the private tier:
  RAWMAKASE_CORPUS=... RAWMAKASE_PROFILES=... RAWMAKASE_PHOTO_FILTER=/default \
  RAWMAKASE_PARITY_DUMP=<dir> cargo test --release --test color photos_camera_raw -- --ignored

Requires numpy. Run from the repository root:
  python3 scripts/corpus/parity-report.py <dump dir> --out <dir> [--previous <report.json>] [--photos]
"""
import argparse
import datetime
import html
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / 'tests/corpus'

BANDS = 'red|orange|yellow|green|aqua|blue|purple|magenta'
# (panel, control, case-name pattern); the first match wins.
CONTROLS = [
    ('Default', 'Default render', r'^default$'),
    ('Combinations', 'Slider pairs and looks', r'^(pair-|look-warm-contrast$)'),
    ('White Balance', 'Temperature', r'^temperature-'),
    ('White Balance', 'Tint', r'^tint[+-]'),
    ('Light', 'Exposure', r'^exposure'),
    ('Light', 'Contrast', r'^contrast'),
    ('Light', 'Highlights', r'^highlights'),
    ('Light', 'Shadows', r'^shadows'),
    ('Light', 'Whites', r'^whites'),
    ('Light', 'Blacks', r'^blacks'),
    ('Presence', 'Texture', r'^texture'),
    ('Presence', 'Clarity', r'^clarity'),
    ('Presence', 'Dehaze', r'^dehaze'),
    ('Presence', 'Vibrance', r'^vibrance'),
    ('Presence', 'Saturation', r'^saturation[+-]\d+$'),
    ('Tone Curve', 'Parametric curve', r'^parametric-'),
    ('Tone Curve', 'Refine Saturation', r'refine-saturation'),
    ('Tone Curve', 'Point curves', r'^curve-'),
    ('Color Mixer', 'Hue', rf'^hue-({BANDS})'),
    ('Color Mixer', 'Saturation', rf'^saturation-({BANDS})'),
    ('Color Mixer', 'Luminance', rf'^luminance-({BANDS})'),
    ('Color Mixer', 'Band combinations', r'^mixer-'),
    ('Color Mixer', 'B&W mix', r'^bw-'),
    ('Color Mixer', 'Point Color', r'^pc-'),
    ('Color Grading', 'Hue and saturation', r'^grading-(shadows|midtones|highlights|global)-h'),
    ('Color Grading', 'Luminance', r'^grading-\w+-lum'),
    ('Color Grading', 'Blending and Balance', r'^grading-(blending|balance)'),
    ('Detail', 'Sharpening', r'^sharpening'),
    ('Detail', 'Noise Reduction', r'^noise-'),
    ('Lens Corrections', 'Manual distortion', r'^lens-manual-distortion'),
    ('Lens Corrections', 'Manual vignetting', r'^lens-vignetting'),
    ('Transform', 'Transform sliders', r'^transform-'),
    ('Effects', 'Post-crop vignetting', r'^vignette-'),
    ('Effects', 'Grain', r'^grain'),
    ('Calibration', 'Calibration', r'^calibration-'),
    ('Profile', 'Profile Amount', r'^(amount-|look-)'),
    ('Profile', 'RGB-table looks', r'^rgb-'),
]


def lab(rgb16):
    v = np.asarray(rgb16, float) / 65535.
    lin = np.where(v <= 0.04045, v / 12.92, ((v + 0.055) / 1.055) ** 2.4)
    m = np.array([[0.4124564, 0.3575761, 0.1804375],
                  [0.2126729, 0.7151522, 0.0721750],
                  [0.0193339, 0.1191920, 0.9503041]])
    xyz = lin @ m.T / np.array([0.95047, 1., 1.08883])
    f = np.where(xyz > 216 / 24389, np.cbrt(xyz), (24389 / 27 * xyz + 16) / 116)
    return np.stack([116 * f[..., 1] - 16, 500 * (f[..., 0] - f[..., 1]), 200 * (f[..., 1] - f[..., 2])], -1)


def ciede2000(lab1, lab2):
    l1, a1, b1 = lab1.T
    l2, a2, b2 = lab2.T
    c_bar = (np.hypot(a1, b1) + np.hypot(a2, b2)) / 2
    g = 0.5 * (1 - np.sqrt(c_bar ** 7 / (c_bar ** 7 + 25. ** 7)))
    a1p, a2p = (1 + g) * a1, (1 + g) * a2
    c1p, c2p = np.hypot(a1p, b1), np.hypot(a2p, b2)
    h1p = np.where((a1p == 0) & (b1 == 0), 0, np.degrees(np.arctan2(b1, a1p)) % 360)
    h2p = np.where((a2p == 0) & (b2 == 0), 0, np.degrees(np.arctan2(b2, a2p)) % 360)
    dl, dc = l2 - l1, c2p - c1p
    dh = h2p - h1p
    dh = np.where(c1p * c2p == 0, 0, np.where(dh > 180, dh - 360, np.where(dh < -180, dh + 360, dh)))
    dhh = 2 * np.sqrt(c1p * c2p) * np.sin(np.radians(dh / 2))
    l_bar, cp_bar = (l1 + l2) / 2, (c1p + c2p) / 2
    hsum = h1p + h2p
    hp_bar = np.where(c1p * c2p == 0, hsum,
                      np.where(np.abs(h1p - h2p) <= 180, hsum / 2,
                               np.where(hsum < 360, (hsum + 360) / 2, (hsum - 360) / 2)))
    t = (1 - 0.17 * np.cos(np.radians(hp_bar - 30)) + 0.24 * np.cos(np.radians(2 * hp_bar))
         + 0.32 * np.cos(np.radians(3 * hp_bar + 6)) - 0.20 * np.cos(np.radians(4 * hp_bar - 63)))
    d_theta = 30 * np.exp(-((hp_bar - 275) / 25) ** 2)
    rc = 2 * np.sqrt(cp_bar ** 7 / (cp_bar ** 7 + 25. ** 7))
    sl = 1 + 0.015 * (l_bar - 50) ** 2 / np.sqrt(20 + (l_bar - 50) ** 2)
    sc, sh = 1 + 0.045 * cp_bar, 1 + 0.015 * cp_bar * t
    rt = -np.sin(np.radians(2 * d_theta)) * rc
    return np.sqrt((dl / sl) ** 2 + (dc / sc) ** 2 + (dhh / sh) ** 2 + rt * (dc / sc) * (dhh / sh))


def p95(values):
    """Nearest-rank 95th percentile, as tests/color computes it."""
    v = np.sort(np.asarray(values))
    return float(v[min(len(v) - 1, int(np.ceil(0.95 * len(v))) - 1)])


def control_of(name):
    for panel, control, pattern in CONTROLS:
        if re.search(pattern, name):
            return panel, control
    return 'Other', 'Other'


def git(*args):
    return subprocess.run(['git', *args], cwd=ROOT, capture_output=True, text=True).stdout.strip()


def measure(dump):
    layout = json.loads((CORPUS / 'charts/layout.json').read_text())['patches']
    names = [p['name'] for p in layout]
    groups = [p['group'] for p in layout]
    keep = np.array([g != 'wide' for g in groups])
    ramp = [i for i, g in enumerate(groups) if g == 'ramp']
    hues = [i for i, g in enumerate(groups) if g == 'hue']
    cases, versions = [], {}
    for ref_path in sorted((CORPUS / 'camera-raw').glob('*.json')):
        if ref_path.stem == 'baseline':
            continue
        chart = ref_path.stem
        ref = json.loads(ref_path.read_text())
        versions[chart] = ref['about'].get('camera_raw', '?')
        ours_path = dump / f'{chart}.json'
        if not ours_path.exists():
            sys.exit(f'{ours_path} is missing: run the parity test with RAWMAKASE_PARITY_DUMP')
        ours = json.loads(ours_path.read_text())['cases']
        for name, expected in ref['cases'].items():
            if name not in ours:
                cases.append({'chart': chart, 'case': name, 'error': 'not rendered by RAWmakase'})
                continue
            a, b = lab(expected), lab(ours[name])
            de = ciede2000(a, b)
            kept = de[keep]
            order = [i for i in np.argsort(-de) if keep[i]][:5]
            ramp_mid = [i for i in ramp if -6 <= -8 + 0.5 * ramp.index(i) <= 2]
            chroma_a, chroma_b = np.hypot(a[:, 1], a[:, 2]), np.hypot(b[:, 1], b[:, 2])
            hk = [i for i in hues if chroma_a[i] > 5 and chroma_b[i] > 5]
            dhue = (np.degrees(np.arctan2(b[hk, 2], b[hk, 1]) - np.arctan2(a[hk, 2], a[hk, 1])) + 540) % 360 - 180
            panel, control = control_of(name)
            cases.append({
                'chart': chart, 'case': name, 'panel': panel, 'control': control,
                'mean': float(kept.mean()), 'p95': p95(kept), 'max': float(kept.max()),
                'tone': float(np.mean([b[i, 0] - a[i, 0] for i in ramp_mid])),
                'hue': float(np.sum(np.abs(dhue) * chroma_a[hk]) / max(chroma_a[hk].sum(), 1e-9)),
                'chroma': float(100 * np.mean(chroma_b[hk] / chroma_a[hk] - 1)) if hk else 0.,
                'worst': [{'patch': names[i], 'de': float(de[i]),
                           'camera_raw': expected[i], 'rawmakase': ours[name][i]} for i in order],
                'de': kept,
            })
    defaults = {c['chart']: c for c in cases if c.get('case') == 'default' and 'de' in c}
    controls = {}
    for c in cases:
        if 'de' not in c:
            continue
        c['excess'] = c['mean'] - defaults[c['chart']]['mean'] if c['chart'] in defaults else None
        controls.setdefault((c['panel'], c['control']), []).append(c)
    order = {(p, k): i for i, (p, k, _) in enumerate(CONTROLS)}
    summary = []
    for key, members in sorted(controls.items(), key=lambda kv: order.get(kv[0], 999)):
        pooled = np.concatenate([m['de'] for m in members])
        excess = [m['excess'] for m in members if m['excess'] is not None]
        worst = max(members, key=lambda m: m['mean'])
        summary.append({
            'panel': key[0], 'control': key[1], 'cases': len(members),
            'mean': float(pooled.mean()), 'p95': p95(pooled), 'max': float(pooled.max()),
            'excess': float(np.mean(excess)) if excess else None,
            'worst_case': f"{worst['case']}" + ('' if worst['chart'] == 'synthetic-d65' else f" ({worst['chart']})"),
            'worst_mean': worst['mean'],
        })
    for c in cases:
        c.pop('de', None)
    return summary, cases, versions


def luminance(rgb16):
    v = np.asarray(rgb16, float) / 65535.
    return np.where(v <= 0.04045, v / 12.92, ((v + 0.055) / 1.055) ** 2.4) @ [0.2126, 0.7152, 0.0722]


def camera_of(reference):
    """'pixls/Sony/ILCE-7M4/<file>/default' -> 'Sony ILCE-7M4'; 'own/sony-a7ii/<photo>/default' -> 'sony-a7ii'.
    Photo names stay out of the report."""
    parts = reference.split('/')
    return f'{parts[1]} {parts[2]}' if parts[0] == 'pixls' else parts[1]


def photo_exposure(dump, corpus):
    """Default render of every corpus photo against Camera Raw: the median exposure
    offset in EV over midtone blocks (positive: Camera Raw is brighter) and mean ΔE00,
    per camera. Photos are listed by camera only."""
    per_camera = {}
    for ours_path in sorted(dump.rglob('default.json')):
        reference = ours_path.relative_to(dump).with_suffix('').as_posix()
        ref_path = corpus / 'camera-raw-photos' / f'{reference}.json'
        if not ref_path.exists():
            continue
        ref = json.loads(ref_path.read_text())['blocks']
        ours = json.loads(ours_path.read_text())
        a, b = luminance(ref), luminance(ours)
        mid = (a > 0.01) & (b > 0.01) & (a < 0.85) & (b < 0.85)
        if mid.sum() < 50:
            continue
        ev = float(np.median(np.log2(a[mid] / b[mid])))
        de = float(ciede2000(lab(ref), lab(ours)).mean())
        per_camera.setdefault(camera_of(reference), []).append((ev, de))
    return [{'camera': camera, 'photos': len(v), 'ev': float(np.median([e for e, _ in v])),
             'ev_min': min(e for e, _ in v), 'ev_max': max(e for e, _ in v),
             'mean': float(np.mean([d for _, d in v]))}
            for camera, v in sorted(per_camera.items())]


MIXER = re.compile(rf'^(hue|saturation|luminance)-({BANDS})([+-]\d+)$')


def label(case, chart):
    return case + ('' if chart == 'synthetic-d65' else f' · {chart}')


def render_html(report, previous):
    prev_controls = {(c['panel'], c['control']): c for c in (previous or {}).get('controls', [])}
    prev_cases = {(c['chart'], c['case']): c for c in (previous or {}).get('cases', [])}

    def num(v, digits=2):
        return '–' if v is None else f'{v:.{digits}f}'

    def delta(now, before):
        if before is None or now is None:
            return ''
        d = now - before
        if abs(d) < 0.05:
            return ''
        cls = 'better' if d < 0 else 'worse'
        return f'<span class="delta {cls}">{d:+.2f}</span>'

    def grade(v):
        return 'ok' if v < 1.0 else 'warn' if v < 2.0 else 'bad'

    scale = max(c['p95'] for c in report['controls']) or 1
    rows = []
    last_panel = None
    for c in report['controls']:
        before = prev_controls.get((c['panel'], c['control']))
        panel = c['panel'] if c['panel'] != last_panel else ''
        last_panel = c['panel']
        rows.append(
            f'<tr><th scope="row"><span class="panel">{html.escape(panel)}</span>{html.escape(c["control"])}</th>'
            f'<td class="n">{c["cases"]}</td>'
            f'<td class="n"><span class="pill {grade(c["mean"])}">{num(c["mean"])}</span>{delta(c["mean"], before and before["mean"])}</td>'
            f'<td class="n">{num(c["p95"])}{delta(c["p95"], before and before["p95"])}</td>'
            f'<td class="bar"><span style="width:{100 * c["mean"] / scale:.1f}%" class="m"></span>'
            f'<span style="left:{100 * c["p95"] / scale:.1f}%" class="p"></span></td>'
            f'<td class="n">{num(c["excess"])}</td>'
            f'<td class="case">{html.escape(c["worst_case"])} <span class="dim">{num(c["worst_mean"])}</span></td></tr>')

    measured = [c for c in report['cases'] if 'mean' in c]
    worst_cases = sorted(measured, key=lambda c: -c['mean'])[:25]
    worst_rows = []
    for c in worst_cases:
        before = prev_cases.get((c['chart'], c['case']))
        patches = ', '.join(f'{html.escape(w["patch"])} <span class="dim">{w["de"]:.1f}</span>' for w in c['worst'][:3])
        worst_rows.append(
            f'<tr><th scope="row">{html.escape(label(c["case"], c["chart"]))}<span class="sub">{html.escape(c["control"])}</span></th>'
            f'<td class="n"><span class="pill {grade(c["mean"])}">{c["mean"]:.2f}</span>{delta(c["mean"], before and before.get("mean"))}</td>'
            f'<td class="n">{c["p95"]:.2f}</td><td class="n">{c["max"]:.1f}</td>'
            f'<td class="n">{c["tone"]:+.2f}</td><td class="n">{c["hue"]:.1f}</td><td class="n">{c["chroma"]:+.1f}</td>'
            f'<td class="patches">{patches}</td></tr>')

    all_rows = []
    for c in sorted(measured, key=lambda c: (c['panel'], c['control'], c['chart'], c['case'])):
        w = c['worst'][0]
        all_rows.append(
            f'<tr><td>{html.escape(c["control"] if c["panel"] == c["control"] else c["panel"] + " · " + c["control"])}</td>'
            f'<td>{html.escape(label(c["case"], c["chart"]))}</td>'
            f'<td class="n">{c["mean"]:.2f}</td><td class="n">{c["p95"]:.2f}</td><td class="n">{c["max"]:.1f}</td>'
            f'<td class="n">{c["tone"]:+.2f}</td><td class="n">{c["hue"]:.1f}</td><td class="n">{c["chroma"]:+.1f}</td>'
            f'<td>{html.escape(w["patch"])} <span class="dim">{w["de"]:.1f}</span></td></tr>')
    mixer = {}
    for c in measured:
        m = MIXER.match(c['case'])
        if m and c['chart'] == 'synthetic-d65':
            mixer[(m.group(2), m.group(1), int(m.group(3)))] = c['mean']
    mixer_html = ''
    if mixer:
        steps = sorted({k[2] for k in mixer})
        head = ''.join(f'<th class="n">{kind[0].upper()} {v:+d}</th>' for kind in ('hue', 'saturation', 'luminance')
                       for v in steps)
        body = []
        for band in BANDS.split('|'):
            cells = ''.join(
                f'<td class="n"><span class="pill {grade(mixer[(band, kind, v)])}">{mixer[(band, kind, v)]:.2f}</span></td>'
                if (band, kind, v) in mixer else '<td class="n dim">–</td>'
                for kind in ('hue', 'saturation', 'luminance') for v in steps)
            body.append(f'<tr><th scope="row">{band.capitalize()}</th>{cells}</tr>')
        mixer_html = ('<section><h2>Color mixer by band</h2><p class="note">Mean ΔE00 of each Hue (H), Saturation (S) and '
                      'Luminance (L) slider position on the main chart, one band at a time.</p>'
                      '<div class="frame"><table><thead><tr><th>Band</th>' + head + '</tr></thead><tbody>'
                      + '\n'.join(body) + '</tbody></table></div></section>')
    exposure_html = ''
    if report.get('photos'):
        body = ''.join(
            f'<tr><th scope="row">{html.escape(p["camera"])}</th><td class="n">{p["photos"]}</td>'
            f'<td class="n"><span class="pill {"ok" if abs(p["ev"]) < 0.1 else "warn" if abs(p["ev"]) < 0.25 else "bad"}">{p["ev"]:+.2f}</span></td>'
            f'<td class="n">{p["ev_min"]:+.2f} to {p["ev_max"]:+.2f}</td><td class="n">{p["mean"]:.2f}</td></tr>'
            for p in sorted(report['photos'], key=lambda p: -abs(p['ev'])))
        exposure_html = ('<section><h2>Default exposure on photos</h2><p class="note">Unedited photos with Adobe Standard: '
                         'how much brighter Camera Raw renders them (median over midtone blocks, in EV; positive means '
                         'Camera Raw is brighter) and the mean ΔE00 of the whole frame. CC0 samples from raw.pixls.us '
                         'and private photos, listed by camera only. Large offsets with a wide range point to more than '
                         'exposure (highlight recovery, decoding or crop).</p>'
                         '<div class="frame"><table><thead><tr><th>Camera</th><th class="n">Photos</th><th class="n">Offset EV</th>'
                         '<th class="n">Range</th><th class="n">Mean ΔE00</th></tr></thead><tbody>' + body
                         + '</tbody></table></div></section>')
    errors = [c for c in report['cases'] if 'error' in c]
    error_html = ''
    if errors:
        error_html = ('<section><h2>Not measured</h2><ul>' + ''.join(
            f'<li>{html.escape(label(c["case"], c["chart"]))}: {html.escape(c["error"])}</li>' for c in errors)
            + '</ul></section>')
    total = len(measured)
    d = report['default']
    versions = ', '.join(sorted(set(report['camera_raw'].values())))
    prev_note = (f'Changes are against the report of {html.escape(previous["generated"])} '
                 f'(commit {html.escape(previous["commit"])}); green is closer to Camera Raw.') if previous else ''
    values = {
        'generated': html.escape(report['generated']), 'commit': html.escape(report['commit']),
        'versions': html.escape(versions), 'cases': total, 'controls': len(report['controls']),
        'default_mean': f'{d["mean"]:.2f}', 'default_p95': f'{d["p95"]:.2f}',
        'prev_note': prev_note, 'rows': '\n'.join(rows), 'worst_rows': '\n'.join(worst_rows),
        'all_rows': '\n'.join(all_rows), 'errors': error_html,
        'mixer': mixer_html, 'exposure': exposure_html,
    }
    return re.sub(r'\{\{(\w+)\}\}', lambda m: str(values[m.group(1)]), PAGE)


PAGE = '''<title>Camera Raw Parity</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Sans:wght@400;600&family=IBM+Plex+Mono:wght@400;500&family=Fraunces:opsz,wght@9..144,600&display=swap">
<style>
/* Layout: one reading column; wide tables scroll inside their own frame. */
:root {
  --bg: #f6f7f5; --surface: #ffffff; --fg: #1d2421; --muted: #5d6762; --rule: #dde2de;
  --accent: #2d6a5a; --ok: #2f7d4f; --warn: #a86b12; --bad: #b23a2c;
  --ok-bg: #e3f1e8; --warn-bg: #f8ecd6; --bad-bg: #f7e0dc; --bar: #9cc3b6;
  --display: "Fraunces", Georgia, serif; --body: "IBM Plex Sans", system-ui, sans-serif;
  --mono: "IBM Plex Mono", ui-monospace, Menlo, monospace;
}
@media (prefers-color-scheme: dark) { :root:not([data-theme="light"]) {
  --bg: #141917; --surface: #1b211f; --fg: #e3e8e5; --muted: #98a39d; --rule: #2e3633;
  --accent: #7cc0aa; --ok: #79c795; --warn: #e0a54a; --bad: #ec8a7c;
  --ok-bg: #1f3a2a; --warn-bg: #3d2f17; --bad-bg: #432420; --bar: #3f6d5f; color-scheme: dark; } }
:root[data-theme="dark"] {
  --bg: #141917; --surface: #1b211f; --fg: #e3e8e5; --muted: #98a39d; --rule: #2e3633;
  --accent: #7cc0aa; --ok: #79c795; --warn: #e0a54a; --bad: #ec8a7c;
  --ok-bg: #1f3a2a; --warn-bg: #3d2f17; --bad-bg: #432420; --bar: #3f6d5f; color-scheme: dark; }
body { background: var(--bg); color: var(--fg); font: 15px/1.55 var(--body); }
main { max-width: 1080px; margin: 0 auto; padding-inline: 16px; padding-block: 32px 64px; display: grid; gap: 40px; }
header { display: grid; gap: 10px; max-width: 70ch; }
h1 { font: 600 2.1rem/1.15 var(--display); margin: 0; text-wrap: balance; }
h2 { font: 600 1.3rem/1.3 var(--display); margin: 0 0 6px; text-wrap: balance; }
p { margin: 0; }
.meta { font: 0.8rem var(--mono); color: var(--muted); letter-spacing: 0.02em; }
.lede { color: var(--fg); }
.note { color: var(--muted); font-size: 0.9rem; max-width: 72ch; }
section { display: grid; gap: 10px; min-width: 0; }
.frame { overflow-x: auto; background: var(--surface); border: 1px solid var(--rule); border-radius: 6px; }
table { border-collapse: collapse; width: 100%; font-size: 0.88rem; }
th, td { padding: 7px 12px; text-align: left; border-bottom: 1px solid var(--rule); vertical-align: top; white-space: nowrap; }
thead th { font: 500 0.72rem var(--mono); text-transform: uppercase; letter-spacing: 0.06em; color: var(--muted); background: var(--surface); position: sticky; top: 0; }
tbody tr:last-child > * { border-bottom: 0; }
tbody th { font-weight: 600; }
.panel { display: block; font: 500 0.68rem var(--mono); text-transform: uppercase; letter-spacing: 0.07em; color: var(--accent); }
.sub { display: block; font-weight: 400; color: var(--muted); font-size: 0.8rem; }
.n { text-align: right; font-family: var(--mono); font-variant-numeric: tabular-nums; }
.dim { color: var(--muted); font-family: var(--mono); font-size: 0.8rem; }
.case, .patches { white-space: normal; min-width: 14ch; }
.pill { display: inline-block; min-width: 3.4em; padding: 1px 6px; border-radius: 3px; text-align: right; font-weight: 500; }
.pill.ok { background: var(--ok-bg); color: var(--ok); }
.pill.warn { background: var(--warn-bg); color: var(--warn); }
.pill.bad { background: var(--bad-bg); color: var(--bad); }
.delta { display: block; font-size: 0.75rem; }
.delta.better { color: var(--ok); } .delta.worse { color: var(--bad); }
td.bar { position: relative; width: 22%; min-width: 120px; }
td.bar .m { position: absolute; left: 12px; top: 50%; height: 8px; margin-top: -4px; background: var(--bar); border-radius: 2px; max-width: calc(100% - 24px); }
td.bar .p { position: absolute; top: 50%; width: 2px; height: 14px; margin-top: -7px; background: var(--fg); margin-left: 12px; }
.legend { display: flex; flex-wrap: wrap; gap: 8px 18px; font-size: 0.82rem; color: var(--muted); }
details summary { cursor: pointer; font-weight: 600; }
details summary:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
ul { margin: 0; padding-left: 1.2em; }
</style>
<main>
<header>
  <p class="meta">Generated {{generated}} · commit {{commit}} · Camera Raw {{versions}}</p>
  <h1>RAWmakase against Camera Raw</h1>
  <p class="lede">Every Develop control the synthetic charts can measure, {{cases}} renders in {{controls}} groups. Each chart patch is averaged in both renders and compared with CIEDE2000; around 1 is hard to see side by side, above 2 is visible.</p>
  <p class="note">The default render is already {{default_mean}} mean (p95 {{default_p95}}) from Camera Raw on the main chart, so “above default” shows what a control adds on top of that. Charts are synthetic DNGs with their own matrices, so no Adobe profile is involved. Flat patches say little about local controls (Clarity, Texture, Dehaze, sharpening, noise reduction, grain) and geometric ones measure where patches land. {{prev_note}}</p>
</header>
<section>
  <h2>Per control</h2>
  <div class="legend"><span>Bar: mean ΔE00, tick: p95, on one scale</span><span>Pill: under 1 · 1 to 2 · 2 and above</span></div>
  <div class="frame"><table>
    <thead><tr><th>Control</th><th class="n">Cases</th><th class="n">Mean</th><th class="n">p95</th><th></th><th class="n">Above default</th><th>Worst case</th></tr></thead>
    <tbody>
{{rows}}
    </tbody></table></div>
</section>
{{exposure}}
{{mixer}}
<section>
  <h2>Worst cases</h2>
  <p class="note">The 25 cases furthest from Camera Raw. Tone is the mean L* offset on the gray ramp (−6 to +2 EV), hue the chroma-weighted hue error in degrees, chroma the mean relative chroma difference on the hue grid; positive means RAWmakase is lighter or more saturated.</p>
  <div class="frame"><table>
    <thead><tr><th>Case</th><th class="n">Mean</th><th class="n">p95</th><th class="n">Max</th><th class="n">Tone L*</th><th class="n">Hue °</th><th class="n">Chroma %</th><th>Worst patches</th></tr></thead>
    <tbody>
{{worst_rows}}
    </tbody></table></div>
</section>
{{errors}}
<section>
  <details>
    <summary>Every case</summary>
    <div class="frame" style="margin-top:10px"><table>
      <thead><tr><th>Control</th><th>Case</th><th class="n">Mean</th><th class="n">p95</th><th class="n">Max</th><th class="n">Tone L*</th><th class="n">Hue °</th><th class="n">Chroma %</th><th>Worst patch</th></tr></thead>
      <tbody>
{{all_rows}}
      </tbody></table></div>
  </details>
</section>
</main>
'''


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('dump', type=Path, help='RAWMAKASE_PARITY_DUMP folder of RAWmakase renders')
    p.add_argument('--out', type=Path, required=True, help='folder for report.json and report.html')
    p.add_argument('--previous', type=Path, help='an earlier report.json to show changes against')
    p.add_argument('--photos', action='store_true',
                   help='also compare default photo renders (<dump>/photos from photos_camera_raw_parity) '
                        'with $RAWMAKASE_CORPUS/camera-raw-photos')
    args = p.parse_args()

    controls, cases, versions = measure(args.dump)
    default = next(c for c in cases if c.get('case') == 'default' and c['chart'] == 'synthetic-d65')
    report = {
        'generated': datetime.datetime.now(datetime.timezone.utc).strftime('%Y-%m-%d %H:%M UTC'),
        'commit': git('rev-parse', '--short', 'HEAD') + ('+' if git('status', '--porcelain', '--untracked-files=no') else ''),
        'camera_raw': versions,
        'default': {'mean': default['mean'], 'p95': default['p95']},
        'controls': controls,
        'cases': cases,
    }
    if args.photos:
        corpus = os.environ.get('RAWMAKASE_CORPUS')
        if not corpus:
            sys.exit('Set RAWMAKASE_CORPUS for --photos')
        report['photos'] = photo_exposure(args.dump / 'photos', Path(corpus))
    previous = json.loads(args.previous.read_text()) if args.previous else None
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / 'report.json').write_text(json.dumps(report, indent=1) + '\n')
    (args.out / 'report.html').write_text(render_html(report, previous))
    width = max(len(c['control']) for c in controls)
    print(f'{"panel":16} {"control":{width}} {"cases":>5} {"mean":>6} {"p95":>6} {"excess":>7}  worst')
    for c in controls:
        excess = '' if c['excess'] is None else f'{c["excess"]:+7.2f}'
        print(f'{c["panel"]:16} {c["control"]:{width}} {c["cases"]:5} {c["mean"]:6.2f} {c["p95"]:6.2f} {excess:>7}  '
              f'{c["worst_case"]} {c["worst_mean"]:.2f}')


if __name__ == '__main__':
    main()
