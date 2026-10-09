import hashlib
import io
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import onnxruntime

RECIPES = [Path(__file__).resolve().parent / "arch" / name / "PKGBUILD"
           for name in ("rawmakase", "rawmakase-git")]

def archive(files):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as tf:
        for name, content in files.items():
            info = tarfile.TarInfo("./" + name)
            info.size = len(content)
            tf.addfile(info, io.BytesIO(content))
    return buffer.getvalue()


class FetchTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def fetch(self, data, digest):
        asset = ("macos", "arm64")
        name, _, member, target = onnxruntime.ASSETS[asset]
        prefix = name.rsplit(".", 1)[0]
        assets = {asset: (name, digest, member, target)}
        response = io.BytesIO(data)
        with patch.dict(onnxruntime.ASSETS, assets), patch("onnxruntime.urllib.request.urlopen", return_value=response):
            return onnxruntime.fetch("macos", "arm64", self.root / "out", self.root / "notices"), prefix

    def test_the_library_and_notices_are_taken_from_a_checked_archive(self):
        prefix = "onnxruntime-osx-arm64-" + onnxruntime.VERSION
        files = {f"{prefix}/lib/libonnxruntime.{onnxruntime.VERSION}.dylib": b"library",
                 f"{prefix}/LICENSE": b"MIT", f"{prefix}/ThirdPartyNotices.txt": b"notices",
                 f"{prefix}/include/other.h": b"header"}
        data = archive(files)
        library, _ = self.fetch(data, hashlib.sha256(data).hexdigest())
        self.assertEqual(library.name, "libonnxruntime.dylib")
        self.assertEqual(library.read_bytes(), b"library")
        self.assertEqual((self.root / "notices/onnxruntime-LICENSE").read_bytes(), b"MIT")
        self.assertEqual(sorted(p.name for p in (self.root / "out").iterdir()), ["libonnxruntime.dylib"])

    def test_a_different_archive_is_refused_before_anything_is_written(self):
        with self.assertRaisesRegex(SystemExit, "SHA-256"):
            self.fetch(b"not the release", "0" * 64)
        self.assertFalse((self.root / "out").exists())

    def test_every_supported_target_pins_one_release(self):
        for (platform, arch), (name, digest, member, target) in onnxruntime.ASSETS.items():
            self.assertIn(onnxruntime.VERSION, name)
            self.assertEqual(len(digest), 64, name)
            self.assertTrue(member.startswith("lib/"), name)


@unittest.skipUnless(shutil.which("bash"), "requires bash")
class ArchRecipeTests(unittest.TestCase):
    """makepkg downloads the runtime for the Arch recipes, so they pin it themselves."""

    def recipe(self, recipe, arch, script):
        return subprocess.run(["bash", "-c", f'source "$1"; CARCH={arch}; {script}', "-", recipe],
                              check=True, capture_output=True, text=True).stdout

    def test_arch_recipes_pin_the_same_runtime_per_architecture(self):
        for recipe in RECIPES:
            for arch in ("x86_64", "aarch64"):
                name, digest, _, _ = onnxruntime.ASSETS[("linux", arch)]
                with self.subTest(recipe=recipe.parent.name, arch=arch):
                    pinned = self.recipe(recipe, arch, f'printf "%s\\n" "${{source_{arch}[@]}}" '
                                                       f'"${{sha256sums_{arch}[@]}}"')
                    self.assertEqual(pinned.split(), [onnxruntime.BASE + name, digest])

    def test_arch_recipes_install_the_runtime_where_the_app_looks(self):
        for recipe in RECIPES:
            for arch in ("x86_64", "aarch64"):
                name, _, member, _ = onnxruntime.ASSETS[("linux", arch)]
                with self.subTest(recipe=recipe.parent.name, arch=arch), \
                        tempfile.TemporaryDirectory() as root:
                    src, pkg = Path(root, "src"), Path(root, "pkg")
                    # makepkg extracts the archive, and the app's checkout, into srcdir.
                    extracted = src / name.rsplit(".", 1)[0]
                    for path, content in ((member, "library"), ("LICENSE", "MIT"),
                                          ("ThirdPartyNotices.txt", "notices")):
                        (extracted / path).parent.mkdir(parents=True, exist_ok=True)
                        (extracted / path).write_text(content)
                    (src / "rawmakase").mkdir()
                    (src / "rawmakase-0").mkdir()
                    self.recipe(recipe, arch, f'pkgver=0; srcdir={src}; pkgdir={pkg}; '
                                              'make() { :; }; cd "$srcdir"; package')
                    licenses = pkg / "usr/share/licenses" / recipe.parent.name
                    self.assertEqual((pkg / "usr/lib/rawmakase/libonnxruntime.so").read_text(), "library")
                    self.assertEqual((licenses / "onnxruntime-LICENSE").read_text(), "MIT")
                    self.assertEqual((licenses / "onnxruntime-ThirdPartyNotices.txt").read_text(), "notices")


if __name__ == "__main__":
    unittest.main()
