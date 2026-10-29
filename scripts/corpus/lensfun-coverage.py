#!/usr/bin/env python3
"""How much Lensfun's database could add to the corrections RAWmakase reads from the
dataset's raw.pixls.us CC0 photos, by RAWmakase's own matcher (docs/lens-corrections.md,
"Lensfun profiles").

Joins the committed decode fingerprints (tests/corpus/dataset/pixls-decode.json: the
decoded frame, focal lengths, lens name and built-in corrections, as RAWmakase reads
them) with the dataset index's EXIF make and model (data/dataset.sqlite of
pch/rawmakase-dataset), runs `examples/lensfun_coverage.rs` over them against a
Lensfun `data/db` folder, and counts. Nothing is rendered and no photo is opened.

    python3 scripts/corpus/lensfun-coverage.py DATASET.sqlite LENSFUN/data/db [--exif-lens]

The lens name is the one RAWmakase decodes, as Develop matches it; --exif-lens takes
the dataset's EXIF lens name where RAWmakase reads none, an upper bound for reading
more lens names.
"""
import collections
import json
import sqlite3
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def components(lens):
    if not lens:
        return set()
    out = set()
    if lens.get("distortion"):
        out.add("D")
    if lens.get("chromatic"):
        out.add("C")
    if lens.get("vignetting") or lens.get("gain_map"):
        out.add("V")
    return out


def main():
    dataset, lensfun = sys.argv[1], sys.argv[2]
    exif_lens_too = "--exif-lens" in sys.argv[3:]
    decoded = json.loads((ROOT / "tests/corpus/dataset/pixls-decode.json").read_text())["files"]
    db = sqlite3.connect(f"file:{dataset}?mode=ro", uri=True)
    exif = {r[0]: r[1:] for r in db.execute("SELECT sha256, exif_make, exif_model, lens FROM exif")}
    rows, builtin = [], {}
    for f in decoded:
        m = (f.get("header") or {}).get("metadata")
        if not m or f["sha256"] not in exif:
            continue
        make, model, exif_lens = exif[f["sha256"]]
        lens = m.get("lens_model") or (exif_lens if exif_lens_too else "") or ""
        w, h = m.get("crop_width") or m["width"], m.get("crop_height") or m["height"]
        rows.append("\t".join(str(v).replace("\t", " ") for v in [
            f["sha256"], make or "", model or "", lens, m.get("focal", 0), m.get("aperture", 0), w, h,
            m.get("focal_35mm", 0)]))
        builtin[f["sha256"]] = components(m.get("lens")) | components(m.get("lens_stand_in"))
    out = subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--locked", "--example", "lensfun_coverage", "--", lensfun],
        input="\n".join(rows) + "\n", capture_output=True, text=True, check=True, cwd=ROOT).stdout
    counts = collections.Counter()
    for line in out.splitlines():
        sha, kind, crop, gives, *lenses = line.split("\t")
        counts["photos"] += 1
        counts[kind] += 1
        if kind != "unique":
            continue
        gives = set(gives) - {"-"}
        own = builtin[sha]
        if not gives:
            counts["unique, corrects nothing here"] += 1
            continue
        counts["unique, corrects something"] += 1
        added, lost = gives - own, own - gives
        if added:
            counts["adds a correction the photo lacks"] += 1
            for c in sorted(added):
                counts[f"adds {c}"] += 1
        if not own:
            counts["photo has no built-in correction"] += 1
        if lost:
            counts["would lose a built-in correction if chosen"] += 1
    for key, value in counts.items():
        print(f"{value:6}  {key}")


main()
