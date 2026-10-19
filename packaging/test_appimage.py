"""The AppImage native-packages builds: configuration and the release check."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
VERIFY = ROOT / "packaging/linux/verify-appimage.sh"
LICENSE = ROOT / "packaging/linux/AppImage-runtime-LICENSE"
# LICENSE from type2-runtime 20251108, the runtime native-packages 0.10.0 pins.
RUNTIME_LICENSE_DIGEST = "aa154fc9070614bbe7921f89db11efd1dba7a1f3a41685958110e2230f9c0ca1"
TOOL_VERSION = "0.10.0"


@unittest.skipUnless(shutil.which("ruby"), "needs Ruby to read native-packages.yaml")
class ConfigurationTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        output = subprocess.check_output(
            ["ruby", "-ryaml", "-rjson", "-e", "puts JSON.dump(YAML.load_file(ARGV[0]))",
             str(ROOT / "native-packages.yaml")], text=True)
        cls.config = json.loads(output)

    def test_runtime_licence_matches_the_pinned_runtime(self):
        # A new native-packages may append a different runtime: check its
        # licence (AppImage::RUNTIME in appimage.rb), then update both.
        self.assertEqual(self.config["tool"]["version"], TOOL_VERSION)
        self.assertEqual(hashlib.sha256(LICENSE.read_bytes()).hexdigest(), RUNTIME_LICENSE_DIGEST)

    def test_each_linux_target_builds_an_appimage_with_the_runtime_licence(self):
        linux = {name: target for name, target in self.config["targets"].items()
                 if target["platform"] == "linux"}
        self.assertEqual(sorted(linux), ["linux-amd64", "linux-arm64"])
        for name, target in linux.items():
            with self.subTest(name):
                self.assertIn("appimage", target["formats"])
                licences = [item for item in target["nfpm"]["contents"]
                            if item["src"] == "packaging/linux/AppImage-runtime-LICENSE"]
                # nFPM skips contents for another packager, so the DEB and RPM
                # do not carry it.
                self.assertEqual(licences, [{
                    "src": "packaging/linux/AppImage-runtime-LICENSE",
                    "dst": "/usr/share/licenses/rawmakase/AppImage-runtime-LICENSE",
                    "packager": "appimage",
                }])


class VerifyScriptTests(unittest.TestCase):
    """verify-appimage.sh against a stand-in that extracts like the runtime."""

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="appimage test ")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.contents = self.root / "contents"
        files = {
            "rawmakase.desktop": "[Desktop Entry]\n",
            "rawmakase.svg": "<svg/>",
            "usr/share/applications/rawmakase.desktop": "[Desktop Entry]\n",
            "usr/share/licenses/rawmakase/LICENSE": "MIT",
            "usr/share/licenses/rawmakase/AppImage-runtime-LICENSE": "MIT",
            "usr/bin/rawmakase": '#!/bin/sh\nexec "$(dirname "$(readlink -f "$0")")/../lib/rawmakase/rawmakase" "$@"\n',
            "usr/lib/rawmakase/rawmakase": '#!/bin/sh\necho "rawmakase 1.2.3"\n',
        }
        for name, text in files.items():
            path = self.contents / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
            path.chmod(0o755)
        (self.contents / "AppRun").symlink_to("usr/bin/rawmakase")
        self.image = self.root / "rawmakase-1.2.3-x86_64.AppImage"
        self.image.write_text(
            "#!/bin/sh\n"
            'if [ "$1" = --appimage-extract ]; then cp -a "$CONTENTS" squashfs-root; exit; fi\n'
            'exec "$CONTENTS/AppRun" "$@"\n')

    def verify(self, *arguments, image=None):
        environment = dict(os.environ, CONTENTS=str(self.contents))
        return subprocess.run(["bash", str(VERIFY), str(image or self.image), *arguments],
                              env=environment, capture_output=True, text=True)

    def test_complete_appimage_passes_and_runs(self):
        result = self.verify("1.2.3", "--run")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Verified rawmakase-1.2.3-x86_64.AppImage", result.stdout)

    def test_runtime_licence_is_required(self):
        (self.contents / "usr/share/licenses/rawmakase/AppImage-runtime-LICENSE").unlink()
        result = self.verify("1.2.3")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("AppImage-runtime-LICENSE", result.stderr)

    def test_apprun_must_be_the_launcher(self):
        (self.contents / "AppRun").unlink()
        (self.contents / "AppRun").symlink_to("usr/lib/rawmakase/rawmakase")
        self.assertNotEqual(self.verify("1.2.3").returncode, 0)

    def test_another_version_fails_when_run(self):
        renamed = self.image.with_name("rawmakase-1.2.4-x86_64.AppImage")
        self.image.rename(renamed)
        self.assertEqual(self.verify("1.2.4", image=renamed).returncode, 0)
        self.assertNotEqual(self.verify("1.2.4", "--run", image=renamed).returncode, 0)

    def test_renamed_download_is_rejected(self):
        renamed = self.image.with_name("RAWmakase-1.2.3-amd64.AppImage")
        self.image.rename(renamed)
        result = self.verify("1.2.3", image=renamed)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Unexpected AppImage name", result.stderr)


if __name__ == "__main__":
    unittest.main()
