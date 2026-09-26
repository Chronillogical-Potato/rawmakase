# Package releases

The release workflow builds stable `vMAJOR.MINOR.PATCH` tags already on `main`.
The tag must match `Cargo.toml`. All CI, package installation checks and Apple
notarization must pass before anything is published. No AUR pushes occur.

## Downloads

| Platform | Artifact | Requirements |
| --- | --- | --- |
| macOS Apple Silicon | `rawmakase-VERSION-macos-arm64.dmg` | macOS 15+ |
| macOS Intel | `rawmakase-VERSION-macos-x86_64.dmg` | macOS 15+ |
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

## Publishing and rehearsal

1. Finish and review the release commit, including the Cargo version/lockfile.
2. Push that commit to `main`, then push its matching `vX.Y.Z` tag.
3. The Release workflow publishes after every required job succeeds.

For a rehearsal without a new publication, manually run **Release**, select an
`main` as the workflow branch, enter an existing stable **tag** (such as
`v0.1.1`), and leave **publish** unchecked. The workflow validates and builds
the tag’s exact source commit using packaging tools from the selected workflow
branch, so tags created before this workflow can also be tested.
This still builds, signs, notarizes and verifies packages, then retains them as
Actions artifacts. It does not replace any existing release assets. A normal
tag push publishes automatically. Re-running publication for an existing
release fails rather than silently overwriting its downloads.

The workflow supports stable versions only. Do not push a prerelease tag with
this workflow expecting a published release.

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

GitHub includes the Arch binary package, a checksum-filled `PKGBUILD`, and an
archive containing `PKGBUILD` and `.SRCINFO`. Install the binary with:

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
