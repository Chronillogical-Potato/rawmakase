# Camera table

`data/cameras.toml` holds what RAWmakase knows about camera bodies beyond what the raw
file says, one `[[camera]]` row per model. It is compiled into the app
(`src/cameras.rs`) and checked by a unit test, so a malformed row fails `cargo test`.
Today a row carries the camera's baseline exposure; other per-camera values (default
sharpening, crop and so on) can be added as new fields on the same rows.

## Baseline exposure

Camera Raw and Lightroom brighten every unedited raw by a per-camera amount, the
BaselineExposure Adobe writes into a DNG it converts. Without it, RAWmakase renders
most cameras darker than Lightroom: about 0.3 EV for Sony, Canon and Nikon bodies.

RAWmakase applies, in order:

1. A DNG's own BaselineExposure tag, or 0 when the DNG has none (the DNG default).
   The table is never used for DNGs.
2. The camera's row in `data/cameras.toml`.
3. For a camera without a row, the median of the rows of the same make, or the
   median of all rows when the make has none.

Fujifilm rows hold at DR100 and base ISO. Camera Raw's value for a Fujifilm raw is a
per-body constant minus the exposure midpoint shift in its maker notes, which is about
−0.72 EV at DR100 on X-Trans bodies (−0.49 on GFX, 0 on the small Bayer bodies), a
stop lower for each DR step and a stop higher at extended low ISO. RAWmakase adds the
difference from that DR100 shift to the row, so DR200 and DR400 photos work even when
the raw does not record the DR mode itself. Camera Raw keeps one value whatever the
shift for the X-T2 (measured at DR200) and, inferred from their equal values, the other
X-Trans III bodies (X-E3, X-Pro2, X-T20, X100F, X-H1); their rows say
`fujifilm_exposure_shift = "ignored"`.

The baseline is stored per edit, apart from the Exposure slider, so changing the table
changes photos opened afterwards. An existing edit keeps its value; Develop's
"Use camera exposure baseline" button applies the current one.

### How good the fallback is

Predicting each of the 180 bodies in the table on 2026-10-05 from the other rows
(leave-one-out) misses its row by:

| Rule | Mean error | 90th percentile | Worst |
|---|---|---|---|
| No baseline | 0.29 EV | 0.50 EV | 0.85 EV |
| Median of all rows | 0.20 EV | 0.45 EV | 0.90 EV |
| Median of the same make | 0.14 EV | 0.35 EV | 1.00 EV |

The worst cases are bodies unlike the rest of their make: Pentax's 645Z (−0.5 among
+0.5 bodies), Panasonic's GH5 II and G9, and the 1-inch compacts (Sony RX100 and ZV-1
at −0.25, Nikon Coolpix at −0.3, against +0.3 for their makes' larger sensors). No
rule from the raw's own metadata did better: within a make, neither white level nor
black level orders the baselines. The same-make median is the fallback.

The camera-matching DCPs (Camera Standard and so on) of some Sony bodies carry a
BaselineExposureOffset of −0.35 EV, which RAWmakase applies with those profiles; Adobe
Standard DCPs carry none.

## Adding a camera

Add one row by hand; nothing else is needed:

```toml
[[camera]]
make = "Panasonic"          # LibRaw's names: `raw-identify -v <file>` prints
model = "DC-S5M2"           #   "Normalized Make/Model"
aliases = ["S5 II"]         # optional
baseline_exposure = 0.35    # EV
source = "measured"         # adobe-dng | adobe-profile | fitted | measured
how = "Lightroom DNG export of one photo, read with exiftool -BaselineExposure"
checked = "2026-10-05"
sample = "1 photo"
```

`source` says where the number comes from:

- `adobe-dng`: the BaselineExposure tag of a DNG that Adobe DNG Converter, Lightroom or
  Camera Raw wrote for the camera (`exiftool -BaselineExposure file.dng`). Exact.
- `adobe-profile`: the BaselineExposureOffset tag of the camera's Adobe Standard DCP.
  Adobe's current profiles leave it out, so this is rare.
- `fitted`: fitted to Camera Raw renders of unedited photos, the median exposure
  offset over midtone areas; about ±0.05 EV per photo.
- `measured`: anything else; `how` says how.

Values are rounded to 0.05 EV. Only numbers are recorded: never commit raw files,
DNGs or Adobe profiles.

