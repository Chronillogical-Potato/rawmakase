# Package releases

The release workflow builds stable `vMAJOR.MINOR.PATCH` tags already on `main`.
The tag must match `Cargo.toml`. All CI, package installation checks and Apple
notarization must pass before anything is published. No AUR pushes occur.

## Downloads

| Platform | Artifact | Requirements |
| --- | --- | --- |
| macOS Apple Silicon | `rawmakase-vVERSION-macos-arm64.dmg` | macOS 15+ |
| macOS Intel | `rawmakase-vVERSION-macos-x86_64.dmg` | macOS 15+ |
| Debian / Ubuntu x86_64 | `.deb` | Ubuntu 24.04+ or Debian 13+ |
| Fedora x86_64 | `.rpm` | Fedora 43+ |
| Arch x86_64 | `.pkg.tar.zst` | Current Arch system dependencies |
| Linux x86_64 archive | `rawmakase-VERSION-x86_64-linux.tar.gz` | Same system baseline as DEB/RPM; not a universal static binary |

The Mac, DEB and RPM packages contain private imaging libraries. Mac users do
not need Homebrew. The Linux tarball contains the same `/usr` layout, including
private libraries; run the extracted `usr/bin/rawmakase` or install the whole
tree under `/usr`. Do not copy just its executable. Linux still needs system
Vulkan/graphics drivers, window-system libraries and a working file-dialog portal.

The current local `packaging/macos-app.sh` remains a development helper using
Homebrew dependencies. It does not produce the standalone release app.

## Apple credentials

Set these **repository Actions secrets**:

- `APPLE_CERTIFICATE_P12`: base64 `.p12` with the Developer ID Application certificate and private key.
- `APPLE_CERTIFICATE_PASSWORD`: the `.p12` export password.
- `APPLE_SIGNING_IDENTITY`: full `Developer ID Application: Name (TEAMID)` identity.
- `APPLE_ID`: the Apple Account email used for notarization.
- `APPLE_TEAM_ID`: that developer team's ID.
- `APPLE_APP_PASSWORD`: the Apple Account app-specific password.

The release step explicitly requires all six. `native-packages` 0.7.0 signs
nested code and the app in a temporary keychain, creates and signs the DMG,
submits it with `notarytool`, waits for acceptance, and staples the ticket.
The subsequent check mounts the actual DMG, verifies its ticket, Gatekeeper
acceptance, architecture and library paths, and runs the bundled CLI.
Checksums are generated after signing and stapling.

No App Store listing or Developer ID Installer certificate is needed for DMGs.
Never put the export, its base64 contents or passwords into the repository.

## Update signatures

