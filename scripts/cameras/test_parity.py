"""Checks for the reference validator and numerical comparison, without Adobe apps."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
from types import SimpleNamespace
import json
import tempfile

import numpy as np

spec = importlib.util.spec_from_file_location('parity', Path(__file__).with_name('parity.py'))
p = importlib.util.module_from_spec(spec)
spec.loader.exec_module(p)


class ParityTests(unittest.TestCase):
    def test_profile_and_look_are_both_required(self):
        tags = dict(CameraProfile='Adobe Standard', LookName='Adobe Color', ColorTemperature=5600, Tint=20)
        p.verify(tags, dict(Temperature='5600', Tint='20'), 'Adobe Standard', 'Adobe Color')
        for wrong in [dict(tags, CameraProfile='Camera Standard'), dict(tags, LookName=''),
                      dict(tags, ColorTemperature=3200), dict(tags, Tint=0)]:
            with self.subTest(tags=wrong), self.assertRaises(ValueError):
                p.verify(wrong, dict(Temperature='5600', Tint='20'), 'Adobe Standard', 'Adobe Color')

    def test_identity_and_neutral_lab_endpoints(self):
        rgb = np.linspace(0, 1, 30).reshape(2, 5, 3)
        lab = p.metrics.lab(rgb)
        np.testing.assert_allclose(p.metrics.de00(lab, lab), 0, atol=1e-12)
        np.testing.assert_allclose(p.metrics.shift(rgb, 0, 0), rgb)
        np.testing.assert_allclose(p.metrics.lab(np.array([[0., 0., 0.], [1., 1., 1.]])),
                                   [[0., 0., 0.], [100., 0., 0.]], atol=2e-5)

    def test_delta_e_is_symmetric_and_detects_color_change(self):
        a = p.metrics.lab(np.array([[.2, .3, .5], [.7, .7, .7]]))
        b = p.metrics.lab(np.array([[.5, .3, .2], [.4, .4, .4]]))
        forward = p.metrics.de00(a, b)
        self.assertTrue(np.all(forward > 10))
        np.testing.assert_allclose(forward, p.metrics.de00(b, a), atol=1e-10)

    def test_published_ciede2000_pairs_including_hue_wrap(self):
        pairs = np.loadtxt(Path(__file__).with_name('ciede2000-test-data.txt'))
        actual = p.metrics.de00(pairs[:, :3], pairs[:, 3:6])
        np.testing.assert_allclose(actual, pairs[:, 6], atol=.000051, rtol=0)
        np.testing.assert_allclose(actual, p.metrics.de00(pairs[:, 3:6], pairs[:, :3]), atol=1e-10)

    def test_small_ignored_edit_fails_even_with_a_poor_default(self):
        reference = np.full((20, 20, 3), .3)
        actual = reference + .1
        measured = p.effect_score(reference + .01, actual, reference, actual)
        self.assertFalse(measured['effect_passed'])
        measured = p.effect_score(reference + .01, actual + .01, reference, actual)
        self.assertTrue(measured['effect_passed'])

    def test_bad_framing_is_reported_without_losing_other_samples(self):
        with tempfile.TemporaryDirectory() as directory:
            out = Path(directory)
            cases = [dict(sample=i, name=name, raw=f'raw{i}', reference=f'{i}-{name}.tif',
                          xmp=f'{i}-{name}.xmp', sha256='fixed', reference_sha256='fixed',
                          xmp_sha256='fixed', look=None, profile='Adobe Standard')
                     for i in [0, 1] for name in ['default', 'whites100']]
            (out / 'manifest.json').write_text(json.dumps(dict(cases=cases, camera_raw='test')))

            def render(command, **kwargs):
                Path(command[command.index('--save-recipe') + 1]).write_text(
                    json.dumps(dict(recipe=dict(profile=dict(name='Adobe Standard')))))
                return SimpleNamespace(returncode=0, stdout='', stderr='')

            def load(path):
                # A different active-area crop changes only the candidate's first sample.
                height = 21 if path.parent.name == 'candidate' and path.name.startswith('0-') else 20
                return np.full((height, 20, 3), .4)

            with patch.object(p, 'digest', return_value='fixed'), \
                 patch.object(p.subprocess, 'run', side_effect=render), \
                 patch.object(p.metrics, 'load', side_effect=load):
                result = p.check(SimpleNamespace(out=out, rawmakase=out / 'renderer'))
            report = json.loads((out / 'report.json').read_text())['results']
            self.assertEqual(result, 1)
            self.assertEqual(len(report), 4)
            self.assertEqual([r['passed'] for r in report], [False, False, True, True])
            self.assertIn('framing differs', report[0]['error'])

    def test_invalid_case_keeps_a_report_and_later_samples(self):
        for failure in ['profile', 'hash', 'missing-recipe']:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                out = Path(directory)
                cases = [dict(sample=i, name='default', raw=f'raw{i}', reference=f'{i}-default.tif',
                              xmp=f'{i}.xmp', sha256='fixed', reference_sha256='fixed',
                              xmp_sha256='fixed', look=None, profile='Adobe Standard') for i in [0, 1]]
                (out / 'manifest.json').write_text(json.dumps(dict(cases=cases, camera_raw='test')))

                def digest(path):
                    return 'changed' if failure == 'hash' and str(path) == 'raw0' else 'fixed'

                def render(command, **kwargs):
                    path = Path(command[command.index('--save-recipe') + 1])
                    bad = path.name.startswith('0-')
                    if not (failure == 'missing-recipe' and bad):
                        name = 'Substituted' if failure == 'profile' and bad else 'Adobe Standard'
                        path.write_text(json.dumps(dict(recipe=dict(profile=dict(name=name)))))
                    return SimpleNamespace(returncode=0, stdout='', stderr='')

                with patch.object(p, 'digest', side_effect=digest), \
                     patch.object(p.subprocess, 'run', side_effect=render), \
                     patch.object(p.metrics, 'load', return_value=np.full((20, 20, 3), .4)):
                    self.assertEqual(p.check(SimpleNamespace(out=out, rawmakase=out / 'renderer')), 1)
                results = json.loads((out / 'report.json').read_text())['results']
                self.assertEqual([r['passed'] for r in results], [False, True])
                self.assertIn('error', results[0])

    def test_dimensions_must_match(self):
        with self.assertRaisesRegex(ValueError, 'framing differs'):
            p.score(np.zeros((20, 20, 3)), np.zeros((21, 20, 3)))


if __name__ == '__main__':
    unittest.main()
