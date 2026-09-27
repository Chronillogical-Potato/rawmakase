# Working on RAWmakase

## Before every push

Run the checks CI runs (`.github/workflows/ci.yml`) and push only when they
pass. Use the current stable Rust, as CI does; newer clippy releases add lints.

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

A `v*` tag builds and publishes a release, so the same applies before tagging.

## Releases

Follow [the release guide](packaging/RELEASING.md#writing-release-notes).
Write reviewed notes in `packaging/release-notes/vX.Y.Z.md` before tagging.
Stable releases require this file; never leave generated or placeholder notes.

Notes explain what changed since the previous stable release and why it matters
for someone using RAWmakase. Inspect the actual tagged commit range and relevant
code, not just commit titles. Start with a short plain-language summary, then use
only the sections that apply: **New**, **Improved**, **Fixed**, **Changed**, or
**Removed**. Lead each bullet with a bold user-visible result. Do not list internal
refactors, CI repairs or implementation details unless they affect users. Never
include changes still on main or in the working tree but absent from the tag.

Include direct download links, relevant compatibility or upgrade notes, known
limitations, and a full-changelog link. Credit contributors and reporters when
supported by the history; omit empty sections and boilerplate thanks. Include
screenshots for substantial visual changes when they help explain the change,
using only synthetic or explicitly approved content. Upload media as release
assets, never commit private photos or screenshots. Verify every linked asset.
Do not invent benchmarks, compatibility claims, contributor credits or fixes.
No reference project or previous release style is required.

1. Bump the version in `Cargo.toml`, `Cargo.lock`, `packaging/Info.plist` and
   both `packaging/*/PKGBUILD` files (see the previous `Release x.y.z` commit).
2. Write and review the notes, run the required checks, and commit as
   `Release x.y.z`, with a short summary of what changed.
3. Push `main`, verify the release commit is on `origin/main`, then create
   and push its annotated tag `vx.y.z`.
4. Wait for every build, signing/notarization, installation check and publication.
   Upload any linked media and verify the published text, images and downloads.
   A pushed tag alone does not complete a release. GitHub publication is enabled;
   AUR publication remains disabled.

## Commits

- Commit only the files you changed, by explicit path.
- Never commit Adobe profiles, RAW files or paths from your own machine.