The app updates itself from the Apple Silicon DMG through
[fastframe-update](https://github.com/crmne/fastframe/tree/main/crates/fastframe-update).
Before downloading a package it verifies `checksums.txt.sig`, a raw Ed25519
signature over `checksums.txt`, against the public key embedded from
`assets/update-public-key.hex`. There is no unsigned fallback.

The publish job signs with `RAWMAKASE_UPDATE_SIGNING_KEY` (the PKCS#8 PEM
private key), a secret of the `release-signing` environment, which is limited
to `v*` tags and requires a maintainer's approval. Keep a backup of the private
key outside GitHub, which never shows a secret again. Installed apps trust only
the key they were built with: losing it means asking users to download the next
release by hand, and a new key must first ship alongside the old one (see
fastframe-update's notes on rotating the publisher key).

The updater looks for `rawmakase-vVERSION-macos-arm64.dmg` by name. Intel Macs
and Linux installs are shown the release page instead.

## Publishing and rehearsal

1. Finish and review the release commit, including the version files and
   `packaging/release-notes/vX.Y.Z.md` (see below).
2. Push that commit to `main`, then push its matching `vX.Y.Z` tag.
3. The Release workflow publishes after every required job succeeds.

For a rehearsal without a new publication, manually run **Release**, select
`main` as the workflow branch, enter an existing stable **tag** (such as
`v0.1.1`), and uncheck **publish** (enabled by default). The workflow validates and builds
the tag’s exact source commit using packaging tools from the selected workflow
branch, so tags created before this workflow can also be tested.
This still builds, signs, notarizes and verifies packages, then retains them as
Actions artifacts. It does not replace any existing release assets. A normal
tag push publishes automatically. Publishing to an existing release attaches
the generated packages and refreshes assets with matching names, including
`SHA256SUMS`; the reviewed notes replace the release description and unrelated
assets are preserved. AUR publication
remains disabled independently of GitHub publication.

The workflow supports stable versions only. Do not push a prerelease tag with
this workflow expecting a published release.

## Writing release notes

Release notes are authored and reviewed Markdown, not generated commit lists.
Write `packaging/release-notes/vX.Y.Z.md` before tagging. The workflow fails early
if the file is missing, empty or whitespace-only, including during rehearsals.
It publishes the file verbatim for both new and existing releases. Automated
checks enforce presence; the maintainer still reviews accuracy and writing.

For a normal tag push, notes come from the tagged commit. A manual rebuild uses
notes from the selected workflow branch while building the exact tag's source.
This allows notes to be backfilled for older releases without moving their tags.
Review those notes against that tag, not against the latest application code.

1. Inspect the changes from the previous stable tag to the release commit. Read
   relevant diffs and issue/PR context so reverted or partial work is not announced
   as a shipped feature. For the first release, describe the capabilities shipped.
2. Open with a short summary of the main changes and their practical effect.
   Focus on what users can now do, what feels better, and which problems are fixed.
3. Group items under **New**, **Improved**, **Fixed**, **Changed**, or **Removed**,
   choosing only sections that have meaningful content. Use a bold result followed
   by a concise explanation; mention UI paths or upgrade actions where useful.
   Omit routine refactors and build plumbing. Packaging changes belong when they
   change installation or supported systems. Do not turn notes into a setup guide.
4. Add clearly labelled direct download links for the shipped platforms and a
   link to the full comparison with the previous tag. State important requirements,
   breaking changes, migration steps or known limitations when relevant. Credit
   implementers and reporters accurately, with PR/issue links where available;
   include a Thanks section only when there is someone specific to acknowledge.
5. For visible features, add useful screenshots or short recordings using
   synthetic or explicitly approved content. Upload them as release assets and
   use their final asset URLs in the notes. Never commit private photographs,
   catalogs, filesystem paths or credentials. Omit media when it adds no value.
6. Review every claim against shipped code and verification results. Qualify
   measured speedups with the tested conditions; do not promise universal gains.
   Verify notes, media and downloads on the published release page.

Use `packaging/release-notes/v0.1.2.md` as an example of structure, not a source
of claims to copy into later releases. No external project's release history
is required. Keep the length proportional to the changes.

To correct only a published description, edit and commit its notes file, then
publish that exact file without rebuilding or replacing packages:

```sh
gh release edit vX.Y.Z --notes-file packaging/release-notes/vX.Y.Z.md
```

## Dependency maintenance

`release/native-deps.sh` pins LibRaw 0.22.2 and Little CMS 2.19.1 by SHA-256 and
builds them into a private prefix. Update versions and hashes together after
testing. Keep JPEG/zlib support enabled so compressed DNG decoding is retained.
The app's native wrapper retains OpenMP acceleration.
Release builds define `CMS_NO_REGISTER_KEYWORD` for compatibility between the
Little CMS headers and the wrapper's C++17 compiler.

Mac bundling follows transitive dependencies, rewrites library paths, preserves
native notices and Homebrew source/version metadata, and fails on unresolved
paths or conflicting library names. JPEG and OpenMP come from the runner's
Homebrew installation; they are recorded in the app's license directory.
Linux bundles imaging dependencies, preserves their notices, and leaves core
OS/C++/OpenMP/zlib libraries to the host. The native source archives and build
script are published alongside packages; application source is also attached.

macOS packaging uses pinned `native-packages` 0.7.0. Linux staging uses nFPM
2.47.0 directly because its private library payload and explicit runtime
dependencies are specific to RAWmakase. There are no downstream repository
credentials or automatic Homebrew/AUR publishers in this setup.

## AUR pause and updates

GitHub includes the Arch binary package and a recipe archive containing a
checksum-filled `PKGBUILD` and `.SRCINFO`. Install the binary with:

```sh
sudo pacman -U ./rawmakase-VERSION-1-x86_64.pkg.tar.zst
```

Or extract the recipe into an empty folder and run `makepkg -si` as a normal
user. No AUR account is required. When AUR pushes resume, add a separate opt-in
publisher with the maintainer's SSH key and verified host keys; it must not be
a prerequisite for GitHub releases.

These packages do not add in-app updates or an apt/dnf/pacman repository.
Users download new releases and install them over the previous version. Their
photo library/settings remain outside package-owned directories.

## Validation

Run `actionlint .github/workflows/release.yml`, `shellcheck packaging/release/*.sh`
and `native-packages validate` before changing the workflow. A Mac bundle can
be tested without Apple credentials using the bundler and `dmg.rb`; public
releases always require signing.

Release CI installs/removes DEB/RPM packages in clean Ubuntu 24.04, Debian 13,
Fedora 43 and Fedora 44 containers; checks linked and dynamically loaded GUI
libraries; and verifies removal preserves user data. Arch builds its exact
tagged source recipe, installs it and runs the CLI. Existing CI covers Rust
tests and dependency audits.

CLI and container checks do not validate a real desktop, Metal/Vulkan driver,
or photo development. Before announcing the first packaged release, test a
downloaded DMG on a Mac without Homebrew and the Linux packages on real desktops:
open a RAW file/folder, preview and edit, import a catalog/profile, export JPEG
and TIFF, and upgrade while preserving settings. Test both Mac architectures
and Linux Wayland/X11. Do not claim older OS compatibility without testing the
executable and every bundled library against that baseline.
