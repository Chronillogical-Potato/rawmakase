#!/usr/bin/env python3
"""Reduce existing Camera Raw / Lightroom reference TIFFs to corpus block files.

Each 16-bit TIFF in the given folders (renders from scripts/camera-raw-sweep.py or
Lightroom exports) must carry its develop settings as XMP, which both apps embed.
The source RAW is found under $RAWMAKASE_CORPUS/raws by crs:RawFileName, or by the
file name prefix before the first "-" (sweep outputs are named <raw>-<setting>.tif).
Each TIFF becomes $RAWMAKASE_CORPUS/camera-raw-photos/<raw path>/<rest of name>.json,
a few KB, checked by `cargo test --test color photos_camera_raw -- --ignored`.

TIFFs are only read; delete them yourself once the new references score the same.
Requires numpy and exiftool.
"""
import argparse
import base64
import json
import os
import re
import subprocess
import sys
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the source tree
sys.path.insert(0, str(Path(__file__).parent))
import blocks  # noqa: E402
import tiff16  # noqa: E402

RAW_EXTENSIONS = {'.arw', '.raf', '.nef', '.nrw', '.cr2', '.cr3', '.dng', '.rw2', '.orf', '.ori', '.pef',
                  '.rwl', '.fff', '.3fr'}


def packets(folder):
    """XMP packet of every TIFF in a folder, with one exiftool call."""
    out = subprocess.run(['exiftool', '-j', '-b', '-XMP', '-ext', 'tif', '-ext', 'tiff', str(folder)],
                         capture_output=True, text=True)
    result = {}
    for item in json.loads(out.stdout or '[]'):
        value = item.get('XMP')
        if isinstance(value, str) and value.startswith('base64:'):
            value = base64.b64decode(value[7:]).decode('utf-8', 'replace')
        if value:
            result[Path(item['SourceFile']).resolve()] = value
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('folders', type=Path, nargs='+')
    p.add_argument('--overwrite', action='store_true')
    args = p.parse_args()
    corpus = Path(os.environ.get('RAWMAKASE_CORPUS') or sys.exit('Set RAWMAKASE_CORPUS'))
    raws = {}
    for r in (corpus / 'raws').rglob('*'):
        if r.suffix.lower() in RAW_EXTENSIONS:
            raws.setdefault(r.stem.lower(), []).append(r)
    out_dir = corpus / 'camera-raw-photos'
    written, skipped = 0, []
    for folder in args.folders:
        for tiff, packet in sorted(packets(folder).items()):
            match = re.search(r'crs:RawFileName="([^"]+)"', packet) or \
                re.search(r'<crs:RawFileName>([^<]+)<', packet)
            stem = Path(match.group(1)).stem if match else tiff.stem.split('-', 1)[0]
            candidates = list(raws.get(stem.lower(), []))
            digits = re.fullmatch(r'[a-z]*(\d{3,})', stem.lower())
            if not candidates and digits:
                # Short names such as p7853 or 8200: the one RAW whose name ends in them.
                endings = {k for k in raws if k.endswith(digits.group(1))}
                if len(endings) == 1:
                    candidates = list(raws[endings.pop()])
            # Prefer the camera's own RAW over an Adobe DNG of the same photo.
            candidates.sort(key=lambda r: r.suffix.lower() == '.dng')
            if not candidates:
                skipped.append(f'{tiff}: no RAW named {stem} in the corpus')
                continue
            raw = candidates[0]
            if match and match.group(1).lower().endswith('.dng'):
                raw = next((r for r in candidates if r.suffix.lower() == '.dng'), raw)
            name = tiff.stem[len(stem) + 1:] if tiff.stem.lower().startswith(stem.lower() + '-') else 'default'
            relative = raw.relative_to(corpus / 'raws')
            target = out_dir / relative.with_suffix('') / f'{name}.json'
            def source(path):
                return json.loads(path.read_text()).get('source') if path.exists() else None
            alternative = target.with_name(f'{name}@{folder.parent.name}-{folder.name}.json')
            if str(tiff) in (source(target), source(alternative)) and not args.overwrite:
                continue
            if target.exists() and source(target) != str(tiff):
                # Same photo and name from another folder: keep both.
                target = alternative
            try:
                image = tiff16.read(tiff)
            except ValueError as e:
                skipped.append(str(e))
                continue
            version = re.search(r'crs:Version="([^"]+)"', packet) or re.search(r'<crs:Version>([^<]+)<', packet)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(json.dumps({
                'raw': str(relative), 'case': name, 'camera_raw': version.group(1) if version else '?',
                'source': str(tiff), 'settings': packet,
                'width': image.shape[1], 'height': image.shape[0], 'blocks': blocks.reduce(image),
            }, separators=(',', ':')) + '\n')
            written += 1
    print(f'{written} references written to {out_dir}')
    for s in skipped:
        print(f'  skipped {s}', file=sys.stderr)


if __name__ == '__main__':
    main()
