//! The per-pixel stage's inputs, flattened for the GPU port in `gpu/develop.wgsl`.
//! Only the current engine's reference path is ported: engine 4 with reference curves,
//! color and calibration and a profile tone curve, which every new photo uses. Other
//! recipes return `None` and render on the CPU, which stays the reference.
use super::{CurveSet, profile_matrix};
use crate::{develop::Recipe, raw::CameraImage};

/// Named slots of the parameter array and their lengths. `wgsl_prelude` turns them into
/// `P_*` index constants for the shader.
const FIELDS: &[(&str, usize)] = &[
    ("CAMERA", 9),
    ("WB", 3),
    ("HUE", 5),
    ("HUE2", 1),
    ("HUE_WEIGHT", 1),
    ("PROFILE_SCALE", 1),
    ("CALIBRATION", 9),
    ("SHADOW_TINT", 1),
    ("EXPOSURE", 1),
    ("RAMP", 4),
    ("LOOK", 5),
    ("ENH", 5),
    ("ENH_CURVE", 1),
    ("TONE", 1),
    ("TONE_COUNT", 1),
    ("LOCAL", 1),
    ("LOCAL_SIZE", 2),
    ("LOCAL_SCALE", 2),
    ("LOCAL_A", 2),
    ("SHADOWS", 4),
    ("HIGHLIGHTS", 4),
    ("BASIC", 1),
    ("LEVELS", 3),
    ("PARAMETRIC_ON", 1),
    ("PARAMETRIC", 4),
    ("SPLITS", 3),
    ("MASTER", 1),
    ("CHANNELS", 3),
    ("MIXER", 1),
    ("GRADE", 3),
    ("ADJUST", 1),
    ("DEFRINGE", 2),
    ("DEFRINGE_RANGES", 4),
    ("MONO", 1),
    ("GRAY_MIX", 8),
];
pub(crate) fn wgsl_prelude() -> String {
    let mut at = 0;
    FIELDS
        .iter()
        .map(|(name, len)| {
            let line = format!("const P_{name}: u32 = {at}u;\n");
            at += len;
            line
        })
        .collect()
}
pub(crate) struct PixelParams {
    pub(crate) params: Vec<f32>,
    pub(crate) tables: Vec<f32>,
}
impl PixelParams {
    fn set(&mut self, name: &str, values: &[f32]) {
        let mut at = 0;
        for (field, len) in FIELDS {
            if *field == name {
                assert_eq!(values.len(), *len, "{name}");
                self.params[at..at + len].copy_from_slice(values);
                return;
            }
            at += len;
        }
        unreachable!("Unknown parameter {name}");
    }
    /// Appends a table and returns its offset as a parameter value.
    fn push(&mut self, values: impl IntoIterator<Item = f32>) -> f32 {
        let at = self.tables.len();
        self.tables.extend(values);
        at as f32
    }
    fn table(&mut self, name: &str, table: Option<&crate::camera_profiles::Table>) {
        let Some(t) = table else {
            return self.set(name, &[-1., 0., 0., 0., 0.]);
        };
        let at = self.push(t.data().iter().flatten().copied());
        let [a, b, c] = t.dims();
        self.set(
            name,
            &[at, a as f32, b as f32, c as f32, t.srgb() as u8 as f32],
        );
    }
}
/// Whether the GPU port renders this (resolved) recipe.
pub(crate) fn supported(r: &Recipe) -> bool {
    let grading = r.grading.iter().any(|g| g[1] != 0. || g[2] != 0.)
        || r.effects.global_grade[1] != 0.
        || r.effects.global_grade[2] != 0.;
    r.engine >= 4
        && r.reference_curves
        && r.reference_color
        && r.reference_calibration
        && r.profile_tone
        && r.profile.is_some()
        // Blending and Balance outside the measured tables use the older operator.
        && (!grading || crate::develop::color_grade::ColorGrade::new(r).is_some())
}
/// Parameters for `im`'s per-pixel stage with the resolved recipe `r`, or `None` when
/// the GPU port does not cover it.
pub(crate) fn pixel_params(im: &CameraImage, r: &Recipe) -> Option<PixelParams> {
    if !supported(r) {
        return None;
    }
    let profile = r.profile.as_ref()?;
    let matrix = profile_matrix(&im.metadata, r);
    let lut = CurveSet::for_image(im, r, matrix);
    let len = FIELDS.iter().map(|f| f.1).sum();
    let mut p = PixelParams {
        params: vec![0.; len],
        tables: Vec::new(),
    };
    p.set("CAMERA", matrix.as_flattened());
    p.set("WB", &r.wb);
    let t = profile.gpu_tables(r.temperature);
    p.table("HUE", t.hue.map(|h| h.0));
    let hue2 = match t.hue.and_then(|h| h.1) {
        Some(table) => p.push(table.data().iter().flatten().copied()),
        None => -1.,
    };
    p.set("HUE2", &[hue2]);
    p.set("HUE_WEIGHT", &[t.hue.map_or(0., |h| h.2)]);
    p.set("PROFILE_SCALE", &[t.exposure_scale]);
    p.set("CALIBRATION", lut.calibration.matrix.as_flattened());
    p.set("SHADOW_TINT", &[lut.calibration.shadow]);
    p.set("EXPOSURE", &[lut.exposure_gain]);
    let ramp = lut.black_ramp.as_ref()?;
    p.set("RAMP", &[ramp.black, ramp.slope, ramp.radius, ramp.q]);
    p.table("LOOK", t.look);
    p.table("ENH", t.enhanced.map(|e| e.0));
    let curve = match t.enhanced {
        Some((_, curve)) => p.push(curve.iter().copied()),
        None => -1.,
    };
    p.set("ENH_CURVE", &[curve]);
    let tone = p.push(t.tone.iter().flatten().copied());
    p.set("TONE", &[tone]);
    p.set("TONE_COUNT", &[t.tone.len() as f32]);
    if let Some(local) = &lut.local {
        p.set("LOCAL", &[1.]);
        p.set("LOCAL_SIZE", &[local.width as f32, local.height as f32]);
        p.set("LOCAL_SCALE", &local.scale);
        let a = p.push(local.a.iter().copied());
        let b = p.push(local.b.iter().copied());
        p.set("LOCAL_A", &[a, b]);
        for (name, curve) in [
            ("SHADOWS", &local.shadows),
            ("HIGHLIGHTS", &local.highlights),
        ] {
            let values = match curve {
                Some(c) => [p.push(c.table), c.key, c.lo, c.hi],
                None => [-1., 0., 0., 0.],
            };
            p.set(name, &values);
        }
    }
    let basic = match &lut.basic {
        Some(b) => p.push(b.lut.iter().copied()),
        None => -1.,
    };
    p.set("BASIC", &[basic]);
    p.set("LEVELS", &[r.black_point, r.white_point, r.midtone]);
    let e = &r.effects;
    p.set("PARAMETRIC_ON", &[(e.parametric != [0.; 4]) as u8 as f32]);
    p.set("PARAMETRIC", &e.parametric);
    p.set("SPLITS", &e.splits);
    let master = p.push(lut.master.values().iter().copied());
    p.set("MASTER", &[master]);
    let channels = lut
        .channels
        .each_ref()
        .map(|c| p.push(c.values().iter().copied()));
    p.set("CHANNELS", &channels);
    let mixer = match &lut.mixer {
        Some(m) => p.push(m.delta.iter().flatten().copied()),
        None => -1.,
    };
    p.set("MIXER", &[mixer]);
    let grade = match &lut.grade {
        Some(g) => [
            p.push(g.gain.iter().flatten().copied()),
            p.push(g.offset.iter().flatten().copied()),
            g.gain.len() as f32,
        ],
        None => [-1., -1., 0.],
    };
    p.set("GRADE", &grade);
    p.set("ADJUST", &[lut.color_adjustments as u8 as f32]);
    p.set("DEFRINGE", &e.defringe);
    p.set("DEFRINGE_RANGES", e.defringe_ranges.as_flattened());
    p.set("MONO", &[e.monochrome as u8 as f32]);
    p.set("GRAY_MIX", &e.gray_mix);
    Some(p)
}
