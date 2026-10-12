#!/usr/bin/env python3
"""Camera Raw parity over the classified test dataset's frozen references
(issue #420, level 3): every reference set of a split, rendered and scored with
one build, and judged against another build's scores.

  run      render and score every set of a split (validation or holdout) with a
           build; each set's renders are deleted once scored, scores kept; an
           interrupted run resumes where it stopped
  accept   gate.py accept on every set: a candidate's scores against a baseline's
  report   how far a run is from Camera Raw: by control, stratum, source, the
           lightness bias per tone band, and what could not be scored

A run is a folder (`--out`) holding `scores/<family>/<set>.json`, a log per set and
`run.json` naming the build. The dataset is $RAWMAKASE_DATASET (or --dataset);
gate.py does the rendering and scoring, so its rules apply (scripts/corpus/gate.py,
tests/corpus/README.md). Judge changes on validation; holdout is scored once, at
the decision, with --exclude for photos already looked at.

  python3 scripts/corpus/dataset-gate.py run --rawmakase target/release/rawmakase \\
      --data-dir <Adobe profile library> --out $RAWMAKASE_DATASET/baselines/gate/<name>
  python3 scripts/corpus/dataset-gate.py accept --candidate <run> --baseline <run>
  python3 scripts/corpus/dataset-gate.py report <run>

Exit status: 0 pass, 1 a set fails (accept) or a set could not be scored (run),
2 the run could not be made.
"""
import argparse
import concurrent.futures
import contextlib
import datetime
import io
import json
import os
import shutil
import statistics
import subprocess
import sys
import types
from collections import Counter, defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.dont_write_bytecode = True
sys.path.insert(0, str(HERE))
import gate  # noqa: E402

ROOT = HERE.parents[1]


class Setup(Exception):
    """The run cannot be made; never a pass."""


def dataset(args):
    d = Path(args.dataset or os.environ.get('RAWMAKASE_DATASET') or '')
    if not (d / 'references').is_dir():
        raise Setup(f'No dataset references at {d}: is the SSD mounted? Set RAWMAKASE_DATASET or pass --dataset.')
    return d.resolve()


def sets(d, split):
    found = sorted(d.glob(f'references/camera-raw-*/*-{split}'))
    if not found:
        raise Setup(f'No {split} sets under {d / "references"}')
    return found


def run_set(args, d, refs, out):
    """One set: render, score, delete the renders. Returns (set, ok, seconds). gate.py
    runs from the dataset folder: the references name their RAWs relative to it."""
    family, name = refs.parent.name, refs.name
    score = out / 'scores' / family / f'{name}.json'
    if score.exists():
        # Kept from an interrupted run only when this build made it, for these
        # references, with this profile library.
        made_by = json.loads(score.read_text()).get('rendered_by') or {}
        expected = dict(references=gate.digest(refs / 'references.json'), rawmakase=gate.digest(args.rawmakase),
                        profiles=args.profiles, policy=gate.RENDER_POLICY)
        if any(made_by.get(k) != v for k, v in expected.items()):
            return f'{family}/{name} (scored by another build, library or references: use a new --out)', False, 0
        return f'{family}/{name}', True, 0
    renders = out / 'renders' / family / name
    logs = out / 'logs' / family
    logs.mkdir(parents=True, exist_ok=True)
    score.parent.mkdir(parents=True, exist_ok=True)
    start = datetime.datetime.now()
    py = sys.executable
    with (logs / f'{name}.render.log').open('w') as log:
        rendered = subprocess.run([py, str(HERE / 'gate.py'), 'render', '--refs', str(refs), '--rawmakase',
                                   str(args.rawmakase), '--data-dir', str(args.data_dir), '--out', str(renders)],
                                  stdout=log, stderr=log, cwd=d).returncode == 0
    # A render gate.py refused (another build's renders in the folder, inputs that
    # changed) is never scored; its folder stays for a look.
    if not rendered:
        return f'{family}/{name} (render failed)', False, (datetime.datetime.now() - start).seconds
    with (logs / f'{name}.score.log').open('w') as log:
        ok = subprocess.run([py, str(HERE / 'gate.py'), 'score', '--refs', str(refs), '--renders', str(renders),
                             '--out', str(score)], stdout=log, stderr=log, cwd=d).returncode == 0
    if ok:
        shutil.rmtree(renders, ignore_errors=True)
    return f'{family}/{name}', ok, (datetime.datetime.now() - start).seconds


