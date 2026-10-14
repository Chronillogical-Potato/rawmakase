"""Synthetic, CPU-only tests of the photo gate; no Adobe or private fixtures."""
import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import types
import unittest
from unittest.mock import patch

import numpy as np
import gate

spec = importlib.util.spec_from_file_location('dataset_gate', Path(__file__).with_name('dataset-gate.py'))
dataset_gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(dataset_gate)


def metrics(a, b):
    return {k: v for k, v in gate.photo_metrics(a, b).items() if not k.startswith('L_')}


def scores(a, b, n=3):
    rows = [dict(photo=i, sha256=f'{st}-{i}', name=name, stratum=st,
                 **metrics(a, a if name == 'default' else b))
            for st in ('low', 'mid', 'high') for i in range(n)
            for name in ('default', 'highlights-100')]
    return dict(schema=2, metric_policy=2, set='highlights-validation',
                settings=['default', 'highlights-100'],
                rendered_by=dict(policy=gate.RENDER_POLICY, references='frozen', profiles='library'),
                photos=rows, bands={})


class GateTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.image = np.full((256, 256, 3), .2)
        cls.image[120:133, 120:133] = 1.
        cls.base = scores(cls.image, cls.image)

    def accept(self, candidate, baseline=None, rule='regression'):
        doc = lambda d: types.SimpleNamespace(read_text=lambda: json.dumps(d))
        with contextlib.redirect_stdout(io.StringIO()):
            return gate.accept(types.SimpleNamespace(candidate=doc(candidate),
                baseline=doc(baseline or self.base), exclude=None, rule=rule))

    def test_identical_complete_scores_pass(self):
        self.assertEqual(self.accept(self.base), 0)

    def test_local_highlight_failure_cannot_hide_in_median(self):
        broken = self.image.copy()
        broken[120:133, 120:133] = .25
        candidate = scores(self.image, broken)
        self.assertEqual(candidate['photos'][1]['median_de00'], 0)
        for rule in ('regression', 'switch'):
            self.assertEqual(self.accept(candidate, rule=rule), 1)

    def test_absent_stratum_fails_even_on_both_sides(self):
        incomplete = copy.deepcopy(self.base)
        incomplete['photos'] = [r for r in incomplete['photos'] if r['stratum'] != 'low']
        self.assertEqual(self.accept(incomplete, incomplete), 1)

    def test_old_scores_require_rescoring(self):
        old = copy.deepcopy(self.base)
        old.pop('metric_policy')
        self.assertEqual(self.accept(old, old), 1)

    def test_declared_setting_cannot_disappear(self):
        incomplete = copy.deepcopy(self.base)
        incomplete['photos'] = [r for r in incomplete['photos'] if r['name'] == 'default']
        self.assertEqual(self.accept(incomplete, incomplete), 1)

    def test_absolute_parity_rejects_unchanged_bad_baseline(self):
        bad = scores(self.image, np.full_like(self.image, .5))
        for row in bad['photos']:
            row.update(metrics(self.image, np.full_like(self.image, .5)))
        self.assertEqual(self.accept(bad, bad), 0)
        self.assertEqual(self.accept(bad, bad, rule='parity'), 1)

    def test_absolute_parity_accepts_exact_reference(self):
        self.assertEqual(self.accept(self.base, rule='parity'), 0)

    def test_nonfinite_pixels_cannot_be_discarded(self):
        broken = self.image.copy()
        broken[20, 20] = np.nan
        self.assertEqual(gate.photo_metrics(self.image, broken)['valid'], 0)

    def test_identically_unscored_photo_is_coverage_failure(self):
        incomplete = copy.deepcopy(self.base)
        incomplete['photos'].append(dict(photo=9, sha256='unscored', name='default',
                                         stratum='low', valid=0., error='framing differs'))
        self.assertEqual(self.accept(incomplete, incomplete), 1)

    def test_missing_or_nan_regional_measurements_fail(self):
        for broken in (None, [float('nan')]):
            candidate = copy.deepcopy(self.base)
            candidate['photos'][0]['regions_de00'] = broken
            self.assertEqual(self.accept(candidate), 1)

    def test_existing_defect_elsewhere_cannot_hide_new_defect(self):
        baseline = copy.deepcopy(self.base)
        baseline['photos'][1]['regions_de00'][0] = 50.
        candidate = copy.deepcopy(baseline)
        candidate['photos'][1]['regions_de00'][-1] = 25.
        self.assertEqual(self.accept(candidate, baseline), 1)

    def test_per_photo_clipping_cannot_be_diluted_by_other_photos(self):
        candidate = copy.deepcopy(self.base)
        candidate['photos'][1]['clipping'] = .01
        self.assertEqual(self.accept(candidate), 1)

    def test_resume_refuses_old_scores_without_modifying_them(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            refs = root / 'camera-raw-test/highlights-validation'
            score = root / 'out/scores/camera-raw-test/highlights-validation.json'
            score.parent.mkdir(parents=True)
            old = json.dumps(dict(schema=1, photos=[]))
            score.write_text(old)
            _, ok, _ = dataset_gate.run_set(types.SimpleNamespace(), root, refs, root / 'out')
            self.assertFalse(ok)
            self.assertEqual(score.read_text(), old)

    def test_small_shadow_and_colour_defects_fail(self):
        for value, wrong in ((.01, .2), (.4, [1., 0., 0.])):
            reference = self.image.copy()
            reference[120:133, 120:133] = value
            candidate = reference.copy()
            candidate[120:133, 120:133] = wrong
            self.assertEqual(self.accept(scores(reference, candidate), scores(reference, reference)), 1)

    def test_score_writes_versioned_regional_metrics(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            refs, renders = root / 'refs', root / 'renders'
            refs.mkdir(); renders.mkdir()
            (refs / 'reference.tif').write_bytes(b'fixture')
            frozen = dict(set='highlights-validation', photos=[dict(sha256='photo', stratum='low')],
                          cases=[dict(photo=0, name='default', reference='reference.tif',
                                      reference_sha256=gate.digest(refs / 'reference.tif'))])
            (refs / 'references.json').write_text(json.dumps(frozen))
            (renders / 'rendered-by.json').write_text(json.dumps(dict(
                references=gate.digest(refs / 'references.json'), policy=gate.RENDER_POLICY)))
            np.save(renders / 'reference.npy', self.image)
            with patch.object(gate, 'reference_srgb', return_value=self.image), \
                 patch.object(gate, 'reduce', side_effect=lambda a: a), \
                 contextlib.redirect_stdout(io.StringIO()):
                gate.score(types.SimpleNamespace(refs=refs, renders=renders, out=root / 'scores.json'))
            result = json.loads((root / 'scores.json').read_text())
            self.assertEqual(result['metric_policy'], gate.METRIC_POLICY)
            self.assertEqual(result['settings'], ['default'])
            self.assertEqual(max(result['photos'][0]['regions_de00']), 0)

    def test_dataset_wrapper_fails_inconclusive(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'references/camera-raw-test/highlights-validation').mkdir(parents=True)
            score = root / 'run/scores/camera-raw-test/highlights-validation.json'
            score.parent.mkdir(parents=True)
            score.write_text(json.dumps(scores(self.image, self.image, n=2)))
            with contextlib.redirect_stdout(io.StringIO()):
                result = dataset_gate.accept(types.SimpleNamespace(dataset=str(root),
                    baseline=root / 'run', candidate=root / 'run', split='validation',
                    exclude=None, rule='regression', verbose=False))
            self.assertEqual(result, 1)


if __name__ == '__main__':
    unittest.main()
