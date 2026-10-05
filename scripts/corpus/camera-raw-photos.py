#!/usr/bin/env python3
"""Render the corpus photos in Camera Raw and keep only block averages.

For every RAW under $RAWMAKASE_CORPUS/raws (or the --raws subset) and every case
marked `photos` in tests/corpus/cases.json, Photoshop 2026 opens a copy of the RAW
with the case's settings plus CameraProfile="Adobe Standard", converts to 16-bit
sRGB at 2000 px long edge and saves a TIFF. Each TIFF is reduced to 48-across block
averages (scripts/corpus/blocks.py) in
$RAWMAKASE_CORPUS/camera-raw-photos/<raw path>/<case>.json and deleted.

Originals are never opened: RAWs are copied into the work folder first, since Camera
Raw writes sidecars next to the file it opens (and into DNGs). The script refuses to
start while Photoshop has documents open. Check them with:
  RAWMAKASE_CORPUS=... cargo test --test color photos_camera_raw -- --ignored --nocapture

Requires numpy. Run from the repository root.
"""
import argparse
import datetime
import json
import os
import plistlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

sys.dont_write_bytecode = True  # no __pycache__ in the source tree
sys.path.insert(0, str(Path(__file__).parent))
import blocks  # noqa: E402
import tiff16  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
CORPUS = ROOT / 'tests/corpus'
PHOTOSHOP = 'Adobe Photoshop 2026'
CAMERA_RAW_PLIST = Path('/Library/Application Support/Adobe/Plug-Ins/CC/File Formats/'
                        'Camera Raw.plugin/Contents/Info.plist')
RAW_EXTENSIONS = {'.arw', '.raf', '.nef', '.nrw', '.cr2', '.cr3', '.dng', '.rw2', '.orf', '.ori', '.pef',
                  '.rwl', '.fff', '.3fr'}
EDGE = 2000

TEMPLATE = r'''#target photoshop
app.displayDialogs = DialogModes.NO;
var jobs = %(jobs)s;
for (var i = 0; i < jobs.length; i++) {
  var j = jobs[i];
  var out = new File(j.out);
  if (out.exists) continue;
  new Folder(out.parent).create();
  var raw = new File(j.raw);
  if (j.fresh || !raw.exists) {
    if (raw.exists) raw.remove();
    new File(j.source).copy(j.raw);
  }
  var x = new File(j.xmp);
  x.encoding = "UTF-8"; x.open("w"); x.write(j.settings); x.close();
  var doc;
  try { doc = app.open(raw); } catch (e) { continue; }
  try {
    if (doc.bitsPerChannel != BitsPerChannelType.SIXTEEN) doc.bitsPerChannel = BitsPerChannelType.SIXTEEN;
    doc.convertProfile("sRGB IEC61966-2.1", Intent.RELATIVECOLORIMETRIC, true, false);
    var w = doc.width.as("px"), h = doc.height.as("px"), s = %(edge)d / Math.max(w, h);
    if (s < 1) doc.resizeImage(UnitValue(Math.round(w * s), "px"), UnitValue(Math.round(h * s), "px"), null, ResampleMethod.BICUBIC);
    var o = new TiffSaveOptions(); o.imageCompression = TIFFEncoding.NONE; o.embedColorProfile = true;
    doc.saveAs(out, o, true);
  } finally {
    doc.close(SaveOptions.DONOTSAVECHANGES);
  }
}
'''


def xmp(base, case, extra):
    attributes = dict(base)
    attributes.update(case.get('settings', {}))
    attributes.update(extra)
    xml = ('<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
           '<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:HasSettings="True"')
    for k in sorted(attributes):
        xml += f' crs:{k}="{attributes[k]}"'
    xml += '>'
    for name, points in sorted(case.get('curves', {}).items()):
        xml += f'<crs:{name}><rdf:Seq>' + ''.join(f'<rdf:li>{p}</rdf:li>' for p in points) + f'</rdf:Seq></crs:{name}>'
    return xml + '</rdf:Description></rdf:RDF></x:xmpmeta>'


