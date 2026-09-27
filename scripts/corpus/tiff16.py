"""Reads the uncompressed 8- or 16-bit RGB TIFFs Photoshop writes, keeping 16 bits.

Pillow reduces 16-bit RGB TIFFs to 8 bits, which is too coarse for patch averages.
"""
import struct

import numpy as np


def read(path):
    """Returns an H×W×3 float64 array in 0–1."""
    data = open(path, 'rb').read()
    order = {b'II': '<', b'MM': '>'}[data[:2]]
    (ifd,) = struct.unpack(order + 'I', data[4:8])
    (count,) = struct.unpack(order + 'H', data[ifd:ifd + 2])
    sizes = {1: 1, 2: 1, 3: 2, 4: 4, 5: 8, 7: 1, 16: 8}
    tags = {}
    for i in range(count):
        tag, kind, n, value = struct.unpack(order + 'HHII', data[ifd + 2 + 12 * i:ifd + 14 + 12 * i])
        size = sizes.get(kind, 1) * n
        raw = data[ifd + 10 + 12 * i:ifd + 14 + 12 * i] if size <= 4 else data[value:value + size]
        if kind == 3:
            tags[tag] = struct.unpack(order + 'H' * n, raw[:2 * n])
        elif kind == 4:
            tags[tag] = struct.unpack(order + 'I' * n, raw[:4 * n])
    width, height = tags[256][0], tags[257][0]
    bits = tags[258][0]
    if tags.get(259, (1,))[0] != 1:
        raise ValueError(f'{path}: compressed TIFF; save with TIFFEncoding.NONE')
    if tags.get(284, (1,))[0] != 1 or tags.get(277, (3,))[0] < 3:
        raise ValueError(f'{path}: expected interleaved RGB')
    spp = tags.get(277, (3,))[0]
    pixels = b''.join(data[o:o + c] for o, c in zip(tags[273], tags[279]))
    dtype = np.dtype(order + ('u2' if bits == 16 else 'u1'))
    a = np.frombuffer(pixels, dtype=dtype, count=width * height * spp).reshape(height, width, spp)
    return a[..., :3].astype(np.float64) / (65535 if bits == 16 else 255)
