"""Reference resumption must keep the original photo/case/look inputs."""
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('cube_looks', Path(__file__).with_name('cube-looks.py'))
cube_looks = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cube_looks)


class ResumeTests(unittest.TestCase):
    def test_resume_rejects_replaced_photo_or_changed_case(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            looks = root / 'looks'
            looks.mkdir()
            (looks / 'test.xmp').write_text(
                '<crs:Name><rdf:Alt><rdf:li>Test</rdf:li></rdf:Alt></crs:Name>')
            photo = root / 'photo.raw'
            photo.write_bytes(b'photo A')
            photos = root / 'photos.json'
            def write_photos():
                photos.write_text(json.dumps([dict(raw=str(photo), sha256=cube_looks.digest(photo))]))
            write_photos()
            cases = root / 'cases.json'
            cases.write_text(json.dumps({'100': {'amount': 1}}))
            args = SimpleNamespace(photos=photos, cases=cases, looks=looks, only=None,
                                   out=root / 'out', set='test', chunk=1)
            def render(jobs, **kwargs):
                for job in jobs:
                    out = Path(job['out'])
                    out.write_bytes(photo.read_bytes())
                    out.with_suffix('.xmp').write_text('synthetic sidecar')
                return []
            tags = json.dumps([dict(CameraProfile='Adobe Standard', LookName='Test', LookAmount=1)]).encode()
            with patch.object(cube_looks.camera_raw, 'render', side_effect=render) as renderer, \
                 patch.object(cube_looks.camera_raw, 'version', return_value='test'), \
                 patch.object(cube_looks, 'compress'), \
                 patch.object(cube_looks.shutil, 'disk_usage', return_value=SimpleNamespace(free=10**12)), \
                 patch.object(cube_looks.subprocess, 'check_output', return_value=tags):
                cube_looks.reference(args)
                cube_looks.reference(args)
                self.assertEqual(renderer.call_count, 1)
                original = (args.out / args.set / 'references.json').read_bytes()
                photo.write_bytes(b'photo B')
                write_photos()
                with self.assertRaisesRegex(ValueError, 'inputs'):
                    cube_looks.reference(args)
                self.assertEqual((args.out / args.set / 'references.json').read_bytes(), original)
                photo.write_bytes(b'photo A')
                write_photos()
                cases.write_text(json.dumps({'100': {'amount': 0.5}}))
                with self.assertRaisesRegex(ValueError, 'inputs'):
                    cube_looks.reference(args)
                self.assertEqual(renderer.call_count, 1)


if __name__ == '__main__':
    unittest.main()
