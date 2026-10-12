#!/usr/bin/env python3
"""Render snapshots of the classified test dataset (issue #420, level 2).

Runs `photos_match_accepted_renders` (tests/color/private.rs) on the dataset's
measured photos and groups what changed by the dataset's classifications, so a
change reads as "X-Trans, low-key scenes" rather than a list of photos.

  run      run the test (release build) with RAWMAKASE_REPORT, then report
  report   group an existing RAWMAKASE_REPORT file

Before the first run, link the photos into a corpus of their own and record its
accepted renders from main (see tests/corpus/README.md, Render snapshots):

  export RAWMAKASE_DATASET=… RAWMAKASE_CORPUS=$RAWMAKASE_DATASET/baselines/corpus
  (cd $RAWMAKASE_DATASET && python3 -I scripts/dataset.py manifest \\
      --where "raw_ok AND rawmakase_ok" --out $RAWMAKASE_CORPUS/manifest.json --link)
  python3 scripts/corpus/dataset-renders.py run --bless     # on main only

Photos are named by SHA-256; --public names only raw.pixls.us CC0 photos and
counts the rest. Exit status: 0 nothing changed, 1 changes or failures, 2 the
run could not be made.
"""
import argparse
import hashlib
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
from collections import Counter, defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PUBLIC_LICENSE = 'CC0-1.0'
# Classifications a change is grouped by: photos view columns, and the CFA.
DIMENSIONS = ['make', 'cfa', 'key', 'scene', 'light', 'setting', 'contrast', 'skin', 'sky']
TEST = 'private::photos_match_accepted_renders'
# The formats the test reads (RAW_EXTENSIONS in tests/color/private.rs).
READ = {'arw', 'raf', 'nef', 'nrw', 'cr2', 'cr3', 'dng', 'rw2', 'orf', 'ori', 'pef', 'rwl', 'fff', '3fr'}


class Setup(Exception):
    """The run cannot be made; never a pass."""


def env_path(name, value):
    value = value or os.environ.get(name)
    if not value:
        raise Setup(f'Set {name} or pass it as an option')
    return Path(value)


def run_test(corpus, report, bless, source_edge, public):
    env = {**os.environ, 'RAWMAKASE_CORPUS': str(corpus), 'RAWMAKASE_REPORT': str(report),
           'RAWMAKASE_PHOTO_SOURCE_EDGE': str(source_edge)}
    # Only --bless records: an exported RAWMAKASE_BLESS would rewrite the baseline.
    env.pop('RAWMAKASE_BLESS', None)
    if bless:
        env['RAWMAKASE_BLESS'] = bless
    # The test's own output names photos by path; with --public it goes to a log
    # beside the corpus, which stays on this machine.
    log = (corpus / 'last-run.log').open('w') if public else None
    # The test fails on any change; the report says which.
    subprocess.run(['cargo', 'test', '--release', '--locked', '--test', 'color', '--', '--ignored', '--exact',
                    TEST, '--nocapture'], cwd=ROOT, env=env, stdout=log or sys.stderr, stderr=log)
    if log:
        log.close()
        print(f'test output: {corpus / "last-run.log"}', file=sys.stderr)
    if not report.is_file():
        raise Setup('The test wrote no report: did it build and find the corpus?')


def classify(dataset, corpus, outcomes):
    """Each outcome with its sample's classifications, found by SHA-256: the
    corpus's links point into the dataset; other files are hashed."""
    db_path = dataset / 'dataset.sqlite'
    if not db_path.is_file():
        raise Setup(f'No dataset at {dataset} (no dataset.sqlite): is the SSD mounted?')
    db = sqlite3.connect(f'file:{db_path}?mode=ro', uri=True)
    db.row_factory = sqlite3.Row
    folders = {r['source']: (dataset / r['folder']).resolve() for r in db.execute('SELECT source, folder FROM sources')}
    by_path = {}
    for r in db.execute('SELECT sha256, source, path FROM samples WHERE path IS NOT NULL'):
        if r['source'] in folders:
            by_path[folders[r['source']] / r['path']] = r['sha256']
    columns = ', '.join(f'p.{d}' for d in DIMENSIONS if d != 'cfa')
    rows = {r['sha256']: dict(r) for r in db.execute(
        f'SELECT p.sha256, p.source, p.model, p.license, st.cfa, {columns} '
        'FROM photos p LEFT JOIN stats st ON st.sha256 = p.sha256')}
    out, unknown = [], []
    for o in outcomes:
        file = (corpus / 'raws' / o['raw']).resolve()
        sha = by_path.get(file)
        if sha is None and file.is_file():
            sha = hashlib.sha256(file.read_bytes()).hexdigest()
        if sha not in rows:
            unknown.append(o)
            continue
        out.append({**o, **rows[sha]})
    return out, unknown


def failed(o):
    return o['status'] not in ('unchanged', 'blessed', 'kept')


