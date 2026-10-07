#!/usr/bin/env bash
# Checks that the extracted crates keep out what they are extracted to avoid:
# the app crate, the GPU and GUI stacks, and the native imaging libraries. Cargo
# already forbids a cycle back to the app; nothing else stops a leaf crate
# from gaining wgpu, egui or a LibRaw binding as a dependency.
#
#     scripts/crate-closures.sh
#
# The catalog's bundled SQLite (libsqlite3-sys) is intended and allowed. The
# renderer has wgpu for its GPU preview port. LibRaw and Little CMS are linked
# by rawmakase-native's own build script, not a crate, so the crates above it
# are kept from depending on rawmakase-native instead. Export reads Inter and
# the installed-font directories from fastframe-fonts, which brings egui (but
# no window or GPU backend) along.
set -euo pipefail

gui='egui.*|eframe|epaint|winit|fastframe-fonts'
native='rawmakase-native|lcms2.*|libraw.*'
leaf="rawmakase|wgpu.*|naga|$gui|$native"
status=0
check() {
    local crate=$1 forbidden="^($2) "
    # Every platform's dependencies, not only this machine's.
    found=$(cargo tree --locked -p "$crate" --all-features --target all -e normal,build \
        --prefix none --format '{p}' | sort -u | grep -E "$forbidden" || true)
    if [ -n "$found" ]; then
        echo "$crate must not depend on:" >&2
        echo "$found" | sed 's/^/    /' >&2
        status=1
    fi
}
for crate in rawmakase-model rawmakase-interop rawmakase-catalog rawmakase-protocol; do
    check "$crate" "$leaf"
done
check rawmakase-engine "rawmakase|rawmakase-catalog|rawmakase-export|$gui|$native"
check rawmakase-native "rawmakase|rawmakase-catalog|rawmakase-export|$gui"
check rawmakase-export "rawmakase|eframe|winit|egui-wgpu|egui-winit|egui_extras|lcms2.*|libraw.*"
[ "$status" -eq 0 ] && echo "The extracted crates depend on no app, GPU, GUI or native imaging crate beyond their own."
exit "$status"
