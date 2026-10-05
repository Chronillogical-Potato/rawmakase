# RAWmakase usage stats

A Cloudflare Worker that counts the opt-in weekly reports described in
[issue #32](https://github.com/pch/rawmakase/issues/32) and publishes coarse
totals at `stats.rawmakase.com`. It is separate from the app and deploys on
its own.

## What it does

- `POST /v1/report`, over HTTPS only, takes one JSON object:

  ```json
  {"schema": 1, "version": "0.1.10", "os": "linux", "arch": "x86_64",
   "channel": "arch-package", "os_release": "linux", "distro": "arch",
   "display": "wayland", "gpu": "vulkan"}
  ```

  It must come with `User-Agent: RAWmakase/<version>`, the header the app's
  update check already sends, matching `version`. That only keeps out
  scanners and generic scripts; anyone reading this source can send it.
  Every field is required and values come from fixed lists in
  [src/report.ts](src/report.ts): `os_release` is `macos-NN`, `windows-10`,
  `windows-11` or `linux`, matching `os`; `distro` (os-release `ID`, else the
  first listed `ID_LIKE`, else `other`) and `display` are `none` off Linux;
  `channel` is how the copy was installed, as the app's updater detects it
  (the release download, or the package manager that owns it). `gpu` is the wgpu backend RAWmakase draws with, or `cpu` for a software
  adapter. Unknown fields are refused and bodies over 512 bytes are refused
  unread.
- `version` must be a `vX.Y.Z` tag of this repository, read from GitHub's git
  endpoint and cached for an hour ([src/versions.ts](src/versions.ts)). Each
  list read is kept in D1's `releases` table, which answers while GitHub
  can't be reached; with neither, every version is refused.
- `REPORTS` (in `wrangler.jsonc`, or the dashboard's variables) is the off
  switch: anything but `on` refuses reports with `503`. Once
  `DAILY_REPORT_BUDGET` (5,000) reports are counted in a UTC day, the rest
  get `503` too, which keeps a flood inside D1's free limits. The app treats
  both like a failed delivery.
- A valid report adds 1 to a weekly tally per question
  ([src/dimensions.ts](src/dimensions.ts)): `(week, os, arch)`,
  `(week, version)`, `(week, channel)`, `(week, os_release)`, `(week, gpu)`,
  and for Linux reports `(week, distro)` and `(week, display)`. It also adds 1
  to a per-day count with no other field. All in one D1 transaction. `week` is the ISO week of receipt in UTC, from
  the server's clock. The report itself is not stored.
- `GET /` is the public page and `GET /stats.json` the same data. They are
  cached at Cloudflare until the next midnight UTC, when their data next
  changes, keyed by path and the deployed version, so the database is read
  about once a day per Cloudflare location. Browsers keep them for five
  minutes, so a deploy reaches them soon. Both show
  the current week's total through the end of yesterday, which updates daily,
  and completed weeks, at most 26. Breakdowns are only published for completed
  weeks, since daily snapshots of them could be subtracted. Groups under 10
  are merged into "other", which always merges at least two groups and
  reaches 10 itself, and weeks with fewer than 10 reports publish no numbers
  ([src/publish.ts](src/publish.ts)). Showing any group therefore takes at
  least 20 reports that week; until then the page leaves the breakdown out
  rather than show a lone "other".
- The daily cron deletes tallies older than 24 months. D1 Time Travel, whose
  window Cloudflare sets by plan, is the only backup.

The Worker never reads the client address, Workers logs are off in
[wrangler.jsonc](wrangler.jsonc), and nothing is written to `console`.
Cloudflare still sees each connection's IP address as the host, and a
rate-limiting rule on the `rawmakase.com` zone blocks addresses that send
reports in bursts; the `workers.dev` address is off so nothing bypasses it.
There is no easy way to prevent abuse of the report endpoint. Reports are not
authenticated, and an open-source client can't make them so: a shared secret
would ship in every binary, and proof of work is cheap on a GPU. The checks
stop junk and floods from one address, not someone determined, so the numbers
are estimates.

## Develop

Node 24. From this directory:

```sh
npm ci
npm run types   # worker-configuration.d.ts, from wrangler.jsonc
npm run check   # tsc
npm test        # vitest in the Workers runtime, with a local D1
npm run dev     # http://localhost:8787
```

To send a report to the local server:

```sh
curl -i localhost:8787/v1/report -H 'content-type: application/json' \
  -H 'user-agent: RAWmakase/0.1.10' \
  -d '{"schema":1,"version":"0.1.10","os":"linux","arch":"x86_64","channel":"unknown","os_release":"linux","distro":"arch","display":"wayland","gpu":"vulkan"}'
```

## Deploy

The [Stats workflow](../.github/workflows/stats.yml) tests every pull request
that touches `stats/`. On `main` it applies migrations and deploys, once the
repository has `CLOUDFLARE_API_TOKEN` (Workers Scripts and D1 edit) and
`CLOUDFLARE_ACCOUNT_ID` secrets. Without them it skips the deploy.

To deploy by hand, from a machine logged in with `npx wrangler login`, run
`npm run deploy`: it applies new `migrations/` and deploys the Worker. The
database was created once with `npx wrangler d1 create rawmakase-stats
--location weur`; its id is in `wrangler.jsonc`.

The Worker is served only at <https://stats.rawmakase.com>, a Workers custom
domain in the `rawmakase.com` Cloudflare zone. The zone's rate-limiting rule
(Security → WAF → Rate limiting rules) matches `URI Path equals /v1/report`
on that host and blocks an IP for 10 seconds after 5 requests in 10 seconds.

Schema changes go in a new numbered file in `migrations/`; never edit an
applied one.
