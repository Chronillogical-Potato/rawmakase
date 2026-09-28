#!/usr/bin/env python3
"""Stage private imaging libraries without replacing system libraries."""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess


def run(*args):
    return subprocess.check_output(args, text=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("native_prefix", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    stage = args.output.resolve()
    stage.mkdir(parents=True, exist_ok=False)
    private = stage / "usr/lib/rawmakase"
    private.mkdir(parents=True)
    executable = private / "rawmakase"
    shutil.copy2(args.binary, executable)
    executable.chmod(0o755)
    licenses = stage / "usr/share/licenses/rawmakase"
    shutil.copytree(args.native_prefix / "notices", licenses)
    for source, target in [(root / "LICENSE", licenses / "LICENSE"),
                           (root / "licenses/Adobe-DNG-SDK.txt", licenses / "Adobe-DNG-SDK.txt"),
                           (root / "licenses/Inter-OFL.txt", licenses / "Inter-OFL.txt"),
                           (root / "licenses/Lucide-ISC.txt", licenses / "Lucide-ISC.txt"),
                           (root / "packaging/applications/rawmakase.desktop", stage / "usr/share/applications/rawmakase.desktop"),
                           (root / "packaging/icons/rawmakase.svg", stage / "usr/share/icons/hicolor/scalable/apps/rawmakase.svg")]:
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
    # These are supplied by the supported distribution, including its GPU stack.
    host = re.compile(r"^(lib(c|m|dl|pthread|rt|resolv|util|gcc_s|stdc\+\+|gomp|z|"
                      r"vulkan|xkbcommon|xkbcommon-x11|wayland-client|wayland-cursor|wayland-egl|"
                      r"X11|X11-xcb|xcb|Xcursor|Xi|Xrandr|EGL|GL)\.so[.\d]*|ld-linux.*)$")
    listing = run("ldd", str(args.binary))
    if "not found" in listing:
        raise RuntimeError(listing)
    records = []
    for line in listing.splitlines():
        match = re.match(r"\s*(\S+) => (/\S+)", line)
        if not match:
            continue
        name, source = match.groups()
        if host.fullmatch(name):
            continue
        destination = private / name
        shutil.copy2(source, destination)
        destination.chmod(0o755)
        if not Path(source).resolve().is_relative_to(args.native_prefix.resolve()):
            # Preserve Ubuntu's copyright/source reference for bundled JPEG etc.
            # ldd may name /lib paths on merged-/usr systems while dpkg records
            # their canonical /usr/lib paths (or the reverse).
            owner = None
            for candidate in dict.fromkeys([str(Path(source).resolve()), source]):
                query = subprocess.run(["dpkg-query", "-S", candidate], text=True, capture_output=True)
                if query.returncode == 0:
                    owner = query.stdout.split(": ", 1)[0].split(":", 1)[0]
                    break
            if not owner:
                raise RuntimeError(f"Cannot find distribution notices for {source}")
            copyright_file = Path("/usr/share/doc") / owner / "copyright"
            shutil.copy2(copyright_file, licenses / (owner + "-copyright"))
            records.append({"library": name, "package": run("dpkg-query", "-W", "-f=${Package} ${Version}", owner)})
    (licenses / "ubuntu-libraries.json").write_text(json.dumps(records, indent=2) + "\n")
    for path in private.iterdir():
        subprocess.run(["patchelf", "--set-rpath", "$ORIGIN", str(path)], check=True)
    launcher = stage / "usr/bin/rawmakase"
    launcher.parent.mkdir(parents=True)
    launcher.write_text('#!/bin/sh\nset -eu\nbase=$(CDPATH= cd -- "$(dirname -- "$0")/../lib/rawmakase" && pwd)\nexec "$base/rawmakase" "$@"\n')
    launcher.chmod(0o755)
    # Verify without the build-time library path.
    import os
    env = dict(os.environ)
    env.pop("LD_LIBRARY_PATH", None)
    subprocess.run([str(launcher), "--version"], env=env, check=True)
    print(stage)


if __name__ == "__main__":
    main()
