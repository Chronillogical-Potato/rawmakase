"""Offline invariants for DNG calibration extraction; no Adobe app required."""
import collections
import importlib.util
import unittest
import json
import tomllib
from pathlib import Path

spec = importlib.util.spec_from_file_location('calibration', Path(__file__).with_name('check-calibration.py'))
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)

ROOT = Path(__file__).resolve().parents[2]
# Models whose units differ in a way no stored preset explains (see docs/camera-parity.md).
UNRESOLVED = {('Canon', 'EOS M3'), ('Olympus', 'E-M5MarkIII')}


def committed():
    facts = json.loads((ROOT / 'data/camera-calibration-facts.json').read_text())['samples']
    rows = tomllib.loads((ROOT / 'data/cameras.toml').read_text())['camera']
    return facts, rows


class CalibrationTests(unittest.TestCase):
    def test_committed_facts_and_supported_native_calibrations(self):
        facts, rows = committed()
        public = {r['sha256']: r for r in json.loads((ROOT / 'tests/corpus/pixls.json').read_text())['files']}
        for fact in facts:
            with self.subTest(model=fact['model'], sha=fact['sha256'][:12]):
                if 'url' in fact:
                    if fact['sha256'] in public:
                        self.assertEqual(public[fact['sha256']]['url'], fact['url'])
                    self.assertTrue(fact['url'].startswith('https://raw.pixls.us/'))
                else:
                    self.assertEqual(fact['source'], 'local')
                    self.assertEqual(len(fact['sha256']), 64)
                result = c.audit(fact['tags'], rows)
                if 'measured' not in fact:
                    self.assertEqual(result['status'], 'unsupported')
                    self.assertEqual(result['reason'], fact['reason'])
                    continue
                self.assertEqual(c.calibration(fact['tags']), fact['measured'])
                self.assertEqual(result['measured'], fact['measured'])
                row = c.table_row(rows, result['make'], result['model'])
                if row and 'unit_calibration' in row:
                    self.assertEqual(result['status'], 'ok', result)
                elif row:
                    # Without a verified reference, no other unit's correction is borrowed.
                    self.assertEqual(result['applied'], [1.] * 3)
                    self.assertEqual(result['status'], 'ok' if fact['measured'] == [1.] * 3 else 'mismatch')
                else:
                    self.assertEqual(result['status'], 'missing')

    def test_units_of_one_model_share_one_reference(self):
        facts, rows = committed()
        models = collections.defaultdict(list)
        for fact in facts:
            if 'measured' in fact:
                models[c.identify(fact['tags'])].append((fact['measured'], fact['tags'].get('NativeWhiteBalance', {})))
        explained = 0
        for model, samples in models.items():
            with self.subTest(model=model):
                found = c.fits(samples)
                if len({tuple(m) for m, _ in samples}) > 1:
                    self.assertEqual(not found, model in UNRESOLVED, found)
                    explained += bool(found)
                row = c.table_row(rows, *model)
                if row and 'unit_calibration' in row:
                    unit = row['unit_calibration']
                    self.assertEqual(found.get(unit['preset']), unit['reference'], found)
        # Units with different calibrations, explained by one reference per model.
        self.assertGreaterEqual(explained, 60)

    def test_every_reference_has_committed_evidence(self):
        facts, rows = committed()
        measured = {c.identify(f['tags']) for f in facts if 'measured' in f}
        for row in rows:
            if 'unit_calibration' in row:
                names = [row['model'], *row.get('aliases', [])]
                self.assertTrue(any((row['make'].casefold(), n.casefold()) in
                                    {(m.casefold(), d.casefold()) for m, d in measured} for n in names), row['model'])

    def test_individual_units_use_their_own_presets(self):
        cases = [
            ('SONY', 'ILCE-7CR', 'daylight', [2423, 1024, 1799],
             [([2569., 1024., 1800.], [.9432, 1., .9994]), ([2610., 1024., 1771.], [.9284, 1., 1.0158])]),
            ('Canon', 'Canon EOS 800D', 'daylight', [1893, 1024, 1697],
             [([1872., 1024., 1649.], [1.0112, 1., 1.0291]), ([1876., 1024., 1623.], [1.0091, 1., 1.0456])]),
            ('FUJIFILM', 'X-T5', 'd65', [615, 302, 499],
             [([658., 302., 473.], [.9347, 1., 1.055]), ([646., 302., 494.], [.952, 1., 1.0101])]),
        ]
        for make, model, preset, reference, units in cases:
            rows = [dict(make=c.identify(dict(Make=make, Model=model))[0],
                         model=c.identify(dict(Make=make, Model=model))[1],
                         unit_calibration=dict(preset=preset, reference=reference))]
            for wb, gains in units:
                with self.subTest(model=model, wb=wb):
                    tags = self.tags(make, model, gains)
                    tags['NativeWhiteBalance'] = {preset: wb}
                    result = c.audit(tags, rows)
                    self.assertEqual(result['status'], 'ok', result)
                    self.assertEqual(result['applied'], gains)
            samples = [(gains, {preset: wb}) for wb, gains in units]
            self.assertEqual(c.fits(samples), {preset: reference})

    def test_native_presets_follow_each_makes_tags(self):
        self.assertEqual(c.native_presets({'WB_RGBLevelsDaylight': '2456 1024 1691'}),
                         {'daylight': [2456., 1024., 1691.]})
        canon = c.native_presets({'WB_RGGBLevelsDaylight': '2013 1024 1024 1479',
                                  'WB_RGGBLevelsShade': '2300 1024 1024 1280'})
        self.assertEqual(canon, {'daylight': [2013., 1024., 1479.], 'shade': [2300., 1024., 1280.]})
        # Fujifilm: G, R, B and an EXIF light source per entry; 21 is D65.
        fuji = c.native_presets({'WB_GRBLevelsStandard': '302 383 859 17 302 679 500 21'})
        self.assertEqual(fuji, {'d65': [679., 302., 500.]})
        self.assertEqual(c.native_presets({'WB_RGGBLevelsDaylight': '0 1024 1024 1479'}), {})

    def tags(self, make='SONY', model='ILCE-7M4', gains=(.9817, 1., .9687)):
        diagonal = f'{gains[0]} 0 0 0 1 0 0 0 {gains[2]}'
        return dict(Make=make, Model=model, DNGVersion='1 7 1 0',
                    NativeWhiteBalance={'daylight': [2456., 1024., 1691.]},
                    CameraCalibrationSig='com.adobe', ProfileCalibrationSig='com.adobe',
                    CameraCalibration1=diagonal, CameraCalibration2=diagonal)

    def test_missing_body_is_a_detected_mismatch_not_a_make_fallback(self):
        r = c.audit(self.tags(), [])
        self.assertEqual(r['status'], 'missing')
        self.assertEqual(r['measured'], [.9817, 1., .9687])
        self.assertEqual(r['applied'], [1.] * 3)

    def test_a_dng_alone_cannot_propose_a_fixed_model_gain(self):
        t = self.tags()
        del t['NativeWhiteBalance']
        result = c.audit(t, [])
        self.assertEqual(result['proposed'], ['no stored preset reproduces it'])
        rows = [dict(make='Sony', model='ILCE-7M4',
                     unit_calibration=dict(preset='daylight', reference=[2411, 1024, 1638]))]
        result = c.audit(t, rows)
        self.assertEqual(result['status'], 'unsupported')

    def test_signatures_must_match(self):
        t = self.tags(); t['ProfileCalibrationSig'] = 'another profile'
        self.assertEqual(c.audit(t, [])['status'], 'unsupported')

    def test_complex_or_invalid_values_are_not_silently_flattened(self):
        for key, value in [('CameraCalibration2', '1 0 0 0 1 0 0 0 1'),
                           ('CameraCalibration1', '1 .1 0 0 1 0 0 0 1'),
                           ('CameraCalibration1', 'nan 0 0 0 1 0 0 0 1'),
                           ('AnalogBalance', '1 nan 1')]:
            with self.subTest(key=key, value=value):
                t = self.tags(); t[key] = value
                self.assertEqual(c.audit(t, [])['status'], 'unsupported')

    def test_analog_balance_is_recorded_apart_from_calibration(self):
        # Canon sRAW of one unit: Adobe writes AnalogBalance for the linear DNG and the
        # same CameraCalibration as that unit's mosaic RAW.
        t = self.tags()
        t['AnalogBalance'] = '1.386256 1 2.397541'
        r = c.audit(t, [])
        self.assertEqual(r['measured'], [.9817, 1., .9687])
        self.assertEqual(r['analog_balance'], [1.386256, 1., 2.397541])
        del t['CameraCalibration1'], t['CameraCalibration2']
        self.assertEqual(c.audit(t, [])['measured'], [1.] * 3)

    def test_unknown_identity_camera_is_not_a_false_pass(self):
        t = dict(Make='OLYMPUS CORPORATION', Model='E-M10MarkIV', DNGVersion='1 4 0 0')
        self.assertEqual(c.audit(t, [])['status'], 'missing')
        self.assertEqual(c.audit(t, [dict(make='Olympus', model='E-M10MarkIV')])['status'], 'ok')

    def test_alias_and_case_match(self):
        r = c.audit(self.tags(), [dict(make='Sony', model='A7 IV', aliases=['ILCE-7M4'],
                                       unit_calibration=dict(preset='daylight', reference=[2411, 1024, 1638]))])
        self.assertEqual(r['status'], 'ok', r)


if __name__ == '__main__':
    unittest.main()
