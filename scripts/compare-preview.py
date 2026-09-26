#!/usr/bin/env python3
"""Compare sRGB previews when RAW decoders differ slightly in active dimensions.
Requires Pillow and numpy. No alignment, exposure fitting, or color fitting.
"""
import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('reference', type=Path)
parser.add_argument('candidates', type=Path, nargs='+')
parser.add_argument('--long-edge', type=int, default=800)
args = parser.parse_args()
if not 64 <= args.long_edge <= 4096:
    parser.error('--long-edge must be between 64 and 4096')
reference = Image.open(args.reference).convert('RGB')
ratio = reference.width / reference.height
scale = args.long_edge / max(reference.size)
size = tuple(round(v * scale) for v in reference.size)
a = np.asarray(reference.resize(size, Image.Resampling.LANCZOS), dtype=float) / 255
results = []
for path in args.candidates:
    candidate = Image.open(path).convert('RGB')
    if abs((candidate.width / candidate.height) / ratio - 1) > 0.005:
        parser.error(f'{path}: aspect ratio differs by more than 0.5%; check crop/orientation')
    b = np.asarray(candidate.resize(size, Image.Resampling.LANCZOS), dtype=float) / 255
    diff = a - b
    results.append(dict(candidate=str(path), mae=float(np.abs(diff).mean()),
                        rmse=float(np.sqrt((diff**2).mean()))))
print(json.dumps(dict(reference=str(args.reference), preview_size=size,
                     space='encoded sRGB, 8-bit preview samples, no fitting', results=results), indent=2))
