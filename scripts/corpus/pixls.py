#!/usr/bin/env python3
"""Sample RAW files from raw.pixls.us for cameras we don't own.

The files are CC0 but large, so they are never committed. The repository keeps
tests/corpus/pixls.json (URL, SHA-256, size and camera of each file); this script
downloads them into $RAWMAKASE_CORPUS/raws/pixls/ and checks their hashes.

  manifest   rebuild tests/corpus/pixls.json from raw.pixls.us for MODELS below
             (recent Sony, Canon, Nikon, Leica and Fujifilm bodies; smallest CC0
             file per model)
  download   fetch missing files; refuses to exceed the corpus budget (--budget-gb)
  cameras    write tests/corpus/cameras.json with each camera's LibRaw color matrix,
             read from the downloaded files and any other RAWs in the corpus; the
             color tests make one synthetic chart per camera from it

Run from the repository root; `cameras` needs a built target/release/rawmakase
(or --rawmakase).
"""
import argparse
import hashlib
import html
import json
import os
import re
import subprocess
import sys
import tempfile
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / 'tests/corpus/pixls.json'
CAMERAS = ROOT / 'tests/corpus/cameras.json'
REPOSITORY = 'https://raw.pixls.us/json/getrepository.php'
# Cameras whose synthetic chart LibRaw does not decode at full size, which would
# move the patches.
NO_CHART = {'leica-sl2': 'LibRaw trims 19 rows from DNGs made by this model'}
RAW_EXTENSIONS = {'.arw', '.raf', '.nef', '.nrw', '.cr2', '.cr3', '.dng', '.rw2', '.orf'}

# Recent bodies (Piotr, 2026-09-26: Sony, Canon, Nikon, Leica, Fujifilm), and popular
# bodies of the last decade from those and other makers (Piotr, 2026-10-05), for the
# per-camera default exposure in scripts/corpus/parity-report.py.
MODELS = {
    'Sony': ['ILCE-7M5', 'ILCE-7M4', 'ILCE-1M2', 'ILCE-7CM2', 'ILCE-6700', 'ILCE-9M3', 'ILCE-7RM5',
             'ILCE-7M3', 'ILCE-7RM4', 'ILCE-7RM3', 'ILCE-7C', 'ILCE-7SM3', 'ILCE-1', 'ILCE-6400',
             'ILCE-6600', 'ILCE-6000', 'ZV-E10', 'DSC-RX100M7'],
    'Canon': ['EOS R6 Mark III', 'EOS R5 Mark II', 'EOS R8', 'EOS R7', 'EOS R50', 'EOS R100', 'EOS R3',
              'EOS R6 Mark II', 'EOS R6', 'EOS R5', 'EOS R', 'EOS RP', 'EOS R10', 'EOS 5D Mark IV',
              'EOS 6D Mark II', 'EOS 90D', 'EOS M50'],
    'Nikon': ['Z 8', 'Z f', 'Z6_3', 'Z5_2', 'Z50_2', 'Z 30', 'Z 9', 'Z 6_2', 'Z 7_2', 'Z 6', 'Z 7',
              'Z 5', 'Z 50', 'Z fc', 'D850', 'D780', 'D750', 'D7500'],
    'Fujifilm': ['X100VI', 'X-T5', 'X-H2S', 'X-S20', 'X-T50', 'X-E5', 'X-H2', 'X-T4', 'X-T3', 'X100V',
                 'X-T30 II', 'X-S10', 'X-Pro3', 'X-E4'],
    'Leica': ['M10-R', 'SL2', 'Q2'],
    'Panasonic': ['DC-S5M2', 'DC-S5', 'DC-S1', 'DC-GH6', 'DC-GH5', 'DC-G9M2', 'DC-G9'],
    'Olympus': ['E-M1MarkIII', 'E-M1MarkII'],
    'Ricoh': ['GR III'],
}


def corpus():
    value = os.environ.get('RAWMAKASE_CORPUS')
    if not value:
        sys.exit('Set RAWMAKASE_CORPUS (e.g. "$HOME/RAWmakase Corpus")')
    return Path(value)


