#!/bin/sh
# Local app bundle: native libraries remain installed through Homebrew.
set -eu
cd "$(dirname "$0")/.."
# Prefer a native rustup compiler over an older Homebrew Rust or Rosetta toolchain.
rustup_bin="${HOME}/.cargo/bin/rustup"
if [ -x "$rustup_bin" ]; then
    architecture=$(uname -m)
    case "$architecture" in arm64) architecture=aarch64 ;; esac
    toolchain="1.98.1-$architecture-apple-darwin"
    if compiler=$("$rustup_bin" which --toolchain "$toolchain" rustc 2>/dev/null); then
        PATH="$(dirname "$compiler"):$PATH"
        export PATH
    else
        echo "Install the native compiler: $rustup_bin toolchain install $toolchain --profile minimal" >&2
        exit 1
    fi
fi
profile=${1:-release}
case "$profile" in
    release) cargo build --release --locked ;;
    debug) cargo build --locked ;;
    *) echo 'Usage: packaging/macos-app.sh [release|debug]' >&2; exit 2 ;;
esac
bundle="target/$profile/RAWmakase.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources/licenses"
cp "target/$profile/rawmakase" "$bundle/Contents/MacOS/rawmakase"
cp packaging/Info.plist "$bundle/Contents/Info.plist"
cp LICENSE "$bundle/Contents/Resources/licenses/RAWmakase.txt"
cp licenses/Adobe-DNG-SDK.txt "$bundle/Contents/Resources/licenses/Adobe-DNG-SDK.txt"
echo "$bundle"
