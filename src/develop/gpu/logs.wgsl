// Log2 luminance for the local-tone blurs: a port of `quality::local_blurs`' first
// step. Appended to `develop.wgsl`, whose camera stage (`params`, `tables`) it shares.
struct LogParams {
    width: u32, height: u32, count: u32, vignetting: i32,
    vignetting_len: u32, center_x: f32, center_y: f32, half: f32,
    amount: f32, pad0: f32, pad1: f32, pad2: f32,
};
@group(1) @binding(0) var<storage, read> photo: array<f32>;
@group(1) @binding(1) var<storage, read_write> logs: array<f32>;
@group(1) @binding(2) var<storage, read> radial: array<f32>;
@group(1) @binding(3) var<uniform> lp: LogParams;

// `lens::Radial::eval` over `len` knots at `radial[at..]`, then `len` values.
fn radial_eval(at: u32, len: u32, r: f32) -> f32 {
    var i = 0u;
    while i < len && radial[at + i] <= r {
        i++;
    }
    if i == 0u {
        return radial[at + len];
    }
    if i == len {
        return radial[at + 2u * len - 1u];
    }
    let t = (r - radial[at + i - 1u]) / (radial[at + i] - radial[at + i - 1u]);
    let a = radial[at + len + i - 1u];
    return a + (radial[at + len + i] - a) * t;
}
@compute @workgroup_size(256)
fn log_luminance(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) local: u32) {
    let i = (group.y * lp.count + group.x) * 256u + local;
    if i >= lp.width * lp.height {
        return;
    }
    var gain = 1.0;
    if lp.vignetting >= 0 {
        let dx = f32(i % lp.width) + 0.5 - lp.center_x;
        let dy = f32(i / lp.width) + 0.5 - lp.center_y;
        let v = radial_eval(u32(lp.vignetting), lp.vignetting_len, sqrt(dx * dx + dy * dy) / lp.half);
        gain = powf(v, lp.amount);
    }
    let c = vec3(photo[3u * i], photo[3u * i + 1u], photo[3u * i + 2u]) * gain
        * vec3(p(P_WB), p(P_WB + 1u), p(P_WB + 2u));
    var pro = matrix(P_CAMERA) * c;
    if offset(P_HUE) >= 0 {
        pro = table_apply(table_at(P_HUE), pro, offset(P_HUE2), p(P_HUE_WEIGHT));
    }
    let rgb = PRO_TO_RGB * pro * p(P_PROFILE_SCALE);
    logs[i] = log2(max(0.2126 * rgb.x + 0.7152 * rgb.y + 0.0722 * rgb.z, 1e-6));
}
