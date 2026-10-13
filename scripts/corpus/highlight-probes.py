#!/usr/bin/env python3
"""Clipped bright spots on dark backgrounds: the Highlights solarization regression.

  render DIR       writes synthetic DNGs and Camera Raw 16-bit ProPhoto references
  references DIR   records centre/rim measurements for tests/color/scene_probes.rs

Only synthetic content is used. The radial ramp joins a clipped plateau to a dark
background; -5, -7 and -9 stop backgrounds exercise the fitted curve's bright tail
and its clamped extension. Camera Raw itself has a small spatial edge lift, so the
regression compares rim/centre ratios, not a claim of global spatial monotonicity.
"""
import argparse
import json
import plistlib
from pathlib import Path

import numpy as np

import camera_raw
import probe_dng

ROOT = Path(__file__).resolve().parents[2]
KEYS = (-5, -7, -9)
SLIDERS = (0, -50, -100)


def render(folder):
    folder.mkdir(parents=True, exist_ok=True)
    y, x = np.mgrid[:480, :480]
    radius = np.hypot(x - 240, y - 240)
    jobs = []
    for key in KEYS:
        lum = 2. ** (key - key * np.clip((90 - radius) / 50, 0, 1))
        source = folder / f'spot{key}.dng'
        probe_dng.write(source, np.repeat(lum[..., None], 3, axis=2))
        for slider in SLIDERS:
            jobs.append(dict(source=source, settings={'Highlights2012': str(slider)},
                             out=folder / f'acr-spot{key}-H{slider}.tif'))
    failures = camera_raw.render(jobs, work=folder / 'acr-work')
    if failures:
        raise RuntimeError('\n'.join(failures))


def references(folder):
    cases = []
    for key in KEYS:
        for slider in SLIDERS:
            lum = camera_raw.read_linear(folder / f'acr-spot{key}-H{slider}.tif').mean(axis=2)
            centre = float(lum[235:245, 235:245].mean())
            rim = float(lum[240, 240:331].max())
            cases.append(dict(background_ev=key, highlights=slider, centre=centre,
                              rim=rim, ratio=rim / centre))
    path = ROOT / 'tests/corpus/camera-raw/highlight-spots.json'
    with camera_raw.CAMERA_RAW_PLIST.open('rb') as source:
        version = plistlib.load(source)['CFBundleShortVersionString']
    path.write_text(json.dumps(dict(camera_raw=version, process_version='15.4',
                                    generator='scripts/corpus/highlight-probes.py', cases=cases),
                               indent=2) + '\n')
    print(path)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('step', choices=['render', 'references'])
    parser.add_argument('folder', type=Path)
    args = parser.parse_args()
    {'render': render, 'references': references}[args.step](args.folder)