def build_manifest(_args):
    with urllib.request.urlopen(REPOSITORY, timeout=120) as r:
        rows = json.load(r)['data']
    chosen = []
    for make, models in MODELS.items():
        for model in models:
            candidates = []
            for row in rows:
                if row[0] != make or row[1] != model or 'zero' not in row[5]:
                    continue
                m = re.search(r"href='([^']+)'.*?sha256 Checksum'>([0-9a-f]{64})</span>&nbsp;\(([\d.]+)MB\)", row[7])
                if m:
                    # Prefer full-precision files (not Nikon HE or lossy modes) in the
                    # camera's usual 3:2 frame, then the smallest file.
                    reduced = '8bit' in row[2] or 'lossy' in row[2].lower()
                    candidates.append((reduced, '3:2' not in row[2], float(m.group(3)), row[2],
                                       html.unescape(m.group(1)), m.group(2)))
            if not candidates:
                print(f'  no CC0 sample for {make} {model}', file=sys.stderr)
                continue
            _, _, size, mode, url, sha = min(candidates)
            chosen.append({'make': make, 'model': model, 'mode': mode, 'url': url, 'sha256': sha,
                           'size_mb': size, 'license': 'CC0-1.0'})
    MANIFEST.write_text(json.dumps({
        'about': 'CC0 sample RAWs from raw.pixls.us, downloaded by scripts/corpus/pixls.py into '
                 '$RAWMAKASE_CORPUS/raws/pixls. Not committed.',
        'files': chosen}, indent=1) + '\n')
    print(f'{len(chosen)} files, {sum(f["size_mb"] for f in chosen):.0f} MB -> {MANIFEST.relative_to(ROOT)}')


def du(path):
    """Bytes the corpus itself uses; symlinked RAWs live elsewhere and don't count."""
    if not path.exists():
        return 0
    return sum(p.stat().st_size for p in path.rglob('*') if p.is_file() and not p.is_symlink())


def target(root, f):
    name = f['url'].rsplit('/', 1)[-1]
    return root / 'raws/pixls' / f['make'] / f['model'] / name


def sha256(path):
    h = hashlib.sha256()
    with open(path, 'rb') as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b''):
            h.update(chunk)
    return h.hexdigest()


def download(args):
    root = corpus()
    files = json.loads(MANIFEST.read_text())['files']
    missing = [f for f in files if not target(root, f).exists()]
    needed = sum(f['size_mb'] for f in missing) * 1e6
    used = du(root)
    if used + needed > args.budget_gb * 1e9:
        sys.exit(f'Corpus uses {used / 1e9:.2f} GB; {needed / 1e9:.2f} GB more would exceed the '
                 f'{args.budget_gb} GB budget. Raise --budget-gb or delete something first.')
    for f in missing:
        path = target(root, f)
        path.parent.mkdir(parents=True, exist_ok=True)
        print(f'  {f["make"]} {f["model"]} ({f["size_mb"]:.0f} MB)', flush=True)
        tmp = path.with_suffix(path.suffix + '.part')
        urllib.request.urlretrieve(urllib.parse.quote(f['url'], safe=':/'), tmp)
        if sha256(tmp) != f['sha256']:
            tmp.unlink()
            sys.exit(f'{path.name}: checksum mismatch')
        tmp.rename(path)
    print(f'{len(missing)} downloaded; corpus now {du(root) / 1e9:.2f} GB')


def slug(make, model):
    return re.sub(r'[^a-z0-9]+', '-', f'{make} {model}'.lower()).strip('-')


def cameras(args):
    root = corpus()
    raws = sorted(p for p in (root / 'raws').rglob('*') if p.suffix.lower() in RAW_EXTENSIONS)
    found = {}
    with tempfile.TemporaryDirectory() as data:
        for raw in raws:
            out = subprocess.run([str(args.rawmakase), 'inspect', str(raw)], capture_output=True, text=True,
                                 env={**os.environ, 'RAWMAKASE_DATA_DIR': data})
            start = out.stdout.find('{')
            if out.returncode != 0 or start < 0:
                print(f'  cannot open {raw.name}: {out.stderr.strip()[:120]}', file=sys.stderr)
                continue
            meta, _ = json.JSONDecoder().raw_decode(out.stdout[start:])
            m = meta.get('cam_xyz')
            if not m or not any(any(abs(v) > 0 for v in row) for row in m):
                print(f'  no color matrix: {raw.name}', file=sys.stderr)
                continue
            key = slug(meta['make'], meta['model'])
            if key in NO_CHART:
                print(f'  no chart for {key}: {NO_CHART[key]}', file=sys.stderr)
                continue
            found.setdefault(key, {'id': key, 'make': meta['make'], 'model': meta['model'],
                                   'color_matrix1': [[round(v, 6) for v in row] for row in m]})
    CAMERAS.write_text(json.dumps(sorted(found.values(), key=lambda c: c['id']), indent=1) + '\n')
    print(f'{len(found)} cameras -> {CAMERAS.relative_to(ROOT)}')


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='command', required=True)
    sub.add_parser('manifest').set_defaults(run=build_manifest)
    d = sub.add_parser('download')
    d.add_argument('--budget-gb', type=float, default=5.)
    d.set_defaults(run=download)
    c = sub.add_parser('cameras')
    c.add_argument('--rawmakase', type=Path, default=ROOT / 'target/release/rawmakase')
    c.set_defaults(run=cameras)
    args = p.parse_args()
    args.run(args)


if __name__ == '__main__':
    main()
