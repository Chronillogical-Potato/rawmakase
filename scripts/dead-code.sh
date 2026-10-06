#!/usr/bin/env bash
# Lists library items the rawmakase binary never reaches.
#
# The library's public items are exported, so the compiler never calls them
# unused. This builds a scratch copy in which the binary declares the library's
# modules itself; there nothing is exported, and rustc's dead_code lint names
# every item no path from main reaches. Items used only by unit tests,
# integration tests or examples are reported too: decide each one (delete it,
# keep it for the tests, or move the test) rather than deleting the list.
#
#     scripts/dead-code.sh            # report; always exits 0
#
# Run it from the repository root. It builds into target/dead-code.
set -euo pipefail

root=$(pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT

rsync -a --exclude target Cargo.toml Cargo.lock build.rs src native data assets crates tools "$scratch"/

# The library's module declarations, private, followed by the binary with its
# `rawmakase::` paths rewritten to `crate::`. Imports of a module itself
# (`develop::{self, ..}`, a bare `raw`) would now clash with the declarations.
python3 - "$scratch/src/main.rs" <<'PY'
import re, sys
library = open("src/lib.rs").read()
modules = re.findall(r"^(?:pub )?mod (\w+);", library, re.M)
declarations = re.findall(r"^(?:#\[cfg\(.*\)\]\n)?(?:pub )?mod \w+;", library, re.M)
main = open("src/main.rs").read()
inner = re.findall(r"^#!\[.*\]\n", main, re.M)
body = re.sub(r"^#!\[.*\]\n", "", main, flags=re.M)
body = re.sub(r"\brawmakase::", "crate::", body)
def without_modules(group):
    for name in modules:
        group = re.sub(rf"\b{name}::\{{self, ", f"{name}::{{", group)
        group = re.sub(rf"(?<=[{{,\s]){name},\s*", "", group)
    return group
body = re.sub(r"use crate::\{.*?\};", lambda m: without_modules(m.group(0)), body, flags=re.S)
declared = "\n".join(d.replace("pub mod", "mod") for d in declarations)
open(sys.argv[1], "w").write("".join(inner) + declared + "\n" + body)
PY
rm "$scratch/src/lib.rs"

CARGO_TARGET_DIR="$root/target/dead-code" cargo check --manifest-path "$scratch/Cargo.toml" \
    --bin rawmakase --message-format short 2>&1 |
    grep -E "(^|: )error|warning: (unused|.* never (used|read|constructed))" |
    sed -e "s|$scratch/||" |
    sort -u || true
