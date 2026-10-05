#!/usr/bin/env python3
"""Suggest data/cameras.toml rows. Optional: a camera can always be added by hand.

From Adobe DNGs (preferred): convert copies of sample raws with Adobe DNG Converter,
outside the repository, in one call so it starts once:
  "/Applications/Adobe DNG Converter.app/Contents/MacOS/Adobe DNG Converter" -c -d <out> <copies>
then pass the folder; each DNG's BaselineExposure, black and white level are read with
exiftool. When LibRaw's white level for the raw differs from Adobe's, add
log2((LibRaw white - black) / (Adobe white - black)) to the printed value (see
docs/cameras.md); --libraw-white gives LibRaw's white for every file.
  python3 scripts/cameras/fit-baselines.py --dng <folder of DNGs> [--libraw-white 16383]

From Camera Raw renders: run the exposure check in tests/corpus/README.md
(parity-report.py --photos), then fit from its report:
  python3 scripts/cameras/fit-baselines.py --report <report dir>/report.json
Each camera's offset is the midtone exposure difference of the rendered output. The
tone curve steepens midtones, so output moves about 1.17 EV per EV of baseline
(measured on 14 bodies, 0.85-1.34): the new value is the baseline RAWmakase applied
(the camera's row, or the fallback) plus offset / 1.17. Rerun the check after
changing rows; a second pass converges to within ±0.05 EV. Only raw.pixls.us
samples are named by camera. Fujifilm fits hold for DR100 photos only.

Prints TOML rows to paste or merge; it never edits the table itself.
"""
import argparse
import datetime
import json
import statistics
import subprocess
import sys
import tomllib
from pathlib import Path

TABLE = Path(__file__).resolve().parents[2] / 'data' / 'cameras.toml'


def rows():
    return tomllib.loads(TABLE.read_text())['camera']


def applied_baseline(table, make, model):
    """What crate::cameras::baseline_exposure returns."""
    same = lambda a, b: a.lower() == b.lower()
    for c in table:
        if same(c['make'], make) and any(same(m, model) for m in [c['model'], *c.get('aliases', [])]):
            return c['baseline_exposure']
    by_make = [c['baseline_exposure'] for c in table if same(c['make'], make)]
    return statistics.median(by_make or [c['baseline_exposure'] for c in table] or [0.])


def row(make, model, ev, source, sample, how=None):
    lines = ['[[camera]]', f'make = "{make}"', f'model = "{model}"',
             f'baseline_exposure = {round(ev * 20) / 20:g}', f'source = "{source}"',
             f'checked = "{datetime.date.today()}"', f'sample = "{sample}"']
    if how:
        lines.append(f'how = "{how}"')
    return '\n'.join(lines) + '\n'


def from_dngs(folder, libraw_white):
    import math

    for dng in sorted(Path(folder).rglob('*.dng'), key=lambda p: p.name.lower()):
        out = subprocess.run(['exiftool', '-j', '-n', '-Make', '-Model', '-ISO', '-BaselineExposure',
                              '-SubIFD:BlackLevel', '-SubIFD:WhiteLevel', str(dng)],
                             capture_output=True, text=True, check=True)
        tags = json.loads(out.stdout)[0]
        if 'BaselineExposure' not in tags:
            print(f'# {dng.name}: no BaselineExposure', file=sys.stderr)
            continue
        ev = float(tags['BaselineExposure'])
        black = statistics.mean(float(v) for v in str(tags.get('BlackLevel', 0)).split())
        white = float(str(tags.get('WhiteLevel', 0)).split()[0])
        if libraw_white and white > black:
            ev += math.log2((libraw_white - black) / (white - black))
        print(f'# {dng.name}: BaselineExposure {tags["BaselineExposure"]}, black {black:g}, white {white:g}')
        make, model = str(tags.get('Make', '?')), str(tags.get('Model', '?'))
        model = model[len(make):].strip() if model.lower().startswith(make.lower()) else model
        # Check make and model against LibRaw's names (raw-identify -v).
        print(row(make, model, ev, 'adobe-dng',
                  f'1 photo at ISO {tags.get("ISO", "?")}, Adobe DNG Converter'))


MIDTONE_GAIN = 1.17


# raw.pixls.us makes whose LibRaw name differs; models differ through table aliases.
PIXLS_MAKES = {'OM System': 'OM Digital'}


def identity(table, camera):
    """'OM System OM-1' (a report's raw.pixls.us label) -> ('OM Digital', 'OM-1'), the
    table's names when a row matches. None for private photos, named by folder."""
    makes = {c['make'] for c in table} | PIXLS_MAKES.keys()
    make = max((m for m in makes if camera.lower().startswith(m.lower() + ' ')), key=len, default=None)
    if make is None:
        make, _, model = camera.partition(' ')
        return (make, model) if model else None
    model = camera[len(make) + 1:]
    make = PIXLS_MAKES.get(make, make)
    same = lambda a, b: a.lower() == b.lower()
    for c in table:
        if same(c['make'], make) and any(same(m, model) for m in [c['model'], *c.get('aliases', [])]):
            return c['make'], c['model']
    return make, model


def from_report(path):
    table = rows()
    for p in json.loads(Path(path).read_text()).get('photos') or sys.exit('No photos in the report; run it with --photos'):
        found = identity(table, p['camera'])
        if found is None:
            continue
        make, model = found
        print(f'# {p["camera"]}: Camera Raw {p["ev"]:+.2f} EV from RAWmakase')
        print(row(make, model, applied_baseline(table, make, model) + p['ev'] / MIDTONE_GAIN, 'fitted',
                  f'raw.pixls.us, {p["photos"]} photo{"s" * (p["photos"] > 1)}'))


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    g = p.add_mutually_exclusive_group(required=True)
    g.add_argument('--dng', type=Path, help='folder of DNGs written by Adobe DNG Converter')
    g.add_argument('--report', type=Path, help='report.json of scripts/corpus/parity-report.py --photos')
    p.add_argument('--libraw-white', type=float, help="LibRaw's white level for the DNGs' raws")
    args = p.parse_args()
    from_dngs(args.dng, args.libraw_white) if args.dng else from_report(args.report)


if __name__ == '__main__':
    main()
