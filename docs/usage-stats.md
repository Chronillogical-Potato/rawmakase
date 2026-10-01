# Usage stats

RAWmakase can send one small anonymous report a week, so its author can see
how many installations are in use and on which platforms. It is off until you
agree. The public totals are at [stats.rawmakase.com](https://stats.rawmakase.com),
and the design discussion is [issue #32](https://github.com/pch/rawmakase/issues/32).

## Turning it on and off

After first-run setup, a card asks once whether to share. It shows the exact
report. **Share** turns sharing on; **Don't Share** leaves it off. Until you
answer, nothing is sent. The answer is saved in `usage-stats.json` in the app
data folder; if that file is missing or can't be read, the question is asked
again and nothing is sent. Change the answer at any time in **Preferences > General >
Usage stats**, where the report is shown again.

Setting `DO_NOT_TRACK=1` or `RAWMAKASE_NO_TELEMETRY=1` turns sharing off
whatever the saved answer, and hides the question. Packagers can leave the
feature out entirely by building with `cargo build --no-default-features`,
which removes the question, the Preferences row and all reporting.

## What is sent

```json
{"schema": 1, "version": "0.1.10", "os": "macos", "arch": "aarch64",
 "channel": "macos-dmg", "os_release": "macos-15", "distro": "none",
 "display": "none", "gpu": "metal"}
```

| Field | Meaning |
| --- | --- |
| `version` | The RAWmakase version. |
| `os`, `arch` | macOS, Linux or Windows; x86_64 or aarch64. |
| `os_release` | The major release: `macos-15`, `windows-11`; just `linux` on Linux. |
| `channel` | How this copy was installed, as the updater detects it: `macos-dmg`, `windows-installer`, `windows-zip`, `linux-tarball`, or the package manager that owns it (`deb`, `rpm`, `arch-package`, `homebrew`, `flatpak`, `snap`, `nix`, `cargo`). Otherwise `unknown`. |
| `distro` | On Linux, the distribution family from os-release: arch, debian, ubuntu, fedora, opensuse, nixos or other. `none` elsewhere. |
| `display` | On Linux, `wayland` or `x11`. `none` elsewhere. |
| `gpu` | The graphics backend the window uses (`metal`, `vulkan`, `dx12`, `gl`), or `cpu` for a software adapter. |

Nothing else is sent: no identifier of any kind, and no file names, paths,
photos, EXIF, edit settings, catalog details, hostnames or user names.

## When

At most once per ISO calendar week (UTC): 30 seconds after launch while
sharing is on, or right away when you turn it on. `usage-stats.json` also
records the last week reported, before sending, so the app never sends a week
twice; the file itself is never sent anywhere. Failures are silent. If no connection could be
made, the next launch tries again; otherwise that week's report is skipped.
Reporting never delays startup, editing, export or quitting.

## What happens to it

The [stats service](https://github.com/pch/rawmakase/blob/main/stats/README.md) adds each field to its own weekly
tally and discards the report. It never reads or stores your IP address.
Cloudflare, which hosts it, sees the connection's address as it does for any
website, and its rate-limiting rule logs the addresses it blocks. The public
page shows only coarse totals for completed weeks, merging groups smaller
than 10. Tallies are deleted after 24 months.

Without an identifier, a past report can't be found or removed; turning
sharing off stops future ones. Reports aren't authenticated, so the totals
are estimates of opted-in installations, not of people.
