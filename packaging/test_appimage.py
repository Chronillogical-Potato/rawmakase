import hashlib
import io
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from linux import appimage


class AppDirTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="appimage test ")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.payload = self.root / "payload"
        for name in appimage.REQUIRED_FILES:
            path = self.payload / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name)
        executable = self.payload / "usr/lib/rawmakase/rawmakase"
        executable.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\n')
        executable.chmod(0o755)
        self.destination = self.root / "RAWmakase.AppDir"

    def test_launcher_preserves_arguments_from_another_directory(self):
        appimage.stage_appdir(self.payload, self.destination)
        arguments = ["--version", "photo with spaces.RAW", "", "--flag"]
        result = subprocess.check_output(
            [str(self.destination / "AppRun"), *arguments], cwd="/", text=True,
        )
        self.assertEqual(result.splitlines(), arguments)

    def test_metadata_libraries_and_notices_survive_relocation(self):
        library = self.payload / "usr/lib/rawmakase/libraw.so.23"
        library.write_bytes(b"private library")
        appimage.stage_appdir(self.payload, self.destination)
        moved = self.root / "relocated AppDir"
        self.destination.rename(moved)
        self.assertEqual((moved / "AppRun").read_text(),
                         (self.payload / "usr/lib/rawmakase/rawmakase").read_text())
        self.assertEqual((moved / "rawmakase.desktop").read_text(),
                         (self.payload / "usr/share/applications/rawmakase.desktop").read_text())
        self.assertEqual((moved / "rawmakase.svg").read_text(),
                         (self.payload / "usr/share/icons/hicolor/scalable/apps/rawmakase.svg").read_text())
        self.assertEqual((moved / library.relative_to(self.payload)).read_bytes(), b"private library")
        self.assertTrue((moved / "usr/share/licenses/rawmakase/LICENSE").is_file())
        self.assertFalse((self.payload / "AppRun").exists())

    def test_incomplete_payload_is_rejected_before_staging(self):
        (self.payload / "usr/lib/rawmakase/rawmakase").unlink()
        with self.assertRaisesRegex(FileNotFoundError, "usr/lib/rawmakase/rawmakase"):
            appimage.stage_appdir(self.payload, self.destination)
        self.assertFalse(self.destination.exists())


class DownloadTests(unittest.TestCase):
    def test_verified_download_is_executable(self):
        data = b"synthetic build tool"
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "tool"
            with patch("urllib.request.urlopen", return_value=io.BytesIO(data)):
                appimage.fetch_asset("https://example.invalid/tool", hashlib.sha256(data).hexdigest(), target)
            self.assertEqual(target.read_bytes(), data)
            self.assertTrue(target.stat().st_mode & 0o111)

    def test_corrupted_download_is_not_written(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / "tool"
            with patch("urllib.request.urlopen", return_value=io.BytesIO(b"corrupt")):
                with self.assertRaisesRegex(ValueError, "SHA-256"):
                    appimage.fetch_asset("https://example.invalid/tool", "0" * 64, target)
            self.assertFalse(target.exists())


class BuildPreconditionTests(unittest.TestCase):
    def test_unsupported_architecture_is_rejected(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaisesRegex(ValueError, "Unsupported AppImage architecture"):
                appimage.build_appimage(root / "payload", root / "output", "armhf", "0.2.4")
            self.assertFalse((root / "output").exists())

    def test_existing_image_is_not_overwritten(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            target = root / "output.AppImage"
            target.write_bytes(b"existing image")
            with self.assertRaises(FileExistsError):
                appimage.build_appimage(root / "payload", target, "x86_64", "0.2.4")
            self.assertEqual(target.read_bytes(), b"existing image")


if __name__ == "__main__":
    unittest.main()
