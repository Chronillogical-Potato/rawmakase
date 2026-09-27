# Working on RAWmakase

## Before every push

Run what CI runs (`.github/workflows/ci.yml`), with CI's toolchain (current
stable Rust, not only the 1.95 MSRV: newer clippy has more lints). Push only
when all of it passes:

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

A `v*` tag builds and publishes the release, so run these before tagging too.

## Toolchain

- The default Homebrew `cargo` is too old. Use rustup's, with the newest
  installed toolchain for the checks above (1.98.1 as of 0.1.4):
  `PATH=$HOME/.cargo/bin:$PATH cargo +1.98.1-aarch64-apple-darwin …`.
  Run `rustup update stable` when CI's Rust moves on.
- The Mac is often loaded by other work: avoid repeated full builds and long
  test runs; run the tests you touched while iterating, the full set before
  pushing.

## Releases

Copy the previous release commit (`git log -1 v0.1.3`): bump the version in
`Cargo.toml`, `Cargo.lock`, `packaging/Info.plist` and both `PKGBUILD`s,
commit "Release x.y.z", tag `vx.y.z` with an annotated tag, push `main`, then
push the tag. Check the Release run and the assets on the GitHub release page.

## Shared tree

Several sessions work in this folder at once: commit by explicit path, format
only files you changed, and never use `git stash` or whole-tree operations.
Never commit `review.md`, Adobe profiles, RAW files or private paths.
