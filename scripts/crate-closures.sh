#!/usr/bin/env bash
# Checks that the extracted crates keep out what they are extracted to avoid:
# the app crate, the GPU and GUI stacks, and the native imaging libraries. Cargo
# already forbids a cycle back to the app; nothing else stops a leaf crate
# from gaining wgpu, egui or a LibRaw binding as a dependency.
#
#     scripts/crate-closures.sh
#
# The catalog's bundled SQLite (libsqlite3-sys) is intended and allowed.
set -euo pipefail

forbidden='^(rawmakase|wgpu.*|naga|egui.*|eframe|epaint|winit|lcms2.*|libraw.*|fastframe-fonts) '
status=0
for crate in rawmakase-model rawmakase-interop rawmakase-catalog rawmakase-protocol; do
    # Every platform's dependencies, not only this machine's.
    found=$(cargo tree --locked -p "$crate" --all-features --target all -e normal,build \
        --prefix none --format '{p}' | sort -u | grep -E "$forbidden" || true)
    if [ -n "$found" ]; then
        echo "$crate must not depend on:" >&2
        echo "$found" | sed 's/^/    /' >&2
        status=1
    fi
done
[ "$status" -eq 0 ] && echo "The extracted crates depend on no app, GPU, GUI or native imaging crate."
exit "$status"