### Optional: filling rows by script

`scripts/cameras/fit-baselines.py` prints rows to paste:

- `--dng <folder>` reads BaselineExposure from DNGs made by Adobe DNG Converter from
  copies of sample raws, kept outside the repository. Check the make and model it
  prints against LibRaw's names.
- `--report <report.json>` fits rows from the exposure check in
  `tests/corpus/README.md` (`parity-report.py --photos`): Camera Raw renders of the
  corpus photos against RAWmakase's default renders. Rerun the check after changing
  rows; the second pass lands within ±0.05 EV.

## Sources of the current rows

Most rows (`adobe-dng`) come from Adobe DNG Converter 18.x: it converted a copy of one
CC0 [raw.pixls.us](https://raw.pixls.us) sample per camera (the files
`scripts/corpus/pixls.py` downloads), and the row is the DNG's BaselineExposure plus
the difference between Adobe's white level and LibRaw's, which RAWmakase scales by:
log2((LibRaw white − black) / (Adobe white − black)). That term is 0 for most bodies;
Canon's ISO-dependent white levels make it up to +0.8 EV, so a Canon row holds at the
sample's ISO and may be off by a few tenths at others.

Adobe's value also follows some shooting settings, which the rows leave out: Canon
Highlight Tone Priority and Fujifilm DR200 add 1 EV, extended low ISOs (Sony ISO 50,
Fujifilm ISO 100 on ISO 160 bodies, Olympus ISO LOW) take 1 EV off. Each such row says
so in `sample`. RAWmakase adds the stop for Highlight Tone Priority itself, reading it
from the Canon maker notes (both On and Enhanced; no Enhanced sample has been measured),
and follows Fujifilm's exposure midpoint shift (above); extended low ISO on other makes
is not handled yet.

After the table, 150 of the 156 raw.pixls.us samples that Camera Raw 18.7 and
RAWmakase both render come out within ±0.1 EV of Camera Raw (median midtone, LibRaw
master of 2026-10-02). For 16 bodies Adobe's value alone left the render more than
0.1 EV off, so their rows are `fitted` to the render instead and say Adobe's value in
`sample`: the Fujifilm X-H2 (0.40 EV darker in Camera Raw), X-T30 II (0.38 darker), X-S10,
X-S20, X-T5 and X100VI, the Canon PowerShot V1 and G5 X Mark II, the Nikon D500,
D5600, D850 and Z 7, the Olympus PEN-F, the Pentax K-70 and the Sony RX100 VII, and since the exposure shift is
followed, the Fujifilm X-H2S (0.14 darker at DR200). The Sony A7 V row is
fitted too: its Adobe DNG is 16-bit, so its white level does not compare with LibRaw's.
Camera Raw renders most of those Fujifilm bodies darker than their baseline explains,
for a reason not found yet.

Still about 1 EV off, for the shooting settings above or a decoding problem: the
Olympus E-M10 Mark III and Sony A7R IV (extended low ISO), and the Canon EOS R6 Mark III,
whose black level LibRaw reads as 0 instead of 512. The Leica Q, Ricoh GR III and Sigma fp L
raws are DNGs with their own value and render 0.11 to 0.13 EV brighter than Camera Raw.

Not compared yet: the Hasselblad, OM System, Olympus ORI and Fujifilm GFX100 II,
GFX100RF, GFX100S, GFX100S II and GFX50S II samples, for which RAWmakase does not find the installed Adobe Standard profile by name, and the Pentax KP
and 645Z and Fujifilm X-T200 and X-T50, whose default crop differs from Camera Raw's.

Three rows are not from raw.pixls.us: the X100F (read from two Lightroom DNGs, see
`macos-lightroom-validation.md`) and the A7 II and A7CR, fitted to Camera Raw 18.6
renders of private photos (8 and 4).

Not in the table: cameras whose raw is a DNG (Leica M, Q, SL and CL, Ricoh GR, Sigma
fp), which carry their own value; the Hasselblad X1D and X1D II, which LibRaw names
alike; and bodies whose only CC0 sample is an sRAW or other non-mosaic file (EOS 5D
Mark IV, 5DS R, 6D Mark II, 7D Mark II, 80D, 1D X Mark II, Nikon D810, Sony RX1R III
and A1 II).
