#!/usr/bin/env python3
"""Write the synthetic look profiles in tests/corpus/looks.

Each is a Camera Raw look profile (an XMP with an HSV look table, a tone curve and
profile-internal settings) made up from simple formulas here: no Adobe profile data.
They exercise Lightroom's Profile Amount, which only looks with `SupportsAmount` have.
Camera Raw reads a look from a sidecar's `crs:Look` and its `Table_` attribute, so
scripts/corpus/camera-raw-charts.py can render them without installing anything.

Run from the repository root: python3 scripts/corpus/synthetic-looks.py
"""
import hashlib
import math
import struct
import zlib
from pathlib import Path

OUT = Path(__file__).resolve().parents[2] / 'tests/corpus/looks'
ALPHABET = b"0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ.-:+=^!/*?`'|()[]{}@%$#"


def base85(data):
    """Adobe's table encoding: zlib with the expanded length first, in base 85."""
    packed = struct.pack('<I', len(data)) + zlib.compress(data, 9)
    out = ''
    for i in range(0, len(packed), 4):
        chunk = packed[i:i + 4]
        value = struct.unpack('<I', chunk + b'\0' * (4 - len(chunk)))[0]
        for _ in range(len(chunk) + 1):
            out += chr(ALPHABET[value % 85])
            value //= 85
    return out


def table(dims, f, srgb=True, bounds=(0., 2.)):
    """A DNG SDK HSV look table: values outermost, then hues, then saturations. A
    version 2 table stores the bounds its Profile Amount scales between."""
    hues, sats, vals = dims
    data = struct.pack('<5I', 0, 2 if bounds else 1, hues, sats, vals)
    for v in range(vals):
        for h in range(hues):
            for s in range(sats):
                data += struct.pack('<3f', *f(h / hues, s / (sats - 1), v / max(vals - 1, 1)))
    data += struct.pack('<I', 1 if srgb else 0)
    if bounds:
        data += struct.pack('<2d', *bounds)
    return data


def look(name, data, settings=None, curve=None, monochrome=False):
    uuid = hashlib.md5(name.encode()).hexdigest().upper()
    table_id = hashlib.md5(data).hexdigest().upper()
    attributes = {
        'PresetType': 'Look', 'Cluster': '', 'UUID': uuid, 'SupportsAmount': 'True',
        'SupportsColor': 'True', 'SupportsMonochrome': 'True' if monochrome else 'False',
        'SupportsHighDynamicRange': 'True', 'SupportsNormalDynamicRange': 'True',
        'SupportsSceneReferred': 'True', 'SupportsOutputReferred': 'False',
        'CameraModelRestriction': '', 'Copyright': 'Synthetic test look, public domain',
        'ContactInfo': '', 'Version': '18.7', 'ProcessVersion': '15.4',
        'ConvertToGrayscale': 'True' if monochrome else 'False', 'LookTable': table_id,
        **(settings or {}),
        f'Table_{table_id}': base85(data),
    }
    xml = ('<x:xmpmeta xmlns:x="adobe:ns:meta/">\n <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">\n'
           '  <rdf:Description rdf:about=""\n    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"')
    xml += ''.join(f'\n   crs:{k}="{v}"' for k, v in attributes.items()) + '>\n'
    for element, text in [('Name', name), ('Group', 'Synthetic')]:
        xml += (f'   <crs:{element}>\n    <rdf:Alt>\n     <rdf:li xml:lang="x-default">{text}</rdf:li>\n'
                f'    </rdf:Alt>\n   </crs:{element}>\n')
    for channel in ['', 'Red', 'Green', 'Blue']:
        points = (curve or IDENTITY) if channel == '' else IDENTITY
        xml += (f'   <crs:ToneCurvePV2012{channel}>\n    <rdf:Seq>\n'
                + ''.join(f'     <rdf:li>{p}</rdf:li>\n' for p in points)
                + f'    </rdf:Seq>\n   </crs:ToneCurvePV2012{channel}>\n')
    return xml + '  </rdf:Description>\n </rdf:RDF>\n</x:xmpmeta>\n'


def twist(h, s, v):
    """Hue shifts of up to 35°, saturation ±35% and value −12% growing with saturation."""
    return (25 * math.sin(2 * math.pi * h) + 10 * math.cos(4 * math.pi * h) * s,
            1 + 0.35 * math.cos(2 * math.pi * (h - 0.1)) * s,
            1 - 0.12 * s + 0.05 * math.sin(2 * math.pi * h) * s)


def identity(h, s, v):
    return (0., 1., 1.)


IDENTITY = ['0, 0', '255, 255']
S_CURVE = ['0, 0', '64, 44', '128, 132', '192, 214', '255, 255']
LOOKS = {
    'synthetic-table.xmp': look('Synthetic Table', table((12, 5, 6), twist)),
    'synthetic-table-v1.xmp': look('Synthetic Table v1', table((12, 5, 6), twist, bounds=None)),
    'synthetic-curve.xmp': look('Synthetic Curve', table((4, 2, 1), identity, srgb=False), curve=S_CURVE),
    'synthetic-tones.xmp': look('Synthetic Tones', table((4, 2, 1), identity, srgb=False),
                                {'Shadows2012': '+40', 'Highlights2012': '-40'}),
    'synthetic-contrast.xmp': look('Synthetic Contrast', table((4, 2, 1), identity, srgb=False),
                                   {'Contrast2012': '+40', 'Blacks2012': '-20'}),
    'synthetic-mono.xmp': look('Synthetic Mono', table((12, 5, 6), twist), {'Contrast2012': '+20'},
                               curve=S_CURVE, monochrome=True),
    'synthetic-look.xmp': look('Synthetic Look', table((12, 5, 6), twist),
                               {'Shadows2012': '+25', 'Clarity2012': '+10'}, curve=S_CURVE),
}

if __name__ == '__main__':
    OUT.mkdir(parents=True, exist_ok=True)
    for name, text in LOOKS.items():
        (OUT / name).write_text(text)
        print(OUT / name)
