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

Fujifilm rows hold for DR100 photos; DR200 and DR400 photos get no baseline yet.

The baseline is stored per edit, apart from the Exposure slider, so changing the table
changes photos opened afterwards. An existing edit keeps its value; Develop's
"Use camera exposure baseline" button applies the current one.

### How good the fallback is

Predicting each of the 59 bodies in the table on 2026-10-05 from the other rows
(leave-one-out) misses Camera Raw by:

| Rule | Mean error | 90th percentile | Worst |
|---|---|---|---|
| No baseline (before the table) | 0.26 EV | 0.50 EV | 0.60 EV |
| Median of all rows | 0.17 EV | 0.40 EV | 0.65 EV |
| Median of the same make | 0.10 EV | 0.20 EV | 0.65 EV |

The worst case is the Panasonic G9 (+0.6 among Panasonic bodies near 0). No rule from
the raw's own metadata did better: within a make, neither white level nor black level
orders the baselines. The same-make median is the fallback.

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

Adobe DNG Converter was not installed when the table was made, so every row except
the X100F (read from two Lightroom DNGs, see `macos-lightroom-validation.md`) is
fitted to Camera Raw renders with Adobe Standard: Camera Raw 18.7 on one CC0 sample
per camera from [raw.pixls.us](https://raw.pixls.us) (the samples `scripts/corpus/pixls.py` downloads), and Camera Raw 18.6 on private
photos for the A7 II (8) and A7CR (4).

After the table, every listed camera's unedited render is within ±0.1 EV of Camera Raw
on its sample. Photos with Canon Highlight Tone Priority or Fujifilm DR200/DR400, and
the EOS R6 Mark III, Sony A7R IV and RX100 VII samples, still differ by about 1 EV for
reasons beyond the baseline.