def camera_raw_version():
    if not CAMERA_RAW_PLIST.exists():
        return '?'
    return plistlib.loads(CAMERA_RAW_PLIST.read_bytes()).get('CFBundleShortVersionString', '?').split()[0]


def photoshop_idle():
    out = subprocess.run(['osascript', '-e', f'tell application "{PHOTOSHOP}" to count documents'],
                         capture_output=True, text=True)
    return out.returncode == 0 and out.stdout.strip() == '0'


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--raws', nargs='*', help='only RAWs whose path under raws/ contains one of these')
    p.add_argument('--cases', nargs='*', help='only cases whose name contains one of these')
    p.add_argument('--profile', default='Adobe Standard', help='CameraProfile for Camera Raw')
    p.add_argument('--redo', action='store_true', help='render cases that already have a reference')
    args = p.parse_args()
    corpus = Path(os.environ.get('RAWMAKASE_CORPUS') or sys.exit('Set RAWMAKASE_CORPUS'))
    cases_doc = json.loads((CORPUS / 'cases.json').read_text())
    cases = [c for c in cases_doc['cases'] if c.get('photos')
             and (not args.cases or any(t in c['name'] for t in args.cases))]
    raws = sorted(r for r in (corpus / 'raws').rglob('*') if r.suffix.lower() in RAW_EXTENSIONS
                  and (not args.raws or any(t in str(r.relative_to(corpus / 'raws')) for t in args.raws)))
    out_dir = corpus / 'camera-raw-photos'
    work = Path(tempfile.mkdtemp(prefix='camera-raw-photos-', dir=corpus))
    extra = {'CameraProfile': args.profile}
    jobs, planned = [], []
    for i, raw in enumerate(raws):
        relative = raw.relative_to(corpus / 'raws')
        copy = work / f'{i}{raw.suffix}'
        for case in cases:
            target = out_dir / relative.with_suffix('') / f"{case['name']}.json"
            if target.exists() and not args.redo:
                continue
            tiff = work / 'out' / f"{i}-{case['name']}.tif"
            settings = xmp(cases_doc['base'], case, extra)
            planned.append((relative, case['name'], tiff, target, settings))
            jobs.append({'source': str(raw.resolve()), 'raw': str(copy), 'xmp': str(copy.with_suffix('.xmp')),
                         # Camera Raw writes settings into DNGs it opens.
                         'fresh': raw.suffix.lower() == '.dng', 'settings': settings, 'out': str(tiff)})
    if not jobs:
        shutil.rmtree(work)
        sys.exit('Nothing to render (use --redo to replace existing references)')
    if not photoshop_idle():
        shutil.rmtree(work)
        sys.exit(f'{PHOTOSHOP} is not running or has documents open; close them (another session may be using it).')
    (work / 'render.jsx').write_text(TEMPLATE % {'jobs': json.dumps(jobs), 'edge': EDGE})
    print(f'Rendering {len(jobs)} cases on {len(raws)} photos in {work}', flush=True)
    try:
        subprocess.run(['osascript', '-e', f'with timeout of 360000 seconds\ntell application "{PHOTOSHOP}" to '
                        f'do javascript file (POSIX file "{work / "render.jsx"}")\nend timeout'],
                       check=True, stdout=subprocess.DEVNULL)
        version, done, missing = camera_raw_version(), 0, []
        for relative, name, tiff, target, settings in planned:
            if not tiff.exists():
                missing.append(f'{relative} / {name}')
                continue
            image = tiff16.read(tiff)
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(json.dumps({
                'raw': str(relative), 'case': name, 'camera_raw': version,
                'rendered': datetime.date.today().isoformat(), 'settings': settings,
                'width': image.shape[1], 'height': image.shape[0], 'blocks': blocks.reduce(image),
            }, separators=(',', ':')) + '\n')
            tiff.unlink()
            done += 1
        print(f'{done} references written to {out_dir}')
        for m in missing:
            print(f'  Camera Raw did not render {m}', file=sys.stderr)
    finally:
        shutil.rmtree(work)


if __name__ == '__main__':
    main()
