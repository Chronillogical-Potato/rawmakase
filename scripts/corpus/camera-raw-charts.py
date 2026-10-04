#!/usr/bin/env python3
"""Render the synthetic charts in Camera Raw and store its patch values.

For each chart and each case in tests/corpus/cases.json that applies to it, Photoshop
2026 opens a fresh copy of the chart DNG with the case's settings in a sidecar XMP,
converts to 16-bit sRGB and saves an uncompressed TIFF. Each TIFF is then reduced to
the mean encoded-sRGB value of every patch in tests/corpus/charts/layout.json and
deleted, so only a few KB per case remain.

Default: the synthetic charts with their embedded matrices (no Adobe profiles). The
results are committed in tests/corpus/camera-raw/ and checked by
`cargo test --test color camera_raw_parity`.

--adobe: the per-camera charts (cameras.json) with CameraProfile="Adobe Standard",
for the cases marked `photos` (add --all-cases for every case). Results are derived
from Adobe's profiles and stay private, in $RAWMAKASE_CORPUS/camera-raw-adobe/.

Only RAW/DNG files are ever opened in Photoshop: Camera Raw blocks Photoshop with a
dialog when a script opens a TIFF. The script refuses to start while Photoshop has
documents open, since another session may be using it.

Requires numpy. Run from the repository root:
  python3 scripts/corpus/camera-raw-charts.py [--charts NAME ...] [--cases TEXT ...]
"""
import argparse
import datetime
import json
import os
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np

sys.dont_write_bytecode = True  # no __pycache__ in the source tree
sys.path.insert(0, str(Path(__file__).parent))
import tiff16  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / 'tests/corpus'
PHOTOSHOP = 'Adobe Photoshop 2026'
CAMERA_RAW_PLIST = Path('/Library/Application Support/Adobe/Plug-Ins/CC/File Formats/'
                        'Camera Raw.plugin/Contents/Info.plist')

TEMPLATE = r'''#target photoshop
app.displayDialogs = DialogModes.NO;
var jobs = %(jobs)s;
for (var i = 0; i < jobs.length; i++) {
  var j = jobs[i];
  var out = new File(j.out);
  if (out.exists) continue;
  new Folder(out.parent).create();
  // A fresh copy for every job: Camera Raw writes into DNGs it opens.
  var dng = new File(j.dng);
  if (dng.exists) dng.remove();
  new File(j.source).copy(j.dng);
  var x = new File(j.xmp);
  x.encoding = "UTF-8"; x.open("w"); x.write(j.settings); x.close();
  var doc = app.open(dng);
  try {
    if (doc.bitsPerChannel != BitsPerChannelType.SIXTEEN) doc.bitsPerChannel = BitsPerChannelType.SIXTEEN;
    doc.convertProfile("sRGB IEC61966-2.1", Intent.RELATIVECOLORIMETRIC, true, false);
    var o = new TiffSaveOptions(); o.imageCompression = TIFFEncoding.NONE; o.embedColorProfile = true;
    doc.saveAs(out, o, true);
  } finally {
    doc.close(SaveOptions.DONOTSAVECHANGES);
  }
}
'''


def look_xmp(look):
    """A case's look as Lightroom writes it into a sidecar: the `Look` element with the
    profile's parameters, and its table as a top-level `Table_` attribute."""
    text = (CORPUS / 'looks' / look['file']).read_text()
    attributes = dict(re.findall(r'crs:(\w+)="([^"]*)"', text))
    name = re.search(r'xml:lang="x-default">([^<]*)<', text).group(1)
    meta = ('PresetType', 'Cluster', 'UUID', 'CameraModelRestriction', 'Copyright', 'ContactInfo')
    parameters = ' '.join(f'crs:{k}="{v}"' for k, v in attributes.items()
                          if k not in meta and not k.startswith(('Supports', 'Table_')))
    curves = ''.join(re.findall(r'(<crs:ToneCurvePV2012\w*>.*?</crs:ToneCurvePV2012\w*>)', text, re.S))
    element = (f'<crs:Look><rdf:Description crs:Name="{name}" crs:Amount="{look["amount"]}"'
               f' crs:UUID="{attributes["UUID"]}" crs:SupportsAmount="{attributes["SupportsAmount"].lower()}"'
               f' crs:SupportsMonochrome="{attributes["SupportsMonochrome"].lower()}" crs:SupportsOutputReferred="false">'
               f'<crs:Parameters><rdf:Description {parameters}>{curves}</rdf:Description></crs:Parameters>'
               '</rdf:Description></crs:Look>')
    tables = {k: v for k, v in attributes.items() if k.startswith('Table_')}
    return element, tables


def xmp(base, case, extra):
    attributes = dict(base)
    attributes.update(case.get('settings', {}))
    attributes.update(extra)
    look, tables = look_xmp(case['look']) if 'look' in case else ('', {})
    attributes.update(tables)
    xml = ('<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
           '<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:HasSettings="True"')
    for k in sorted(attributes):
        xml += f' crs:{k}="{attributes[k]}"'
    xml += '>' + look
    for name, points in sorted(case.get('curves', {}).items()):
        xml += f'<crs:{name}><rdf:Seq>' + ''.join(f'<rdf:li>{p}</rdf:li>' for p in points) + f'</rdf:Seq></crs:{name}>'
    return xml + '</rdf:Description></rdf:RDF></x:xmpmeta>'


def applies(case, chart):
    charts = case.get('charts')
    if charts is None:
        return chart == 'synthetic-d65'
    return '*' in charts or chart in charts


