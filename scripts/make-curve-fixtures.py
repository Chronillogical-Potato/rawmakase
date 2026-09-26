#!/usr/bin/env python3
"""Generate private-data-free 16-bit sRGB ramps for a Lightroom curve comparison.

Requires numpy, Pillow and tifffile. Import into a disposable Lightroom catalog,
read metadata from the files, then export 16-bit sRGB TIFF without resizing or
output sharpening. Reading metadata explicitly avoids import-default overrides.
"""
import argparse
from pathlib import Path

import numpy as np
import tifffile
from PIL import ImageCms

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("output", type=Path, help="New directory for generated TIFFs")
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
x = np.linspace(0, 1, 1024)
strips = [np.stack([x, x, x], -1)]
for c in range(3):
    strip = np.full((1024, 3), .25)
    strip[:, c] = x
    strips.append(strip)
for c in range(3):
    strip = np.tile(x[:, None], (1, 3))
    strip[:, c] = .5
    strips.append(strip)
pixels = np.uint16(np.round(np.repeat(np.stack(strips), 32, axis=0) * 65535))
identity = [(0, 0), (255, 255)]
s_curve = [(0, 0), (48, 24), (128, 142), (200, 224), (255, 255)]
clipped = [(32, 16), (96, 120), (200, 224)]
red = [(0, 12), (72, 52), (170, 195), (255, 244)]
blue = [(0, 0), (64, 86), (192, 168), (255, 255)]
cases = {
    "linear": [identity] * 4,
    "s": [s_curve, identity, identity, identity],
    "rgb": [identity, red, identity, blue],
    "clipped": [clipped, identity, identity, identity],
    "rgb-clipped": [identity, clipped, identity, [(0, 0), (64, 180), (192, 64), (255, 255)]],
}
icc = ImageCms.ImageCmsProfile(ImageCms.createProfile("sRGB")).tobytes()
settings = {
    "Version": "15.4", "ProcessVersion": "11.0", "HasSettings": "True",
    "AlreadyApplied": "False", "WhiteBalance": "As Shot", "CameraProfile": "Embedded",
    "Exposure2012": 0, "Contrast2012": 0, "Highlights2012": 0, "Shadows2012": 0,
    "Whites2012": 0, "Blacks2012": 0, "Texture": 0, "Clarity2012": 0, "Dehaze": 0,
    "Vibrance": 0, "Saturation": 0, "Sharpness": 0, "LuminanceSmoothing": 0,
    "ColorNoiseReduction": 0, "ToneCurveName2012": "Custom", "CurveRefineSaturation": 100,
}
for name, curves in cases.items():
    attrs = " ".join(f'crs:{k}="{v}"' for k, v in settings.items())
    body = ""
    for suffix, points in zip(["", "Red", "Green", "Blue"], curves):
        tag = "crs:ToneCurvePV2012" + suffix
        body += f"<{tag}><rdf:Seq>" + "".join(f"<rdf:li>{a}, {b}</rdf:li>" for a, b in points) + f"</rdf:Seq></{tag}>"
    xmp = (f'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF '
           f'xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
           f'<rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" {attrs}>'
           f'{body}</rdf:Description></rdf:RDF></x:xmpmeta>').encode()
    tifffile.imwrite(args.output / f"rawmakase-curve-{name}.tif", pixels,
                     photometric="rgb", metadata=None,
                     extratags=[(34675, "B", len(icc), icc, False), (700, "B", len(xmp), xmp, False)])
print(args.output)
