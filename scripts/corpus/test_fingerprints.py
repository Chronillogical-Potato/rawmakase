"""Synthetic tests of the decode fingerprint comparison; no RAWs needed."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('fingerprints', Path(__file__).with_name('dataset-fingerprints.py'))
fingerprints = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fingerprints)


class Compare(unittest.TestCase):
    old = {'version': 2, 'decoder': 'a', 'header': {'metadata': {'wb': [2, 1, 1.5], 'make': 'X'}},
           'sensor': {'white': 4095}}

    def test_nested_fields_compare_by_dotted_name(self):
        new = {'version': 2, 'decoder': 'b', 'header': {'metadata': {'wb': [2, 1, 1.25], 'make': 'X'}},
               'sensor': {'white': 4095, 'cfa': 'RG/gB'}}
        self.assertEqual(fingerprints.compare(self.old, new), [
            ('header.metadata.wb', [2, 1, 1.5], [2, 1, 1.25]),
            ('sensor.cfa', None, 'RG/gB'),
        ])


spec = importlib.util.spec_from_file_location('develop', Path(__file__).with_name('dataset-develop.py'))
develop = importlib.util.module_from_spec(spec)
spec.loader.exec_module(develop)


class DevelopCheck(unittest.TestCase):
    def test_a_baseline_file_missing_from_the_dataset_is_a_problem_unless_a_manifest_selects(self):
        import types
        with tempfile.TemporaryDirectory() as d:
            dataset = Path(d)
            sample = {'sha256': 'a' * 64, 'source': 's', 'make': 'X', 'model': 'Y', 'ext': 'raw', 'path': 'p.raw',
                      'license': 'x'}
            fingerprints.write_baseline(dataset / 'baselines/develop/s.json', {},
                                        {h: {'sha256': h, 'full': {}} for h in ('a' * 64, 'b' * 64)})
            for manifest, status in ((None, 1), (Path('m.json'), 0)):
                args = types.SimpleNamespace(dataset=dataset, manifest=manifest, public=False)
                result = develop.check(args, {'a' * 64: sample}, {'a' * 64: {'sha256': 'a' * 64, 'full': {}}}, [])
                self.assertEqual(result, status)


spec = importlib.util.spec_from_file_location('half', Path(__file__).with_name('dataset-half.py'))
half = importlib.util.module_from_spec(spec)
spec.loader.exec_module(half)


class HalfCheck(unittest.TestCase):
    reference = {'ratio': 0.4, 'de_mean': 0.4, 'de_p95': 1.2, 'detail': 1.3}

    def test_each_gate_fails_on_its_own(self):
        # Above the absolute limits but within LibRaw's values plus 0.1.
        ok = {'halved': True, 'xtrans': False, 'de_mean': 0.49, 'de_p95': 1.29, 'detail': 1.39,
              'seconds': 0.459, 'full_seconds': 1.0, 'reference': self.reference}
        self.assertEqual(half.failures(ok), [])
        for change, gate in (({'de_mean': 0.51}, 'colour mean'), ({'de_p95': 1.31}, 'colour p95'),
                             ({'detail': 1.41}, 'detail'), ({'detail': 0.59}, 'detail'),
                             ({'seconds': 0.461}, 'time'), ({'error': 'x'}, 'error')):
            self.assertEqual(half.failures({**ok, **change}), [gate], change)
        self.assertEqual(half.failures({**ok, 'detail': 0.61}), [])

    def test_without_a_reference_the_absolute_limits_hold(self):
        ok = {'halved': True, 'de_mean': 0.29, 'de_p95': 0.99, 'detail': 0.81, 'seconds': 1.11,
              'full_seconds': 1.0}
        self.assertEqual(half.failures(ok), [])
        for change, gate in (({'de_mean': 0.31}, 'colour mean'), ({'de_p95': 1.01}, 'colour p95'),
                             ({'detail': 1.26}, 'detail'), ({'detail': 0.79}, 'detail'),
                             ({'seconds': 1.13}, 'time')):
            self.assertEqual(half.failures({**ok, **change}), [gate], change)

    def test_time_slack_x_trans_and_small_raws(self):
        # A fast file: 1.1 × 0.4 × 50 ms + 20 ms = 42 ms.
        fast = {'halved': True, 'full_seconds': 0.05, 'reference': {'ratio': 0.4}}
        self.assertEqual(half.failures({**fast, 'seconds': 0.041}), [])
        self.assertEqual(half.failures({**fast, 'seconds': 0.043}), ['time'])
        # X-Trans drafts take their own full develop as the reference.
        xtrans = {'halved': True, 'xtrans': True, 'full_seconds': 1.0, 'reference': {'ratio': 0.27}}
        self.assertEqual(half.failures({**xtrans, 'seconds': 1.1}), [])
        self.assertEqual(half.failures({**xtrans, 'seconds': 1.2}), ['time'])
        # Small RAWs, not halved, skip the image gates but not the time: they
        # develop at full size in both modes, so the reference is 1.
        small = {'halved': False, 'de_p95': 9, 'full_seconds': 1.0, 'reference': {'ratio': 0.3}}
        self.assertEqual(half.failures({**small, 'seconds': 1.1}), [])
        self.assertEqual(half.failures({**small, 'seconds': 1.2}), ['time'])
        # Untimed lines (an error before the full develop) skip the time gate.
        self.assertEqual(half.failures({'halved': False, 'de_p95': 9}), [])

    def test_recorded_references_round_trip_by_sha256(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'baselines/half/own.json'
            half.write_references(path, {'reason': 'test'}, {
                'a' * 64: {'sha256': 'a' * 64, 'libraw_half_seconds': 0.4, 'ratio': 0.4, 'de_mean': 0.2},
                'b' * 64: {'sha256': 'b' * 64, 'error': 'x'}})
            sample = {'sha256': 'a' * 64, 'source': 'own', 'license': 'x'}
            self.assertEqual(half.reference_path(Path(temporary), sample), path)
            self.assertEqual(half.load_references(Path(temporary), [sample]),
                             {'a' * 64: {'ratio': 0.4, 'de_mean': 0.2}})

    def test_the_report_counts_and_names_failing_files(self):
        samples = {h: {'sha256': h, 'make': 'X', 'model': 'Y', 'ext': 'raw', 'source': 'own', 'license': 'x'}
                   for h in ('a' * 64, 'b' * 64)}
        results = [{'sha256': 'a' * 64, 'halved': True, 'de_mean': 0.1, 'de_p95': 0.5, 'detail': 1.0},
                   {'sha256': 'b' * 64, 'halved': True, 'de_mean': 0.5, 'de_p95': 2.0, 'detail': 1.0}]
        report = half.summarize(results, True, samples)
        self.assertEqual((report['files'], report['passing']), (2, 1))
        self.assertEqual(report['failing'], {'colour mean': 1, 'colour p95': 1})
        # A private file is named by its hash alone with --public.
        self.assertEqual(report['failing files']['X .raw'][0][0], 'b' * 12)


if __name__ == '__main__':
    unittest.main()
