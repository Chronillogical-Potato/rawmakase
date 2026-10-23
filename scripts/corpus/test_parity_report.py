"""Chart report discovery without rendering or private data."""
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('parity_report', Path(__file__).with_name('parity-report.py'))
report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report)


class ChartReports(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.corpus = self.root / 'corpus'
        self.refs = self.corpus / 'camera-raw'
        self.charts = self.corpus / 'charts'
        self.dump = self.root / 'dump'
        for path in (self.refs, self.charts, self.dump):
            path.mkdir(parents=True)
        layout = json.loads((report.CORPUS / 'charts/layout.json').read_text())
        (self.charts / 'layout.json').write_text(json.dumps(layout))
        (self.charts / 'synthetic-d65.dng').touch()
        values = [[1000 + i * 100] * 3 for i in range(len(layout['patches']))]
        self.fixture = {'about': {'camera_raw': '18.7'}, 'cases': {'default': values}}
        for path in (self.refs, self.dump):
            (path / 'synthetic-d65.json').write_text(json.dumps(self.fixture))
        self.addCleanup(patch.stopall)
        patch.object(report, 'CORPUS', self.corpus).start()

    def test_other_reference_families_do_not_enter_chart_report(self):
        for name, value in (
            ('baseline', {'about': 'accepted chart distances', 'charts': {}}),
            ('scene-probes', {'about': 'stage probes', 'cases': {}}),
            ('scene-probes-baseline', {'cases': {}}),
            ('highlight-spots', {'cases': []}),
        ):
            (self.refs / f'{name}.json').write_text(json.dumps(value))
        controls, cases, versions = report.measure(self.dump)
        self.assertEqual(versions, {'synthetic-d65': '18.7'})
        self.assertEqual(len(cases), 1)
        self.assertEqual(cases[0]['mean'], 0)
        self.assertEqual(controls[0]['p95'], 0)

    def test_missing_chart_dump_still_fails(self):
        (self.dump / 'synthetic-d65.json').unlink()
        with self.assertRaisesRegex(SystemExit, 'synthetic-d65.json is missing'):
            report.measure(self.dump)

    def test_missing_case_is_reported(self):
        (self.dump / 'synthetic-d65.json').write_text(json.dumps({'cases': {}}))
        _, cases, _ = report.measure(self.dump)
        self.assertEqual(cases, [{'chart': 'synthetic-d65', 'case': 'default',
                                 'error': 'not rendered by RAWmakase'}])


if __name__ == '__main__':
    unittest.main()
