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
cp licenses/Inter-OFL.txt "$bundle/Contents/Resources/licenses/Inter-OFL.txt"
cp licenses/Lucide-ISC.txt "$bundle/Contents/Resources/licenses/Lucide-ISC.txt"
# The app icon, rendered as packaging/release/bundle-macos.py does.
if command -v rsvg-convert >/dev/null 2>&1 && command -v iconutil >/dev/null 2>&1; then
    iconset="target/$profile/rawmakase.iconset"
    rm -rf "$iconset"
    mkdir "$iconset"
    for size in 16 32 128 256 512; do
        rsvg-convert -w "$size" -h "$size" -o "$iconset/icon_${size}x${size}.png" packaging/rawmakase.svg
        rsvg-convert -w $((size * 2)) -h $((size * 2)) -o "$iconset/icon_${size}x${size}@2x.png" packaging/rawmakase.svg
    done
    iconutil -c icns "$iconset" -o "$bundle/Contents/Resources/rawmakase.icns"
    rm -rf "$iconset"
    plist="$bundle/Contents/Info.plist"
    /usr/libexec/PlistBuddy -c 'Set :CFBundleIconFile rawmakase.icns' "$plist" 2>/dev/null ||
        /usr/libexec/PlistBuddy -c 'Add :CFBundleIconFile string rawmakase.icns' "$plist"
else
    echo 'brew install librsvg for the app icon' >&2
fi
echo "$bundle"
