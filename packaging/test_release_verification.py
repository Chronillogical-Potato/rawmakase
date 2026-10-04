"""Exercise the release workflow's actual shell with synthetic downloads."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import textwrap
import unittest


WORKFLOW = Path(__file__).resolve().parents[1] / ".github/workflows/release.yml"


@unittest.skipUnless(shutil.which("sha256sum"), "requires GNU sha256sum")
class PublishedReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="release-verification-")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.published = self.root / "published"
        self.workspace = self.root / "workspace"
        self.expected = self.workspace / "expected"
        self.bin = self.root / "bin"
        for directory in (self.published, self.expected, self.bin):
            directory.mkdir(parents=True)
        (self.published / "rawmakase.deb").write_bytes(b"synthetic package")
        (self.published / "rawmakase.rb").write_bytes(b"synthetic cask")
        (self.published / "checksums.txt").write_bytes(b"synthetic manifest")
        # Signature authenticity is checked before upload by sign-release.
        # This job checks that those exact signature bytes were published.
        (self.published / "checksums.txt.sig").write_bytes(b"synthetic signature")
        self.record_upload()
        gh = self.bin / "gh"
        gh.write_text(textwrap.dedent("""\
            #!/usr/bin/env python3
            import os
            from pathlib import Path
            import shutil
            import sys
            args = sys.argv[1:]
            source = Path(os.environ['FAKE_RELEASE'])
            if args[:2] == ['release', 'download']:
                name = args[args.index('--pattern') + 1]
                path = source / name
                if not path.exists():
                    sys.exit(1)
                shutil.copy2(path, Path(args[args.index('--dir') + 1]))
            else:
                sys.exit(2)
            """))
        gh.chmod(0o755)

    def checksum_lines(self, names):
        return "".join(
            hashlib.sha256((self.published / name).read_bytes()).hexdigest()
            + "  " + name + "\n" for name in sorted(names)
        )

    def record_upload(self):
        names = sorted(path.name for path in self.published.iterdir())
        (self.expected / "expected-checksums.txt").write_text(self.checksum_lines(names))

    def verify(self, success, diagnostic=""):
        job = WORKFLOW.read_text().split("\n  verify:\n", 1)[1].split("\n  website:\n", 1)[0]
        script = textwrap.dedent(job.split("        run: |\n", 1)[1])
        env = dict(os.environ, PATH=str(self.bin) + os.pathsep + os.environ["PATH"],
                   FAKE_RELEASE=str(self.published), RUNNER_TEMP=str(self.root),
                   GITHUB_WORKSPACE=str(self.workspace), RELEASE_TAG="v0.0.0",
                   GITHUB_REPOSITORY="synthetic/repository",
                   GITHUB_STEP_SUMMARY=str(self.root / "summary"))
        result = subprocess.run(["bash", "-c", script], cwd=self.workspace,
                                env=env, text=True, capture_output=True)
        output = result.stdout + result.stderr
        self.assertEqual(result.returncode == 0, success, output)
        self.assertIn(diagnostic, output)

    def test_complete_release_ignores_unrelated_assets(self):
        (self.published / "screenshot.png").write_bytes(b"synthetic screenshot")
        (self.published / "old-package.zip").write_bytes(b"older package")
        self.verify(True)
        self.assertIn("every expected release asset matches", (self.root / "summary").read_text())

    def test_missing_package_fails(self):
        (self.published / "rawmakase.deb").unlink()
        self.verify(False, "Downloading rawmakase.deb")

    def test_changed_package_fails(self):
        (self.published / "rawmakase.deb").write_bytes(b"truncated or stale")
        self.verify(False, "rawmakase.deb: FAILED")

    def test_changed_signature_fails(self):
        (self.published / "checksums.txt.sig").write_bytes(b"corrupt signature")
        self.verify(False, "checksums.txt.sig: FAILED")


if __name__ == "__main__":
    unittest.main()