def missing(dataset, corpus, photos):
    """Photos of the corpus's manifest (`dataset.py manifest --out
    $RAWMAKASE_CORPUS/manifest.json`) in formats the test reads that the run did
    not see: a link that is missing drops its photo without failing the test."""
    manifest = corpus / 'manifest.json'
    if not manifest.is_file():
        raise Setup(f'No {manifest}: write the selection there with dataset.py manifest --out')
    db = sqlite3.connect(f'file:{dataset / "dataset.sqlite"}?mode=ro', uri=True)
    seen = {p['sha256'] for p in photos}
    out = []
    for f in json.loads(manifest.read_text())['files']:
        row = db.execute('SELECT ext, source, license FROM samples WHERE sha256 = ?', (f['sha256'],)).fetchone()
        # A hash the database no longer knows is missing too, never skipped.
        if row is None:
            out.append(dict(sha256=f['sha256'], source=None, license=None))
        elif (row[0] or '').lower() in READ and f['sha256'] not in seen:
            out.append(dict(sha256=f['sha256'], source=row[1], license=row[2]))
    return out


def report(photos, unknown, public, out_path, absent=()):
    shown = (lambda p: not public or (p['source'] == 'pixls' and p['license'] == PUBLIC_LICENSE))
    statuses = Counter(p['status'] for p in photos)
    print(f'{len(photos)} dataset photos: ' + ', '.join(f'{n} {s}' for s, n in statuses.most_common()))
    if unknown:
        print(f'{len(unknown)} corpus photos are not in the dataset and were left out')
    if absent:
        named = [a['sha256'][:12] for a in absent if not public or (a['source'] == 'pixls' and a['license'] == PUBLIC_LICENSE)]
        print(f'{len(absent)} photos of the manifest were not run (not linked in the corpus?)'
              + (': ' + ', '.join(named) if named else ''))
    out = {'photos': len(photos), 'statuses': dict(statuses), 'not_in_dataset': len(unknown),
           'not_run': len(absent), 'groups': {},
           'changed': []}
    bad = [p for p in photos if failed(p)]
    for dim in DIMENSIONS:
        total, hit = Counter(), Counter()
        for p in photos:
            total[str(p[dim])] += 1
        for p in bad:
            hit[str(p[dim])] += 1
        if not hit:
            continue
        # Values where changes concentrate first.
        ranked = sorted(hit, key=lambda v: (-hit[v] / total[v], -hit[v], v))
        print(f'\nby {dim}: ' + ', '.join(f'{v} {hit[v]}/{total[v]}' for v in ranked))
        out['groups'][dim] = {v: [hit[v], total[v]] for v in ranked}
    hidden = 0
    for p in sorted(bad, key=lambda p: (str(p['make']), str(p['model']), p['sha256'])):
        if not shown(p):
            hidden += 1
            continue
        worst = max(p.get('cases', {}).items(), key=lambda c: c[1][0], default=None)
        detail = p.get('detail') or (f'{len(p["cases"])} cases, worst {worst[0]}: mean ΔE00 {worst[1][0]:.2f}, '
                                     f'p95 {worst[1][1]:.2f}' if worst else '')
        print(f'  {p["sha256"][:12]} {p["make"]} {p["model"]} [{p["cfa"]}, {p["key"]}, {p["scene"]}, '
              f'{p["light"]}]: {p["status"]}' + (f' ({detail})' if detail else ''))
        out['changed'].append({k: p.get(k) for k in ['sha256', 'make', 'model', 'status', 'detail', 'cases',
                                                     *DIMENSIONS]})
    if hidden:
        print(f'  and {hidden} photos of private sources')
        out['private_changed'] = hidden
    if out_path:
        out_path.write_text(json.dumps(out, indent=1) + '\n')
    return 1 if bad or unknown or absent else 0


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('command', choices=['run', 'report'])
    p.add_argument('file', nargs='?', type=Path, help='report: the RAWMAKASE_REPORT file')
    p.add_argument('--dataset', help='the dataset checkout (default: $RAWMAKASE_DATASET)')
    p.add_argument('--corpus', help='the corpus the test reads (default: $RAWMAKASE_CORPUS)')
    p.add_argument('--bless', nargs='?', const='1', choices=['1', 'new'],
                   help='run: record accepted renders (on main only); "new": only photos without one')
    # The 48-block comparison sees nothing a 2,400-pixel source loses, and the run
    # takes hours instead of minutes at full size.
    p.add_argument('--source-edge', type=int, default=2400,
                   help='run: render from the photo reduced to this long edge; 0 for full size (default 2400)')
    p.add_argument('--public', action='store_true',
                   help='name only raw.pixls.us CC0 photos; count the others (for shared reports)')
    p.add_argument('--out', type=Path, help='also write the grouped report as JSON')
    args = p.parse_args()
    try:
        dataset = env_path('RAWMAKASE_DATASET', args.dataset)
        corpus = env_path('RAWMAKASE_CORPUS', args.corpus)
        if not (corpus / 'raws').is_dir():
            raise Setup(f'No raws/ in {corpus}: link the photos with dataset.py manifest --link')
        if args.command == 'run':
            report_file = Path(tempfile.mkstemp(suffix='.json')[1])
            report_file.unlink()
            run_test(corpus, report_file, args.bless, args.source_edge, args.public)
        elif args.file is None:
            raise Setup('report needs the RAWMAKASE_REPORT file')
        else:
            report_file = args.file
        outcomes = json.loads(report_file.read_text())
        if not outcomes:
            raise Setup('The report lists no photos')
        photos, unknown = classify(dataset, corpus, outcomes)
        return report(photos, unknown, args.public, args.out, missing(dataset, corpus, photos))
    except Setup as e:
        print(f'error: {e}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