def run(args):
    d = dataset(args)
    args.rawmakase = args.rawmakase.resolve(strict=True)
    if not args.data_dir:
        raise Setup('Pass --data-dir: the library with the Adobe profiles (see tests/corpus/README.md)')
    out = args.out.resolve()
    out.mkdir(parents=True, exist_ok=True)
    args.profiles = gate.library_digest(Path(args.data_dir) / 'camera-profiles')
    built = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=args.rawmakase.parent, capture_output=True,
                           text=True).stdout.strip()
    identity = dict(rawmakase_sha256=gate.digest(args.rawmakase), profiles=args.profiles)
    # One run folder is one build with one profile library; a split may be added.
    previous = json.loads((out / 'run.json').read_text()) if (out / 'run.json').exists() else None
    if previous and any(previous.get(k) != v for k, v in identity.items()):
        raise Setup(f'{out} holds a run of another build or profile library: use a new --out')
    splits = sorted(set((previous or {}).get('splits', [])) | {args.split})
    (out / 'run.json').write_text(json.dumps(dict(
        identity, rawmakase=str(args.rawmakase), commit=built or None, data_dir=str(args.data_dir), splits=splits,
        started=(previous or {}).get('started') or datetime.datetime.now().isoformat(timespec='seconds')),
        indent=1))
    todo = sets(d, args.split)
    failed = []
    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        for name, ok, seconds in pool.map(lambda r: run_set(args, d, r, out), todo):
            print(f'{datetime.datetime.now():%H:%M} {name}: {"scored" if ok else "FAILED"} ({seconds} s)', flush=True)
            if not ok:
                failed.append(name)
    print(f'{len(todo) - len(failed)} of {len(todo)} sets scored in {out / "scores"}')
    for name in failed:
        print(f'  not scored: {name} (see its logs)')
    return 1 if failed else 0


def accept(args):
    """Every set of the split, which both runs must have scored."""
    d = dataset(args)
    baseline, candidate = args.baseline.resolve(), args.candidate.resolve()
    expected = [f'{r.parent.name}/{r.name}' for r in sets(d, args.split)]
    counts, failing = Counter(), []
    for name in expected:
        b, c = baseline / 'scores' / f'{name}.json', candidate / 'scores' / f'{name}.json'
        if not b.exists() or not c.exists():
            counts['missing'] += 1
            failing.append((name, 'not scored by the ' + ('baseline' if not b.exists() else 'candidate')))
            continue
        results, text = [], io.StringIO()
        with contextlib.redirect_stdout(text):
            status = gate.accept(types.SimpleNamespace(candidate=c, baseline=b, exclude=args.exclude,
                                                       rule=args.rule, results=results))
        if args.verbose:
            print(f'== {name}\n{text.getvalue()}')
        if status and not results:
            # Refused before judging (other references, profiles or policy).
            failing.append((name, text.getvalue().strip().splitlines()[-1]))
            continue
        for r in results:
            # A pair fails on any reason but an inconclusive one; one that is both
            # inconclusive on a rule and failing another still fails.
            hard = [why for why in r['reasons'] if not why.startswith('inconclusive')]
            counts['pairs'] += 1
            counts['fail' if hard else 'inconclusive' if r['reasons'] else 'pass'] += 1
            if hard:
                failing.append((name, f'{r["name"]} {r["stratum"]}: ' + '; '.join(hard)))
    print(f'{args.split}: {len(expected)} sets, {counts["pairs"]} setting × stratum pairs: {counts["pass"]} pass, '
          f'{counts["fail"]} fail, {counts["inconclusive"]} inconclusive (fewer than three photos)'
          + (f'; {counts["missing"]} sets not scored by both runs' if counts['missing'] else ''))
    for name, why in failing:
        print(f'  {name}: {why}')
    return 1 if failing else 0


