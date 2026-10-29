#!/usr/bin/env python3
"""Writes reference.json: what Lensfun computes for the synthetic calibrations in
cases.json, which RAWmakase's Lensfun tests compare with (`optics::lensfun`).

The calibrations are made up for the tests; no Lensfun database data is used. Lensfun
itself only runs here, offline: build it (https://github.com/lensfun/lensfun, checked
at revision bc5615d6), then

    g++ -O1 -std=c++17 -I$BUILD probe.cpp -L$BUILD/libs/lensfun -llensfun \\
        -Wl,-rpath,$BUILD/libs/lensfun -o probe
    python3 make_reference.py ./probe
"""
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent


def points(w, h):
    xs = [0, w * 0.1, w * 0.25, (w - 1) / 2, w * 0.7, w * 0.9, w - 1]
    ys = [0, h * 0.2, (h - 1) / 2, h * 0.8, h - 1]
    return [[round(x, 1), round(y, 1)] for x in xs for y in ys]


def run(probe, case, shot, mode, pts):
    args = [probe, str(shot["focal"]), str(case["crop"]), str(case["width"]), str(case["height"]),
            str(shot["aperture"]), str(shot["distance"]), mode] + [f"{x},{y}" for x, y in pts]
    out = subprocess.run(args, input=case["xml"], capture_output=True, text=True, check=True).stdout
    return [[float(v) for v in line.split()] for line in out.splitlines()]


def main():
    probe = sys.argv[1]
    cases = json.loads((HERE / "cases.json").read_text())
    for case in cases:
        pts = points(case["width"], case["height"])
        case["points"] = pts
        for shot in case["shots"]:
            for mode in ("d", "t", "dt", "v"):
                rows = run(probe, case, shot, mode, pts)
                shot[mode] = {"on": int(rows[0][0]), "values": [r[1:] for r in rows]}
    (HERE / "reference.json").write_text("[\n" + ",\n".join(json.dumps(c, separators=(",", ":")) for c in cases) + "\n]\n")


main()
