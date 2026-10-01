// Engine 4 per-pixel color and tone stage: a port of `pipeline::process_pixel` for
// recipes with the reference curves, color and calibration (see `pixel_params.rs`).
// The CPU implementation is the reference; functions keep its names and order.
// `P_*` indices into `params` are generated from `pixel_params::FIELDS`.

@group(0) @binding(0) var<storage, read> samples: array<f32>;
@group(0) @binding(1) var<storage, read> positions: array<f32>;
@group(0) @binding(2) var<storage, read_write> output: array<f32>;
@group(0) @binding(3) var<storage, read> params: array<f32>;
@group(0) @binding(4) var<storage, read> tables: array<f32>;
@group(0) @binding(5) var<uniform> size: vec4<u32>;
// Mask weights: one byte per mask, four per word, `P_MASK_WORDS` words per pixel.
@group(0) @binding(6) var<storage, read> weights: array<u32>;

const TO_2020 = mat3x3<f32>(
    vec3(0.627404, 0.069097, 0.016391),
    vec3(0.329283, 0.91954, 0.088013),
    vec3(0.043313, 0.011362, 0.895595),
);
const FROM_2020 = mat3x3<f32>(
    vec3(1.660491, -0.12455, -0.018151),
    vec3(-0.587641, 1.1329, -0.100579),
    vec3(-0.07285, -0.008349, 1.11873),
);
const PRO_TO_RGB = mat3x3<f32>(
    vec3(2.034075, -0.228813, -0.00857),
    vec3(-0.727334, 1.231731, -0.153286),
    vec3(-0.306742, -0.002918, 1.161856),
);
const RGB_TO_PRO = mat3x3<f32>(
    vec3(0.529345, 0.098374, 0.016883),
    vec3(0.330072, 0.873462, 0.117673),
    vec3(0.140583, 0.028164, 0.865444),
);
const TAU: f32 = 6.28318530717958647692;
const OUTSIDE: f32 = -1e38;

fn p(i: u32) -> f32 {
    return params[i];
}

// masks::local: the pixel's summed mask adjustments, and whether any mask reaches it.
const LOCAL_LEN: u32 = 18u;
const L_TEMPERATURE: u32 = 0u;
const L_TINT: u32 = 1u;
const L_EXPOSURE: u32 = 2u;
const L_CONTRAST: u32 = 3u;
const L_HIGHLIGHTS: u32 = 4u;
const L_SHADOWS: u32 = 5u;
const L_WHITES: u32 = 6u;
const L_BLACKS: u32 = 7u;
const L_DEHAZE: u32 = 10u;
const L_HUE: u32 = 11u;
const L_SATURATION: u32 = 12u;
const L_COLOR: u32 = 15u;
var<private> delta: array<f32, 18>;
var<private> masked: bool;
fn load_delta(i: u32) {
    masked = false;
    for (var k = 0u; k < LOCAL_LEN; k++) {
        delta[k] = 0.0;
    }
    let n = u32(p(P_MASKS));
    if n == 0u {
        return;
    }
    let words = u32(p(P_MASK_WORDS));
    let base = offset(P_MASK_DELTAS);
    for (var g = 0u; g < n; g++) {
        let byte = (weights[i * words + g / 4u] >> (8u * (g % 4u))) & 255u;
        if byte > 0u {
            masked = true;
            let w = f32(byte) / 255.0;
            for (var k = 0u; k < LOCAL_LEN; k++) {
                delta[k] += table(base + i32(g * LOCAL_LEN + k)) * w;
            }
        }
    }
}
fn offset(i: u32) -> i32 {
    return i32(params[i]);
}
/// Row-major 3×3 matrix stored at `params[i..i + 9]`.
fn matrix(i: u32) -> mat3x3<f32> {
    return transpose(mat3x3<f32>(
        vec3(p(i), p(i + 1u), p(i + 2u)),
        vec3(p(i + 3u), p(i + 4u), p(i + 5u)),
        vec3(p(i + 6u), p(i + 7u), p(i + 8u)),
    ));
}
fn rem_euclid(x: f32, y: f32) -> f32 {
    let r = x - trunc(x / y) * y;
    return select(r, r + abs(y), r < 0.0);
}
fn powf(x: f32, y: f32) -> f32 {
    if x <= 0.0 {
        return select(0.0, 1.0, y == 0.0);
    }
    return pow(x, y);
}
fn cbrt(x: f32) -> f32 {
    return sign(x) * powf(abs(x), 1.0 / 3.0);
}
fn srgb_decode(v: f32) -> f32 {
    if v <= 0.04045 {
        return v / 12.92;
    }
    return powf((v + 0.055) / 1.055, 2.4);
}
fn srgb_encode(v: f32) -> f32 {
    if v <= 0.0031308 {
        return 12.92 * v;
    }
    return 1.055 * powf(max(v, 0.0), 1.0 / 2.4) - 0.055;
}
fn table(i: i32) -> f32 {
    return tables[u32(i)];
}
fn table3(i: i32) -> vec3<f32> {
    return vec3(table(i), table(i + 1), table(i + 2));
}
fn hsv_to_rgb(h: f32, v: f32, s: f32) -> vec3<f32> {
    let c = v * s;
    let x = c * (1.0 - abs(h % 2.0 - 1.0));
    var rgb: vec3<f32>;
    switch u32(h) {
        case 0u: { rgb = vec3(c, x, 0.0); }
        case 1u: { rgb = vec3(x, c, 0.0); }
        case 2u: { rgb = vec3(0.0, c, x); }
        case 3u: { rgb = vec3(0.0, x, c); }
        case 4u: { rgb = vec3(x, 0.0, c); }
        default: { rgb = vec3(c, 0.0, x); }
    }
    return rgb;
}
fn hue_of(rgb: vec3<f32>, max_v: f32, d: f32) -> f32 {
    if d < 1e-9 {
        return 0.0;
    } else if max_v == rgb.x {
        return rem_euclid((rgb.y - rgb.z) / d, 6.0) / 6.0;
    } else if max_v == rgb.y {
        return ((rgb.z - rgb.x) / d + 2.0) / 6.0;
    }
    return ((rgb.x - rgb.y) / d + 4.0) / 6.0;
}