def report(args):
    """How far a run is from Camera Raw (median ΔE00 per photo, then medians)."""
    out = args.run.resolve()
    rows = []
    for f in sorted(out.glob('scores/*/*.json')):
        d = json.loads(f.read_text())
        source = f.parent.name.replace('camera-raw-18.7', 'raw.pixls.us').replace('raw.pixls.us-', '')
        rows += [dict(r, source=source, control=f.stem.rsplit('-', 1)[0], split=f.stem.rsplit('-', 1)[1])
                 for r in d['photos']]
    if not rows:
        raise Setup(f'No scores in {out / "scores"}')
    valid = [r for r in rows if r.get('valid', 0) >= 0.5]
    med = lambda v: statistics.median(v) if v else float('nan')
    p90 = lambda v: sorted(v)[int(0.9 * (len(v) - 1))] if v else float('nan')
    kind = lambda r: 'default' if r['name'].endswith('default') else 'edited'
    v = [r['median_de00'] for r in valid]
    print(f'{len(valid)} of {len(rows)} cases scored; median ΔE00 {med(v):.2f}, 90% within {p90(v):.2f}\n')
    for key, label in (('control', 'set'), ('stratum', 'stratum'), ('source', 'source')):
        groups = defaultdict(list)
        for r in valid:
            groups[(r[key], kind(r))].append(r['median_de00'])
        print(f'{label:24s} {"kind":8s} {"n":>5s} {"median":>7s} {"p90":>6s}')
        for (k, kd), g in sorted(groups.items()):
            print(f'{k:24s} {kd:8s} {len(g):5d} {med(g):7.2f} {p90(g):6.2f}')
        print()
    worst = defaultdict(list)
    for r in valid:
        if kind(r) == 'edited':
            worst[r['name']].append(r['median_de00'])
    print('furthest settings:', ', '.join(f'{n} {med(g):.2f}' for n, g in
                                           sorted(worst.items(), key=lambda x: -med(x[1]))[:12]))
    bands = defaultdict(lambda: defaultdict(list))
    for f in sorted(out.glob('scores/*/*.json')):
        d = json.loads(f.read_text())
        for key, b in d.get('bands', {}).items():
            k = 'default' if key.split('|')[0].endswith('default') else 'edited'
            for band, value in b.items():
                bands[(f.stem.rsplit('-', 1)[0], k)][band].append(value)
    order = [f'{lo}-{lo + 10}' for lo in range(0, 100, 10)]
    print('\nΔL* by tone band (render − Camera Raw; + is lighter), median over settings and strata:')
    print(f'{"set":24s} {"kind":8s} ' + ' '.join(f'{b:>6s}' for b in order))
    for (c, k), b in sorted(bands.items()):
        print(f'{c:24s} {k:8s} ' + ' '.join(f'{med(b[x]):+6.1f}' if b.get(x) else '     .' for x in order))
    invalid = Counter((r['source'], (r.get('error') or '').split(':')[0]) for r in rows if r.get('valid', 0) < 0.5)
    if invalid:
        print('\nnot scored:', ', '.join(f'{n} × {why} ({src})' for (src, why), n in invalid.most_common()))
    return 0


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--dataset', help='the dataset checkout (default: $RAWMAKASE_DATASET)')
    sub = p.add_subparsers(dest='command', required=True)
    s = sub.add_parser('run')
    s.add_argument('--rawmakase', type=Path, required=True, help='a release build')
    s.add_argument('--data-dir', type=Path, help='the profile library with the Adobe profiles')
    s.add_argument('--out', type=Path, required=True)
    s.add_argument('--split', choices=['validation', 'holdout', 'train'], default='validation')
    s.add_argument('--jobs', type=int, default=2, help='sets at a time')
    s = sub.add_parser('accept')
    s.add_argument('--candidate', type=Path, required=True, help='a run folder')
    s.add_argument('--baseline', type=Path, required=True, help='a run folder')
    s.add_argument('--split', choices=['validation', 'holdout', 'train'], default='validation')
    s.add_argument('--rule', choices=['regression', 'switch'], default='regression')
    s.add_argument('--exclude', type=Path, help='photos to leave out (gate.py accept --exclude)')
    s.add_argument('--verbose', action='store_true', help='every set\'s table')
    s = sub.add_parser('report')
    s.add_argument('run', type=Path)
    args = p.parse_args()
    try:
        return {'run': run, 'accept': accept, 'report': report}[args.command](args)
    except Setup as e:
        print(f'error: {e}', file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())