def patch_values(path, patches):
    image = tiff16.read(path)
    values = []
    for p in patches:
        area = image[p['y']:p['y'] + p['h'], p['x']:p['x'] + p['w']]
        values.append([int(round(min(max(v, 0.), 1.) * 65535)) for v in area.reshape(-1, 3).mean(0)])
    return values


def write_patch_file(path, about, cases):
    """Same layout as PatchFile::write in tests/color: one case per line."""
    lines = [f'  {json.dumps(k)}: {json.dumps(v, separators=(",", ":"))}' for k, v in sorted(cases.items())]
    text = '{\n "about": ' + json.dumps(about, sort_keys=True, separators=(',', ':')) + ',\n "cases": {\n'
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text + ',\n'.join(lines) + '\n }\n}\n')


def photoshop_idle():
    out = subprocess.run(['osascript', '-e', f'tell application "{PHOTOSHOP}" to count documents'],
                         capture_output=True, text=True)
    return out.returncode == 0 and out.stdout.strip() == '0'


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--charts', nargs='*', help='chart names (default: all synthetic charts, or all camera charts with --adobe)')
    p.add_argument('--cases', nargs='*', help='only cases whose name contains one of these')
    p.add_argument('--adobe', action='store_true', help='per-camera charts with Adobe Standard; private output')
    p.add_argument('--all-cases', action='store_true', help='with --adobe: every case, not only `photos` cases')
    p.add_argument('--work', type=Path, help='scratch folder for DNG copies and TIFFs (deleted afterwards)')
    p.add_argument('--keep-tiffs', action='store_true', help='keep the rendered TIFFs')
    args = p.parse_args()

    cases_doc = json.loads((CORPUS / 'cases.json').read_text())
    layout = json.loads((CORPUS / 'charts/layout.json').read_text())
    cameras = json.loads((CORPUS / 'cameras.json').read_text()) if (CORPUS / 'cameras.json').exists() else []
    if args.adobe:
        corpus = os.environ.get('RAWMAKASE_CORPUS')
        if not corpus:
            sys.exit('Set RAWMAKASE_CORPUS: Adobe-profile references stay outside the repository.')
        out_dir = Path(corpus) / 'camera-raw-adobe'
        charts = args.charts or [f"{c['id']}-d65" for c in cameras]
        extra = {'CameraProfile': 'Adobe Standard'}
    else:
        out_dir = CORPUS / 'camera-raw'
        charts = args.charts or sorted(p.stem for p in (CORPUS / 'charts').glob('synthetic-*.dng'))
        extra = {}

    work = args.work or Path(tempfile.mkdtemp(prefix='camera-raw-charts-'))
    jobs, planned = [], {}
    for chart in charts:
        source = CORPUS / 'charts' / f'{chart}.dng'
        if not source.exists():
            sys.exit(f'{source} does not exist')
        for case in cases_doc['cases']:
            if args.adobe:
                if not (case.get('photos') or (args.all_cases and applies(case, 'synthetic-d65'))):
                    continue
            elif not applies(case, chart):
                continue
            if args.cases and not any(t in case['name'] for t in args.cases):
                continue
            tiff = work / 'out' / chart / f"{case['name']}.tif"
            planned.setdefault(chart, []).append((case['name'], tiff))
            jobs.append({'source': str(source), 'dng': str(work / f'{chart}.dng'), 'xmp': str(work / f'{chart}.xmp'),
                         'settings': xmp(cases_doc['base'], case, extra), 'out': str(tiff)})
    if not jobs:
        sys.exit('Nothing to render')
    if not photoshop_idle():
        sys.exit(f'{PHOTOSHOP} is not running or has documents open; close them (another session may be using it).')

    script = work / 'render.jsx'
    work.mkdir(parents=True, exist_ok=True)
    script.write_text(TEMPLATE % {'jobs': json.dumps(jobs)})
    print(f'Rendering {len(jobs)} cases on {len(planned)} charts in {work}', flush=True)
    subprocess.run(['osascript', '-e', f'with timeout of 36000 seconds\ntell application "{PHOTOSHOP}" to do javascript file (POSIX file "{script}")\nend timeout'],
                   check=True, stdout=subprocess.DEVNULL)

    version = plistlib.loads(CAMERA_RAW_PLIST.read_bytes()).get('CFBundleShortVersionString', '?') \
        if CAMERA_RAW_PLIST.exists() else '?'
    for chart, rendered in planned.items():
        path = out_dir / f'{chart}.json'
        existing = json.loads(path.read_text())['cases'] if path.exists() else {}
        for name, tiff in rendered:
            if not tiff.exists():
                print(f'  missing render: {chart} / {name}', file=sys.stderr)
                continue
            existing[name] = patch_values(tiff, layout['patches'])
            if not args.keep_tiffs:
                tiff.unlink()
        about = {
            'what': 'Camera Raw render of each case: mean encoded sRGB (16-bit) per patch, in layout.json order.',
            'camera_raw': version.split()[0],
            'profile': extra.get('CameraProfile', 'Embedded (DNG color matrices)'),
            'rendered': datetime.date.today().isoformat(),
        }
        write_patch_file(path, about, existing)
        print(f'  {path.relative_to(ROOT) if path.is_relative_to(ROOT) else path}: {len(existing)} cases')
    if not args.keep_tiffs and not args.work:
        shutil.rmtree(work)


if __name__ == '__main__':
    main()
