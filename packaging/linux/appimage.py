#!/usr/bin/env python3
"""Package the existing Linux payload as an AppImage; host requirements are unchanged."""
import argparse
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import urllib.request


TOOL_VERSION = "1.9.1"
RUNTIME_VERSION = "20251108"
RUNTIME_LICENSE_DIGEST = "aa154fc9070614bbe7921f89db11efd1dba7a1f3a41685958110e2230f9c0ca1"
# SHA-256 digests from the versioned upstream GitHub release assets.
ASSETS = {
    "x86_64": (
        "ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0",
        "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d",
    ),
    "aarch64": (
        "f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158",
        "00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444",
    ),
}
REQUIRED_FILES = (
    "usr/bin/rawmakase",
    "usr/lib/rawmakase/rawmakase",
    "usr/share/applications/rawmakase.desktop",
    "usr/share/icons/hicolor/scalable/apps/rawmakase.svg",
    "usr/share/licenses/rawmakase/LICENSE",
)


def stage_appdir(payload, destination):
    """Copy a staged Linux payload, preserving private libraries and notices."""
    for name in REQUIRED_FILES:
        if not (payload / name).is_file():
            raise FileNotFoundError(f"Missing Linux payload file: {name}")
    shutil.copytree(payload, destination)
    # The executable finds its private libraries and ONNX Runtime from its
    # resolved path, so AppRun can link to it without a wrapper script.
    for source, name in (
        ("usr/lib/rawmakase/rawmakase", "AppRun"),
        ("usr/share/applications/rawmakase.desktop", "rawmakase.desktop"),
        ("usr/share/icons/hicolor/scalable/apps/rawmakase.svg", "rawmakase.svg"),
    ):
        (destination / name).symlink_to(source)


def fetch_asset(url, digest, destination, mode=0o755):
    """Verify a downloaded build input before writing it."""
    with urllib.request.urlopen(url, timeout=300) as response:
        data = response.read()
    found = hashlib.sha256(data).hexdigest()
    if found != digest:
        raise ValueError(f"{url}: SHA-256 is {found}, expected {digest}")
    destination.write_bytes(data)
    destination.chmod(mode)


def build_appimage(payload, output, arch, version):
    if arch not in ASSETS:
        raise ValueError(f"Unsupported AppImage architecture: {arch}")
    if output.exists():
        raise FileExistsError(output)
    output = output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    # Both the packaging tool and the finished image run without a FUSE mount.
    environment = dict(os.environ, APPIMAGE_EXTRACT_AND_RUN="1", ARCH=arch)
    environment.pop("LD_LIBRARY_PATH", None)
    with tempfile.TemporaryDirectory(prefix="rawmakase-appimage-") as temporary:
        root = Path(temporary)
        appdir = root / "RAWmakase.AppDir"
        stage_appdir(payload, appdir)
        tool = root / "appimagetool.AppImage"
        runtime = root / "runtime"
        tool_digest, runtime_digest = ASSETS[arch]
        fetch_asset(
            f"https://github.com/AppImage/appimagetool/releases/download/{TOOL_VERSION}/"
            f"appimagetool-{arch}.AppImage", tool_digest, tool,
        )
        fetch_asset(
            f"https://github.com/AppImage/type2-runtime/releases/download/{RUNTIME_VERSION}/"
            f"runtime-{arch}", runtime_digest, runtime,
        )
        fetch_asset(
            f"https://raw.githubusercontent.com/AppImage/type2-runtime/{RUNTIME_VERSION}/LICENSE",
            RUNTIME_LICENSE_DIGEST,
            appdir / "usr/share/licenses/rawmakase/AppImage-runtime-LICENSE",
            mode=0o644,
        )
        subprocess.run(
            [str(tool), "--runtime-file", str(runtime), str(appdir), str(output)],
            env=environment, check=True,
        )
        output.chmod(0o755)
        result = subprocess.check_output(
            [str(output), "--version"], env=environment, text=True,
        ).strip()
        expected = f"rawmakase {version}"
        if result != expected:
            raise RuntimeError(f"AppImage reports {result!r}, expected {expected!r}")
        print(result)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("payload", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("arch", choices=sorted(ASSETS))
    parser.add_argument("version")
    args = parser.parse_args()
    build_appimage(args.payload, args.output, args.arch, args.version)


if __name__ == "__main__":
    main()
