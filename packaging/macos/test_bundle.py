import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from bundle import install_icon_assets


class IconAssetsTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.resources = self.root / "Resources"
        self.resources.mkdir()
        self.catalog = self.root / "Assets.car"

    def test_precompiled_catalog_does_not_run_actool(self):
        self.catalog.write_bytes(b"compiled icon fixture")
        with patch("bundle.subprocess.run", side_effect=AssertionError("actool must not run")):
            self.assertTrue(install_icon_assets(self.root, self.resources, "15.0", self.catalog))
        self.assertEqual((self.resources / "Assets.car").read_bytes(), self.catalog.read_bytes())

    def test_missing_or_empty_supplied_catalog_never_falls_back(self):
        for empty in (False, True):
            with self.subTest(empty=empty):
                if empty:
                    self.catalog.touch()
                with patch("bundle.subprocess.run", side_effect=AssertionError("actool must not run")):
                    with self.assertRaisesRegex(SystemExit, "Missing or empty compiled icon"):
                        install_icon_assets(self.root, self.resources, "15.0", self.catalog)
                self.assertFalse((self.resources / "Assets.car").exists())

    def test_release_compilation_failure_is_fatal(self):
        failed = subprocess.CompletedProcess([], 1, stderr=b"asset runtime crashed")
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "true"}), patch("bundle.subprocess.run", return_value=failed):
            with self.assertRaisesRegex(SystemExit, "asset runtime crashed"):
                install_icon_assets(self.root, self.resources, "15.0")

    def test_success_without_catalog_is_rejected_in_ci(self):
        succeeded = subprocess.CompletedProcess([], 0, stderr=b"")
        with patch.dict(os.environ, {"GITHUB_ACTIONS": "true"}), patch("bundle.subprocess.run", return_value=succeeded):
            with self.assertRaisesRegex(SystemExit, "actool cannot compile"):
                install_icon_assets(self.root, self.resources, "15.0")


if __name__ == "__main__":
    unittest.main()