// camera_profiles::Table
struct Table {
    data: i32,
    dims: vec3<u32>,
    srgb: bool,
}
fn table_at(i: u32) -> Table {
    return Table(offset(i), vec3(u32(p(i + 1u)), u32(p(i + 2u)), u32(p(i + 3u))), p(i + 4u) != 0.0);
}
fn table_lookup(t: Table, h: f32, s: f32, v: f32) -> vec3<f32> {
    let coords = vec3(
        rem_euclid(h, 1.0) * f32(t.dims.x),
        clamp(s, 0.0, 1.0) * f32(t.dims.y - 1u),
        clamp(v, 0.0, 1.0) * f32(t.dims.z - 1u),
    );
    let base = vec3<u32>(floor(coords));
    let f = coords - vec3<f32>(base);
    var out = vec3(0.0);
    for (var z = 0u; z < 2u; z++) {
        for (var y = 0u; y < 2u; y++) {
            for (var x = 0u; x < 2u; x++) {
                let ix = (base.x + x) % t.dims.x;
                let iy = min(base.y + y, t.dims.y - 1u);
                let iz = min(base.z + z, t.dims.z - 1u);
                let wx = select(1.0 - f.x, f.x, x == 1u);
                let wy = select(1.0 - f.y, f.y, y == 1u);
                let wz = select(1.0 - f.z, f.z, z == 1u);
                let cell = (iz * t.dims.x + ix) * t.dims.y + iy;
                out += table3(t.data + i32(cell) * 3) * (wx * wy * wz);
            }
        }
    }
    return out;
}
fn table_apply(t: Table, rgb: vec3<f32>, other: i32, weight: f32) -> vec3<f32> {
    let max_v = max(max(max(rgb.x, rgb.y), rgb.z), 0.0);
    let min_v = max(min(min(rgb.x, rgb.y), rgb.z), 0.0);
    let d = max_v - min_v;
    let h = hue_of(rgb, max_v, d);
    let sat = select(0.0, d / max_v, max_v > 0.0);
    let encoded = t.srgb && t.dims.z > 1u;
    let v = select(max_v, srgb_encode(max_v), encoded);
    var delta = table_lookup(t, h, sat, v);
    if other >= 0 {
        let b = table_lookup(Table(other, t.dims, t.srgb), h, sat, v);
        delta = delta * (1.0 - weight) + b * weight;
    }
    let h2 = rem_euclid(h + delta.x / 360.0, 1.0) * 6.0;
    let s = clamp(sat * delta.y, 0.0, 1.0);
    let v2 = select(v * delta.z, srgb_decode(v * delta.z), encoded);
    let c = v2 * s;
    return hsv_to_rgb(h2, v2, s) + (v2 - c);
}
/// Piecewise-linear profile tone curve: `partition_point` then interpolation.
fn tone_eval(v: f32) -> f32 {
    let base = offset(P_TONE);
    let n = u32(p(P_TONE_COUNT));
    let x = clamp(v, 0.0, 1.0);
    var lo = 0u;
    var hi = n;
    while lo < hi {
        let mid = (lo + hi) / 2u;
        if table(base + i32(mid) * 2) <= x {
            lo = mid + 1u;
        } else {
            hi = mid;
        }
    }
    let i = min(max(lo, 1u) - 1u, n - 2u);
    let a = vec2(table(base + i32(i) * 2), table(base + i32(i) * 2 + 1));
    let b = vec2(table(base + i32(i) * 2 + 2), table(base + i32(i) * 2 + 3));
    let t = clamp((x - a.x) / (b.x - a.x), 0.0, 1.0);
    return a.y * (1.0 - t) + b.y * t;
}
/// A 0–1 lookup table of `size + 1` samples, linearly interpolated.
fn lut(base: i32, size: u32, v: f32) -> f32 {
    let f = clamp(v, 0.0, 1.0) * f32(size);
    let i = min(u32(f), size - 1u);
    let a = table(base + i32(i));
    return a + (table(base + i32(i) + 1) - a) * (f - f32(i));
}
/// DNG RGBTone: map the brightest and darkest channels, keep the middle one's position.
fn rgb_tone_values(q: vec3<f32>, a: f32, b: f32, lo: f32, hi: f32) -> vec3<f32> {
    if hi - lo > 1e-8 {
        return a + (b - a) * (q - lo) / (hi - lo);
    }
    return vec3(a);
}
fn enhanced_curve(rgb: vec3<f32>) -> vec3<f32> {
    let q = vec3(
        srgb_encode(clamp(rgb.x, 0.0, 1.0)),
        srgb_encode(clamp(rgb.y, 0.0, 1.0)),
        srgb_encode(clamp(rgb.z, 0.0, 1.0)),
    );
    let lo = min(min(q.x, q.y), q.z);
    let hi = max(max(max(q.x, q.y), q.z), 0.0);
    let base = offset(P_ENH_CURVE);
    let r = rgb_tone_values(q, lut(base, 4096u, lo), lut(base, 4096u, hi), lo, hi);
    return vec3(srgb_decode(r.x), srgb_decode(r.y), srgb_decode(r.z));
}
// CameraProfile::finish with profile tone.
fn profile_finish(rgb: vec3<f32>) -> vec3<f32> {
    var q = RGB_TO_PRO * rgb;
    if offset(P_LOOK) >= 0 {
        q = table_apply(table_at(P_LOOK), q, -1, 0.0);
    }
    if offset(P_ENH) >= 0 {
        q = table_apply(table_at(P_ENH), clamp(q, vec3(0.0), vec3(1.0)), -1, 0.0);
    }
    q = clamp(q, vec3(0.0), vec3(1.0));
    let low = min(min(q.x, q.y), q.z);
    let high = max(max(max(q.x, q.y), q.z), 0.0);
    q = rgb_tone_values(q, tone_eval(low), tone_eval(high), low, high);
    if offset(P_ENH_CURVE) >= 0 {
        q = enhanced_curve(q);
    }
    return PRO_TO_RGB * q;
}
fn calibrate(color: vec3<f32>) -> vec3<f32> {
    let q = matrix(P_CALIBRATION) * color;
    let shadow = p(P_SHADOW_TINT);
    if shadow == 0.0 {
        return q;
    }
    let y = max(0.2126 * q.x + 0.7152 * q.y + 0.0722 * q.z, 0.0);
    let amount = abs(shadow) * y * exp(-6.0 * y);
    let direction = select(vec3(-0.331, 0.029, -0.152), vec3(0.116, -0.189, -0.002), shadow > 0.0);
    return q + amount * direction;
}
// ExposureRamp::new(black).eval(x), for masks that change exposure.
fn ramp_with(x: f32, black_in: f32) -> f32 {
    let black = clamp(black_in, 0.0, 0.5);
    let slope = 1.0 / (1.0 - black);
    let radius = min(0.5 * black, 1.0 / 16.0 / slope);
    let q = select(0.0, slope / (4.0 * radius), radius > 0.0);
    if x <= black - radius {
        return 0.0;
    } else if x >= black + radius {
        return (x - black) * slope;
    }
    let y = x - (black - radius);
    return q * y * y;
}
fn ramp(x: f32) -> f32 {
    let black = p(P_RAMP);
    let slope = p(P_RAMP + 1u);
    let radius = p(P_RAMP + 2u);
    let q = p(P_RAMP + 3u);
    if x <= black - radius {
        return 0.0;
    } else if x >= black + radius {
        return (x - black) * slope;
    }
    let y = x - (black - radius);
    return q * y * y;
}
fn luma2020(c: vec3<f32>) -> f32 {
    return 0.2627 * c.x + 0.678 * c.y + 0.0593 * c.z;
}
// local_tone::LocalToneMap::gain
fn local_curve(i: u32, base: f32) -> f32 {
    let t = offset(i);
    if t < 0 {
        return 0.0;
    }
    let key = p(i + 1u);
    let lo = p(i + 2u);
    let hi = p(i + 3u);
    let f = clamp((base - key - lo) / (hi - lo) * 48.0 - 0.5, 0.0, 47.0);
    let j = min(u32(f), 46u);
    let a = table(t + i32(j));
    return a + (table(t + i32(j) + 1) - a) * (f - f32(j));
}
// Slider position `k` of the 7 around a measured family, with 0 inserted at index 3,
// and its table index (-1 for the identity).
fn slider_point(values: i32, k: u32) -> f32 {
    if k == 3u {
        return 0.0;
    }
    return table(values + i32(select(k - 1u, k, k < 3u)));
}
fn slider_table(k: u32) -> i32 {
    if k == 3u {
        return -1;
    }
    return i32(select(k - 1u, k, k < 3u));
}
// The bracketing pair of measured positions for slider `s` and the weight between.
fn bracket(values: i32, s: f32) -> vec2<f32> {
    var j = 5u;
    for (var k = 0u; k < 6u; k++) {
        if s <= slider_point(values, k + 1u) {
            j = k;
            break;
        }
    }
    let s0 = slider_point(values, j);
    let s1 = slider_point(values, j + 1u);
    return vec2(f32(j), (s - s0) / (s1 - s0));
}
// local_tone::family: a Shadows (0) or Highlights (1) gain at slider `s`.
fn family(f: u32, s_in: f32, key: f32, base: f32) -> f32 {
    if s_in == 0.0 {
        return 0.0;
    }
    let s = clamp(s_in, -1.0, 1.0);
    let t = offset(P_LOCAL_FAMILIES) + i32(f * 290u);
    let lo = table(t + 288);
    let hi = table(t + 289);
    let b = bracket(offset(P_LOCAL_FAMILIES) + 580, s);
    let j = u32(b.x);
    let x = clamp((base - key - lo) / (hi - lo) * 48.0 - 0.5, 0.0, 47.0);
    let i = min(u32(x), 46u);
    var y = vec2(0.0);
    for (var k = 0u; k < 2u; k++) {
        let row = slider_table(j + k);
        if row >= 0 {
            let a = table(t + row * 48 + i32(i));
            y[k] = a + (table(t + row * 48 + i32(i) + 1) - a) * (x - f32(i));
        }
    }
    return y.x + (y.y - y.x) * b.y;
}
// basic_tone::slider over a 6 × 64 table at `t`, with positions at `values`.
fn measured(t: i32, values: i32, s_in: f32, x: f32) -> f32 {
    if s_in == 0.0 {
        return x;
    }
    let s = clamp(s_in, -1.0, 1.0);
    let b = bracket(values, s);
    let j = u32(b.x);
    let f = clamp(x * 64.0 - 0.5, -0.5, 63.5);
    let i = clamp(i32(floor(f)), 0, 62);
    var y = vec2(x);
    for (var k = 0u; k < 2u; k++) {
        let row = slider_table(j + k);
        if row >= 0 {
            let a = table(t + row * 64 + i);
            y[k] = clamp(a + (table(t + row * 64 + i + 1) - a) * (f - f32(i)), 0.0, 1.0);
        }
    }
    return y.x + (y.y - y.x) * b.y;
}
// basic_tone::compose at the pixel's local slider values.
fn local_tone_curve(x_in: f32) -> f32 {
    let t = offset(P_LOCAL_TONE);
    var x = measured(t, t + 1536, delta[L_DEHAZE], x_in);
    x = measured(t + 384, t + 1542, delta[L_CONTRAST], x);
    x = measured(t + 768, t + 1542, delta[L_WHITES], x);
    return measured(t + 1152, t + 1542, delta[L_BLACKS], x);
}
fn local_gain(pos: vec2<f32>, rgb: vec3<f32>) -> f32 {
    let w = u32(p(P_LOCAL_SIZE));
    let h = u32(p(P_LOCAL_SIZE + 1u));
    let fx = clamp((pos.x + 0.5) * p(P_LOCAL_SCALE) - 0.5, 0.0, f32(w - 1u));
    let fy = clamp((pos.y + 0.5) * p(P_LOCAL_SCALE + 1u) - 0.5, 0.0, f32(h - 1u));
    let ix = u32(fx);
    let iy = u32(fy);
    let jx = min(ix + 1u, w - 1u);
    let jy = min(iy + 1u, h - 1u);
    let tx = fx - f32(ix);
    let ty = fy - f32(iy);
    var coef = vec2(0.0);
    for (var k = 0u; k < 2u; k++) {
        let base = offset(P_LOCAL_A + k);
        let top = table(base + i32(iy * w + ix)) * (1.0 - tx) + table(base + i32(iy * w + jx)) * tx;
        let bottom = table(base + i32(jy * w + ix)) * (1.0 - tx) + table(base + i32(jy * w + jx)) * tx;
        coef[k] = top * (1.0 - ty) + bottom * ty;
    }
    let y = max(0.2126 * rgb.x + 0.7152 * rgb.y + 0.0722 * rgb.z, 6e-4);
    let base = coef.x * log2(y) + coef.y;
    if masked && (delta[L_SHADOWS] != 0.0 || delta[L_HIGHLIGHTS] != 0.0) {
        let s = p(P_GLOBAL_SH) + delta[L_SHADOWS];
        let h = p(P_GLOBAL_SH + 1u) + delta[L_HIGHLIGHTS];
        return exp2(family(0u, s, p(P_LOCAL_KEYS), base)
            + family(1u, h, p(P_LOCAL_KEYS + 1u), base));
    }
    return exp2(local_curve(P_SHADOWS, base) + local_curve(P_HIGHLIGHTS, base));
}
fn parametric(x: f32) -> f32 {
    if p(P_PARAMETRIC_ON) == 0.0 {
        return x;
    }
    var anchors = array<f32, 5>(0.0, p(P_SPLITS), p(P_SPLITS + 1u), p(P_SPLITS + 2u), 1.0);
    var delta = 0.0;
    for (var i = 0u; i < 4u; i++) {
        let lo = anchors[i];
        let hi = anchors[i + 1u];
        let mid = (lo + hi) * 0.5;
        let radius = (hi - lo) * 1.5;
        let w = clamp(1.0 - abs((x - mid) / radius), 0.0, 1.0);
        delta += p(P_PARAMETRIC + i) * w * w * (3.0 - 2.0 * w) * 0.18;
    }
    return clamp(x + delta * 4.0 * x * (1.0 - x), 0.0, 1.0);
}
fn level(v: f32) -> f32 {
    let bp = p(P_LEVELS);
    let wp = p(P_LEVELS + 1u);
    let midtone = p(P_LEVELS + 2u);
    var x = clamp((v - bp) / (wp - bp), 0.0, 1.0);
    if midtone != 1.0 {
        x = powf(x, 1.0 / midtone);
    }
    // Contrast is a measured curve on this path: the S-curve has power 1.
    let low = x;
    return parametric(low / max(low + (1.0 - x), 1e-8));
}
fn reference_curves(rgb: vec3<f32>) -> vec3<f32> {
    let pro = RGB_TO_PRO * rgb;
    var q = vec3(
        srgb_encode(clamp(pro.x, 0.0, 1.0)),
        srgb_encode(clamp(pro.y, 0.0, 1.0)),
        srgb_encode(clamp(pro.z, 0.0, 1.0)),
    );
    let basic = offset(P_BASIC);
    if basic >= 0 {
        q = clamp(q, vec3(0.0), vec3(1.0));
        let lo = min(min(q.x, q.y), q.z);
        let hi = max(max(max(q.x, q.y), q.z), 0.0);
        q = rgb_tone_values(q, lut(basic, 1024u, lo), lut(basic, 1024u, hi), lo, hi);
    }
    if masked && (delta[L_CONTRAST] != 0.0 || delta[L_WHITES] != 0.0 || delta[L_BLACKS] != 0.0
        || delta[L_DEHAZE] != 0.0) {
        q = clamp(q, vec3(0.0), vec3(1.0));
        let lo = min(min(q.x, q.y), q.z);
        let hi = max(max(max(q.x, q.y), q.z), 0.0);
        q = rgb_tone_values(q, local_tone_curve(lo), local_tone_curve(hi), lo, hi);
    }
    q = vec3(level(q.x), level(q.y), level(q.z));
    let lo = min(min(q.x, q.y), q.z);
    let hi = max(max(max(q.x, q.y), q.z), 0.0);
    let master = offset(P_MASTER);
    let a = lut(master, 4096u, lo);
    let b = lut(master, 4096u, hi);
    var m = vec3(a);
    if hi - lo > 1e-8 {
        m = a + (b - a) * (q - lo) / (hi - lo);
    }
    let channels = vec3(
        srgb_decode(lut(offset(P_CHANNELS), 4096u, m.x)),
        srgb_decode(lut(offset(P_CHANNELS + 1u), 4096u, m.y)),
        srgb_decode(lut(offset(P_CHANNELS + 2u), 4096u, m.z)),
    );
    return PRO_TO_RGB * channels;
}
// color_mixer::ColorMixer (36 hues × 6 saturations × 6 values).
fn mixer(rgb: vec3<f32>) -> vec3<f32> {
    let q = max(RGB_TO_PRO * rgb, vec3(0.0));
    let max_v = max(max(max(q.x, q.y), q.z), 0.0);
    let min_v = min(min(q.x, q.y), q.z);
    if max_v <= 1e-6 {
        return rgb;
    }
    let d = max_v - min_v;
    let h = hue_of(q, max_v, d);
    let s = d / max_v;
    let fh = rem_euclid(h, 1.0) * 36.0 - 0.5;
    let fs = clamp(sqrt(clamp(s, 0.0, 1.0)) * 6.0 - 0.5, 0.0, 5.0);
    let fv = clamp(powf(clamp(max_v, 0.0, 1.0), 0.45) * 6.0 - 0.5, 0.0, 5.0);
    let h0 = floor(fh);
    let th = fh - h0;
    let ts = fract(fs);
    let tv = fract(fv);
    let s0 = u32(fs);
    let v0 = u32(fv);
    let s1 = min(s0 + 1u, 5u);
    let v1 = min(v0 + 1u, 5u);
    let base = offset(P_MIXER);
    var delta = vec3(0.0);
    for (var dh = 0; dh < 2; dh++) {
        let hi = u32(rem_euclid(h0 + f32(dh), 36.0));
        let wh = select(1.0 - th, th, dh == 1);
        for (var k = 0u; k < 2u; k++) {
            let si = select(s0, s1, k == 1u);
            let ws = select(1.0 - ts, ts, k == 1u);
            for (var m = 0u; m < 2u; m++) {
                let vi = select(v0, v1, m == 1u);
                let wv = select(1.0 - tv, tv, m == 1u);
                delta += table3(base + i32((hi * 6u + si) * 6u + vi) * 3) * (wh * ws * wv);
            }
        }
    }
    let h2 = rem_euclid(h + delta.x, 1.0) * 6.0;
    let s2 = clamp(s * exp2(delta.y), 0.0, 1.0);
    let v = max_v * exp2(delta.z);
    let c = v * s2;
    return PRO_TO_RGB * (hsv_to_rgb(h2, v, s2) + (max_v * exp2(delta.z) - c));
}
// color_grade::ColorGrade
fn grade_at(base: i32, l: f32) -> vec3<f32> {
    let bins = u32(p(P_GRADE + 2u));
    let f = clamp(clamp(l, 0.0, 1.0) * f32(bins) - 0.5, 0.0, f32(bins - 1u));
    let i = min(u32(f), bins - 2u);
    let t = f - f32(i);
    return table3(base + i32(i) * 3) * (1.0 - t) + table3(base + i32(i + 1u) * 3) * t;
}
fn grade(rgb: vec3<f32>) -> vec3<f32> {
    let y = max(0.2126 * rgb.x + 0.7152 * rgb.y + 0.0722 * rgb.z, 0.0);
    let l = srgb_encode(min(y, 1.0));
    let g = grade_at(offset(P_GRADE), l);
    let o = grade_at(offset(P_GRADE + 1u), l);
    let q = RGB_TO_PRO * rgb;
    var out: vec3<f32>;
    for (var c = 0; c < 3; c++) {
        let v = max(q[c], 0.0) * exp2(g[c]);
        out[c] = srgb_decode(clamp(srgb_encode(clamp(v, 0.0, 1.0)) + o[c], 0.0, 1.0));
    }
    return PRO_TO_RGB * out;
}
fn srgb_to_lab(q: vec3<f32>) -> vec3<f32> {
    let a = vec3(
        0.41222146 * q.x + 0.53633255 * q.y + 0.051445995 * q.z,
        0.2119035 * q.x + 0.6806995 * q.y + 0.10739696 * q.z,
        0.08830246 * q.x + 0.28171885 * q.y + 0.6299787 * q.z,
    );
    let b = vec3(cbrt(a.x), cbrt(a.y), cbrt(a.z));
    return vec3(
        0.21045426 * b.x + 0.7936178 * b.y - 0.004072047 * b.z,
        1.9779985 * b.x - 2.4285922 * b.y + 0.4505937 * b.z,
        0.025904037 * b.x + 0.78277177 * b.y - 0.80867577 * b.z,
    );
}
fn lab_to_srgb(q: vec3<f32>) -> vec3<f32> {
    let a = vec3(
        q.x + 0.39633778 * q.y + 0.21580376 * q.z,
        q.x - 0.105561346 * q.y - 0.06385417 * q.z,
        q.x - 0.08948418 * q.y - 1.2914855 * q.z,
    );
    let b = a * a * a;
    return vec3(
        4.0767417 * b.x - 3.3077116 * b.y + 0.23096994 * b.z,
        -1.268438 * b.x + 2.6097574 * b.y - 0.3413194 * b.z,
        -0.0041960863 * b.x - 0.7034186 * b.y + 1.7076147 * b.z,
    );
}
fn hue_weights(hue: f32) -> array<f32, 8> {
    var centers = array<f32, 8>(0.081, 0.151, 0.305, 0.395, 0.541, 0.733, 0.815, 0.912);
    var weights = array<f32, 8>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    let h0 = rem_euclid(hue, 1.0);
    for (var i = 0u; i < 8u; i++) {
        let left = centers[i];
        let right = select(centers[i + 1u], centers[0] + 1.0, i == 7u);
        let h = select(h0, h0 + 1.0, h0 < left);
        if h >= left && h <= right {
            let t = (h - left) / (right - left);
            weights[i] = 1.0 - t;
            weights[(i + 1u) % 8u] = t;
            break;
        }
    }
    return weights;
}
// effects::Effects::defringe_color
fn defringe_step(x: f32) -> f32 {
    let t = clamp((x + 0.025) / 0.05, 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}
fn defringe(lab_in: vec3<f32>, h: f32) -> vec3<f32> {
    var lab = lab_in;
    var centers = array<f32, 2>(0.875, 0.46);
    for (var i = 0u; i < 2u; i++) {
        let amount = p(P_DEFRINGE + i);
        if amount == 0.0 { continue; }
        let lo = (p(P_DEFRINGE_RANGES + i * 2u) - 0.5) * 0.5;
        let hi = (p(P_DEFRINGE_RANGES + i * 2u + 1u) - 0.5) * 0.5;
        let d = rem_euclid(h - centers[i] + 0.5, 1.0) - 0.5;
        let w = defringe_step(d - lo) * defringe_step(hi - d);
        let chroma = length(lab.yz);
        let strength = (1.0 - 0.45 * exp(-chroma / 0.09)) * (1.0 - exp(-amount * 20.0 / 2.5));
        let k = 1.0 - w * strength;
        lab.y *= k;
        lab.z *= k;
    }
    return lab;
}
/// Oklab color controls with engine 4's measured sliders zeroed: hue angle
/// round trip, Defringe and Monochrome.
fn adjust(lab_in: vec3<f32>) -> vec3<f32> {
    var lab = lab_in;
    let chroma = length(lab.yz);
    let hue = rem_euclid(atan2(lab.z, lab.y), TAU) / TAU;
    let weights = hue_weights(hue);
    let angle = hue * TAU;
    lab.x = clamp(lab.x, 0.0, 1.0);
    lab.y = cos(angle) * chroma;
    lab.z = sin(angle) * chroma;
    lab = defringe(lab, hue);
    if p(P_MONO) != 0.0 {
        var shift = 0.0;
        for (var i = 0u; i < 8u; i++) {
            shift += p(P_GRAY_MIX + i) * weights[i];
        }
        lab.x = clamp(lab.x + shift * 0.25, 0.0, 1.0);
        lab.y = 0.0;
        lab.z = 0.0;
    }
    return lab;
}
fn process_pixel(sample: vec3<f32>, pos: vec2<f32>) -> vec3<f32> {
    // tone_stage
    var wb = vec3(1.0);
    if masked {
        let temp = vec3(p(P_LOCAL_WB), p(P_LOCAL_WB + 1u), p(P_LOCAL_WB + 2u));
        let tint = vec3(p(P_LOCAL_WB + 3u), p(P_LOCAL_WB + 4u), p(P_LOCAL_WB + 5u));
        wb = exp2(delta[L_TEMPERATURE] * temp + delta[L_TINT] * tint);
    }
    let c = sample * vec3(p(P_WB), p(P_WB + 1u), p(P_WB + 2u)) * wb;
    var pro = matrix(P_CAMERA) * c;
    let hue = offset(P_HUE);
    if hue >= 0 {
        pro = table_apply(table_at(P_HUE), pro, offset(P_HUE2), p(P_HUE_WEIGHT));
    }
    let color = calibrate(PRO_TO_RGB * pro * p(P_PROFILE_SCALE));
    var wide = TO_2020 * color;
    if masked {
        let exposure = exp2(delta[L_EXPOSURE]);
        let tint = exp2(vec3(delta[L_COLOR], delta[L_COLOR + 1u], delta[L_COLOR + 2u]));
        wide = wide * p(P_EXPOSURE) * exposure * tint;
    } else {
        wide = wide * p(P_EXPOSURE);
    }
    if masked && delta[L_EXPOSURE] != 0.0 {
        let black = 0.0015 * exp2(p(P_EXPOSURE_EV) + delta[L_EXPOSURE]);
        wide = vec3(ramp_with(wide.x, black), ramp_with(wide.y, black), ramp_with(wide.z, black));
    } else {
        wide = vec3(ramp(wide.x), ramp(wide.y), ramp(wide.z));
    }
    let y = max(luma2020(wide), 1e-8);
    wide = wide * y / y;
    var rgb = profile_finish(FROM_2020 * wide);
    // Toning the reduced photo for the Shadows/Highlights map (`tone_params`).
    if p(P_TONE_ONLY) != 0.0 {
        return rgb;
    }
    if p(P_LOCAL) != 0.0 {
        rgb *= local_gain(pos, rgb);
    }
    // color_stage
    rgb = reference_curves(rgb);
    if offset(P_MIXER) >= 0 {
        rgb = mixer(rgb);
    }
    if offset(P_GRADE) >= 0 {
        rgb = grade(rgb);
    }
    var lab = srgb_to_lab(rgb);
    if masked && (delta[L_HUE] != 0.0 || delta[L_SATURATION] != 0.0) {
        let angle = radians(delta[L_HUE]);
        let k = max(1.0 + delta[L_SATURATION], 0.0);
        let a = lab.y;
        let b = lab.z;
        lab.y = (a * cos(angle) - b * sin(angle)) * k;
        lab.z = (a * sin(angle) + b * cos(angle)) * k;
    }
    if p(P_ADJUST) != 0.0 {
        lab = adjust(lab);
    } else {
        lab.x = clamp(lab.x, 0.0, 1.0);
    }
    rgb = lab_to_srgb(lab);
    lab = srgb_to_lab(rgb);
    let l = clamp(lab.x, 0.0, 1.0);
    let gray = l * l * l;
    var gamut = 1.0;
    for (var k = 0; k < 3; k++) {
        let v = rgb[k];
        if v < 0.0 {
            gamut = min(gamut, gray / max(gray - v, 1e-8));
        }
        if v > 1.0 {
            gamut = min(gamut, (1.0 - gray) / max(v - gray, 1e-8));
        }
    }
    var out: vec3<f32>;
    for (var k = 0; k < 3; k++) {
        out[k] = clamp(srgb_encode(gray + (rgb[k] - gray) * gamut), 0.0, 1.0);
    }
    return out;
}

@compute @workgroup_size(256)
fn develop(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) local: u32) {
    let i = (group.y * size.y + group.x) * 256u + local;
    if i >= size.x {
        return;
    }
    let pos = vec2(positions[i * 2u], positions[i * 2u + 1u]);
    var out = vec3(1.0);
    // Positions outside the photo are uploaded as OUTSIDE; Lightroom shows white there.
    if pos.x > OUTSIDE {
        load_delta(i);
        out = process_pixel(vec3(samples[i * 3u], samples[i * 3u + 1u], samples[i * 3u + 2u]), pos);
    }
    output[i * 3u] = out.x;
    output[i * 3u + 1u] = out.y;
    output[i * 3u + 2u] = out.z;
}
