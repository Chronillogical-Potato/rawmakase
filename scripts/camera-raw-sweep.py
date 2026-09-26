#!/usr/bin/env python3
"""Write a Photoshop script that renders Camera Raw slider sweeps as reference TIFFs.

For every RAW in --raws and every slider value, the generated ExtendScript writes an
XMP sidecar next to the RAW (Adobe Standard, no lens profile, As Shot white balance
plus the one slider), opens the RAW through Camera Raw with those settings, converts
to 16-bit sRGB, resizes to --long-edge and saves `<stem>-<slider><value>.tif`.
Existing outputs are skipped. Use copies of RAWs: sidecars are overwritten.

Run the result with:
  osascript -e 'tell application "Adobe Photoshop 2026" to do javascript file (POSIX file "sweep.jsx")'
Score it with scripts/lightroom-scorecard.py.
"""
import argparse
import json
from pathlib import Path

SLIDERS = {
    "Exposure2012": [-2, -1, -0.5, 0.5, 1, 2],
    "Contrast2012": [-100, -50, -25, 25, 50, 100],
    "Highlights2012": [-100, -60, -30, 30, 60, 100],
    "Shadows2012": [-100, -60, -30, 30, 60, 100],
    "Whites2012": [-100, -50, -25, 25, 50, 100],
    "Blacks2012": [-100, -50, -25, 25, 50, 100],
    "Clarity2012": [-100, -50, -25, 25, 50, 100],
    "Texture": [-50, 50],
    "Dehaze": [-100, -40, -20, 20, 40, 100],
    "PerspectiveVertical": [-100, -50, 50, 100],
    "PerspectiveHorizontal": [-100, -50, 50, 100],
    "PerspectiveRotate": [-5, 5],
    "PerspectiveAspect": [-50, 50],
    "PerspectiveScale": [80, 120],
    "PerspectiveX": [-50, 50],
    "PerspectiveY": [-50, 50],
    "Saturation": [-50, 50],
    "Vibrance": [-50, 50],
    "HueAdjustmentRed": [-100, 100],
    "HueAdjustmentOrange": [-100, 100],
    "HueAdjustmentYellow": [-100, 100],
    "HueAdjustmentGreen": [-100, 100],
    "HueAdjustmentAqua": [-100, 100],
    "HueAdjustmentBlue": [-100, 100],
    "HueAdjustmentPurple": [-100, 100],
    "HueAdjustmentMagenta": [-100, 100],
    "SaturationAdjustmentRed": [-100, 100],
    "SaturationAdjustmentOrange": [-100, 100],
    "SaturationAdjustmentYellow": [-100, 100],
    "SaturationAdjustmentGreen": [-100, 100],
    "SaturationAdjustmentAqua": [-100, 100],
    "SaturationAdjustmentBlue": [-100, 100],
    "SaturationAdjustmentPurple": [-100, 100],
    "SaturationAdjustmentMagenta": [-100, 100],
    "LuminanceAdjustmentRed": [-100, 100],
    "LuminanceAdjustmentOrange": [-100, 100],
    "LuminanceAdjustmentYellow": [-100, 100],
    "LuminanceAdjustmentGreen": [-100, 100],
    "LuminanceAdjustmentAqua": [-100, 100],
    "LuminanceAdjustmentBlue": [-100, 100],
    "LuminanceAdjustmentPurple": [-100, 100],
    "LuminanceAdjustmentMagenta": [-100, 100],
}

TEMPLATE = r'''#target photoshop
app.displayDialogs = DialogModes.NO;
var RAWS = %(raws)s, OUT = %(out)s, EDGE = %(edge)d;
var jobs = %(jobs)s;
for (var i = 0; i < jobs.length; i++) {
  var j = jobs[i], stem = j.photo.replace(/\.[^.]+$/, "");
  var out = new File(OUT + "/" + stem + "-" + j.name + ".tif");
  if (out.exists) continue;
  var x = new File(RAWS + "/" + stem + ".xmp");
  x.encoding = "UTF-8"; x.open("w");
  x.write('<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
    + '<rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:ProcessVersion="15.4"'
    + ' crs:CameraProfile="Adobe Standard" crs:LensProfileEnable="0" crs:AutoLateralCA="0" crs:WhiteBalance="As Shot" '
    + j.attrs + '></rdf:Description></rdf:RDF></x:xmpmeta>');
  x.close();
  var doc = app.open(new File(RAWS + "/" + j.photo));
  if (doc.bitsPerChannel != BitsPerChannelType.SIXTEEN) doc.bitsPerChannel = BitsPerChannelType.SIXTEEN;
  doc.convertProfile("sRGB IEC61966-2.1", Intent.RELATIVECOLORIMETRIC, true, false);
  var w = doc.width.as("px"), h = doc.height.as("px"), s = EDGE / Math.max(w, h);
  if (s < 1) doc.resizeImage(UnitValue(Math.round(w * s), "px"), UnitValue(Math.round(h * s), "px"), null, ResampleMethod.BICUBIC);
  var o = new TiffSaveOptions(); o.imageCompression = TIFFEncoding.NONE; o.embedColorProfile = true;
  doc.saveAs(out, o, true);
  doc.close(SaveOptions.DONOTSAVECHANGES);
}
'''


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument('--raws', type=Path, required=True, help='directory of RAW copies')
    p.add_argument('--out', type=Path, required=True, help='directory for reference TIFFs')
    p.add_argument('--script', type=Path, required=True, help='ExtendScript file to write')
    p.add_argument('--sliders', nargs='*', default=list(SLIDERS), help='subset of slider names')
    p.add_argument('--long-edge', type=int, default=2000)
    args = p.parse_args()
    photos = sorted(f.name for f in args.raws.iterdir() if f.suffix.lower() in {'.raf', '.arw', '.nef', '.cr2', '.cr3', '.dng', '.orf', '.rw2'})
    variants = {'default': ''}
    for name in args.sliders:
        for v in SLIDERS[name]:
            variants[f"{name}{'+' if v > 0 else ''}{v:g}"] = f'crs:{name}="{v}"'
    jobs = [dict(photo=ph, name=n, attrs=a) for ph in photos for n, a in variants.items()]
    args.out.mkdir(parents=True, exist_ok=True)
    args.script.write_text(TEMPLATE % dict(raws=json.dumps(str(args.raws.resolve())), out=json.dumps(str(args.out.resolve())),
                                           edge=args.long_edge, jobs=json.dumps(jobs)))
    print(f'{len(jobs)} renders written to {args.script}')


if __name__ == '__main__':
    main()
