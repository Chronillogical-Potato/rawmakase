from pathlib import Path
import subprocess
import tempfile
import unittest

from linux import bundle


class LauncherTests(unittest.TestCase):
    """usr/bin/rawmakase must find usr/lib/rawmakase however it is started."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="launcher test ")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.payload = self.root / "payload"
        executable = self.payload / "usr/lib/rawmakase/rawmakase"
        executable.parent.mkdir(parents=True)
        # Reports where it ran from, then its arguments.
        executable.write_text('#!/bin/sh\nprintf "%s\\n" "$0" "$@"\n')
        executable.chmod(0o755)
        self.launcher = self.payload / "usr/bin/rawmakase"
        bundle.write_launcher(self.launcher)
        self.arguments = ["--version", "photo with spaces.RAW", "", "--flag"]

    def run_launcher(self, path):
        output = subprocess.check_output([str(path), *self.arguments], cwd="/", text=True)
        ran, *arguments = output.split("\n")[:-1]
        self.assertEqual(Path(ran).resolve(), (self.payload / "usr/lib/rawmakase/rawmakase").resolve())
        self.assertEqual(arguments, self.arguments)

    def test_direct_invocation_preserves_arguments(self):
        self.run_launcher(self.launcher)

    def test_appimage_apprun_symlink_at_the_root(self):
        # native-packages links AppRun to the desktop entry's Exec target.
        app_run = self.payload / "AppRun"
        app_run.symlink_to("usr/bin/rawmakase")
        self.run_launcher(app_run)

    def test_symlink_from_another_directory(self):
        # For example ~/.local/bin/rawmakase pointing into an extracted archive.
        link = self.root / "bin/rawmakase"
        link.parent.mkdir()
        link.symlink_to(self.launcher)
        self.run_launcher(link)


if __name__ == "__main__":
    unittest.main()
