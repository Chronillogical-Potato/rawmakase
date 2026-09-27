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

1. Bump the version in `Cargo.toml`, `Cargo.lock`, `packaging/Info.plist` and
   both `packaging/*/PKGBUILD` files (see the previous `Release x.y.z` commit).
2. Commit as `Release x.y.z`, with a short summary of what changed since the
   last release.
3. Create an annotated tag `vx.y.z`, push `main`, then push the tag.
4. Check that the Release workflow passes and the assets are on the GitHub
   release page.

## Commits

- Commit only the files you changed, by explicit path.
- Never commit Adobe profiles, RAW files or paths from your own machine.
