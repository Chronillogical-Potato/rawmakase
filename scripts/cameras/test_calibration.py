"""Offline invariants for DNG calibration extraction; no Adobe app required."""
import importlib.util
import unittest
import json
import tomllib
from pathlib import Path

spec = importlib.util.spec_from_file_location('calibration', Path(__file__).with_name('check-calibration.py'))
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)


class CalibrationTests(unittest.TestCase):
    def test_committed_facts_and_supported_native_calibrations(self):
        root = Path(__file__).resolve().parents[2]
        facts = json.loads((root / 'data/camera-calibration-facts.json').read_text())
        rows = tomllib.loads((root / 'data/cameras.toml').read_text())['camera']
        public = {r['sha256']: r for r in json.loads((root / 'tests/corpus/pixls.json').read_text())['files']}
        for fact in facts['samples']:
            with self.subTest(model=fact['model']):
                if 'url' in fact:
                    self.assertEqual(public[fact['sha256']]['url'], fact['url'])
                else:
                    self.assertEqual(fact['source'], 'local')
                    self.assertEqual(len(fact['sha256']), 64)
                result = c.audit(fact['tags'], rows)
                if 'measured' in fact:
                    self.assertEqual(c.calibration(fact['tags']), fact['measured'])
                    self.assertEqual(result['measured'], fact['measured'])
                    if result['make'].casefold() == 'sony' or (result['make'], result['model']) == ('Fujifilm', 'X100F'):
                        self.assertEqual(result['status'], 'ok', result)
                    else:
                        # Other makes remain measured evidence, not a per-unit
                        # correction borrowed for every native RAW of that model.
                        self.assertEqual(result['applied'], [1.] * 3)
                        self.assertEqual(result['status'], 'ok' if fact['measured'] == [1.] * 3 else 'mismatch')
                else:
                    self.assertEqual(result['status'], 'unsupported')
                    self.assertEqual(result['reason'], fact['reason'])

    def test_sony_individual_units_use_their_own_daylight_calibration(self):
        rows = [dict(make='Sony', model='ILCE-7CR',
                     sony_daylight_reference=[2423., 1024., 1799.])]
        for wb, gains in [([2569., 1024., 1800.], [.9432, 1., .9994]),
                          ([2610., 1024., 1771.], [.9284, 1., 1.0158])]:
            tags = self.tags()
            tags.update(Model='ILCE-7CR', NativeDaylightWB=wb,
                        CameraCalibration1=[gains[0], 0, 0, 0, 1, 0, 0, 0, gains[2]],
                        CameraCalibration2=[gains[0], 0, 0, 0, 1, 0, 0, 0, gains[2]])
            result = c.audit(tags, rows)
            self.assertEqual(result['status'], 'ok', result)
            self.assertEqual(result['applied'], gains)

    def tags(self):
        return dict(Make='SONY', Model='ILCE-7M4', DNGVersion='1 7 1 0',
                    NativeDaylightWB=[2456., 1024., 1691.],
                    CameraCalibrationSig='com.adobe', ProfileCalibrationSig='com.adobe',
                    CameraCalibration1='0.9817 0 0 0 1 0 0 0 0.9687',
                    CameraCalibration2='0.9817 0 0 0 1 0 0 0 0.9687')

    def test_missing_body_is_a_detected_mismatch_not_a_make_fallback(self):
        r = c.audit(self.tags(), [])
        self.assertEqual(r['status'], 'missing')
        self.assertEqual(r['measured'], [.9817, 1., .9687])
        self.assertEqual(r['applied'], [1.] * 3)

    def test_sony_dng_alone_cannot_propose_a_fixed_model_gain(self):
        t = self.tags()
        del t['NativeDaylightWB']
        result = c.audit(t, [])
        self.assertEqual(result['status'], 'unsupported')
        self.assertNotIn('proposed', result)

    def test_signatures_must_match(self):
        t = self.tags(); t['ProfileCalibrationSig'] = 'another profile'
        self.assertEqual(c.audit(t, [])['status'], 'unsupported')

    def test_complex_or_invalid_values_are_not_silently_flattened(self):
        for key, value in [('CameraCalibration2', '1 0 0 0 1 0 0 0 1'),
                           ('CameraCalibration1', '1 .1 0 0 1 0 0 0 1'),
                           ('CameraCalibration1', 'nan 0 0 0 1 0 0 0 1'),
                           ('AnalogBalance', '1 2 1')]:
            with self.subTest(key=key, value=value):
                t = self.tags(); t[key] = value
                self.assertEqual(c.audit(t, [])['status'], 'unsupported')

    def test_missing_matrices_do_not_hide_analog_balance(self):
        t = self.tags()
        del t["CameraCalibration1"]
        del t["CameraCalibration2"]
        t["AnalogBalance"] = "1 2 1"
        self.assertEqual(c.audit(t, [])["status"], "unsupported")

    def test_unknown_identity_camera_is_not_a_false_pass(self):
        t = dict(Make='OLYMPUS CORPORATION', Model='E-M10MarkIV', DNGVersion='1 4 0 0')
        self.assertEqual(c.audit(t, [])['status'], 'missing')
        self.assertEqual(c.audit(t, [dict(make='Olympus', model='E-M10MarkIV',
                                       neutral_calibration=[.9, 1., 1.])])['status'], 'mismatch')

    def test_alias_and_case_match(self):
        r = c.audit(self.tags(), [dict(make='Sony', model='A7 IV', aliases=['ILCE-7M4'],
                                      neutral_calibration=[.9817, 1., .9687])])
        self.assertEqual(r['status'], 'ok')


if __name__ == '__main__':
    unittest.main()
