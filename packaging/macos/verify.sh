#!/usr/bin/env bash
set -euo pipefail
dmg="${1:?usage: verify.sh FILE.dmg ARCH}"
arch="${2:?expected architecture}"
temporary=$(mktemp -d)
mount="$temporary/mount"
mkdir "$mount"
trap 'hdiutil detach "$mount" >/dev/null 2>&1 || true; rm -rf "$temporary"' EXIT
xcrun stapler validate "$dmg"
hdiutil attach "$dmg" -readonly -nobrowse -mountpoint "$mount"
app="$mount/RAWmakase.app"
test -s "$app/Contents/Resources/Assets.car"
test -s "$app/Contents/Resources/rawmakase.icns"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconName' "$app/Contents/Info.plist")" = RAWmakase
codesign --verify --strict --deep "$app"
spctl --assess --type execute --verbose=2 "$app"
for binary in "$app/Contents/MacOS/rawmakase" "$app/Contents/Frameworks/"*.dylib; do
  lipo "$binary" -verify_arch "$arch"
  # Only system paths and bundle-relative paths may survive packaging.
  if otool -L "$binary" | tail -n +2 | grep -vE '^[[:space:]]+(/usr/lib/|/System/Library/|@loader_path/|@executable_path/|@rpath/)'; then
    echo "External dependency in $binary" >&2
    exit 1
  fi
done
env -u DYLD_LIBRARY_PATH -u DYLD_FALLBACK_LIBRARY_PATH "$app/Contents/MacOS/rawmakase" --version
