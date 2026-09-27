// Full-resolution stages on the photo kept on the device: the local-tone box blurs
// and gain (`quality::box_blur`, `quality::apply_local`), the toned image reduced for
// the Shadows/Highlights map (`pipeline::preview_source`) and region sampling through
// geometry, lens correction and noise reduction (`pipeline::sample_region`). The CPU
// versions are the reference; functions keep their names and order. Every entry point
// uses its own bindings.

// Box blur along rows (axis 0) or columns (axis 1) as the CPU does it: a running sum
// per line, then window differences.
@group(0) @binding(0) var<storage, read> blur_src: array<f32>;
@group(0) @binding(1) var<storage, read_write> prefix: array<f32>;
@group(0) @binding(2) var<storage, read_write> blur_dst: array<f32>;
// width, height, radius, axis.
@group(0) @binding(3) var<uniform> blur: vec4<u32>;

@compute @workgroup_size(64)
fn running_sum(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = blur.x;
    let h = blur.y;
    var sum = 0.0;
    if blur.w == 0u {
        if id.x >= h { return; }
        for (var x = 0u; x < w; x++) {
            let i = id.x * w + x;
            sum += blur_src[i];
            prefix[i] = sum;
        }
    } else {
        if id.x >= w { return; }
        for (var y = 0u; y < h; y++) {
            let i = y * w + id.x;
            sum += blur_src[i];
            prefix[i] = sum;
        }
    }
}
@compute @workgroup_size(16, 16)
fn window(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = blur.x;
    let h = blur.y;
    let r = blur.z;
    if id.x >= w || id.y >= h { return; }
    var at = id.x;
    var len = w;
    var line = id.y * w;
    var step = 1u;
    if blur.w == 1u {
        at = id.y;
        len = h;
        line = id.x;
        step = w;
    }
    let a = select(0u, at - r, at > r);
    let b = min(at + r + 1u, len);
    // Running sum of the first `k` values of the line.
    var lower = 0.0;
    if a > 0u { lower = prefix[line + (a - 1u) * step]; }
    let upper = prefix[line + (b - 1u) * step];
    blur_dst[id.y * w + id.x] = (upper - lower) / f32(b - a);
}

// Shadows, Highlights (engines before 4), Clarity and Texture as a gain.
struct Sliders {
    count: u32, groups: u32, texture_on: u32, pad: u32,
    exposure: f32, shadows: f32, highlights: f32, clarity: f32,
    texture: f32, pad0: f32, pad1: f32, pad2: f32,
};
@group(0) @binding(4) var<storage, read> logs_in: array<f32>;
@group(0) @binding(5) var<storage, read> fine_in: array<f32>;
@group(0) @binding(6) var<storage, read> broad_in: array<f32>;
@group(0) @binding(7) var<storage, read> texture_in: array<f32>;
@group(0) @binding(8) var<storage, read_write> gain_out: array<f32>;
@group(0) @binding(9) var<uniform> sliders: Sliders;

@compute @workgroup_size(256)
fn local_gain(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) local: u32) {
    let i = (group.y * sliders.groups + group.x) * 256u + local;
    if i >= sliders.count { return; }
    let exposure = sliders.exposure;
    let logs = logs_in[i] + exposure;
    let fine = logs + guide(fine_in[i] + exposure - logs);
    let base = (fine + (logs + guide(broad_in[i] + exposure - logs))) * 0.5;
    let y = exp2(base);
    let shadow = exp(-y * 6.0);
    let high = y / (y + 0.5);
    let clarity = clamp(logs - fine, -1.0, 1.0) * sliders.clarity * 0.6;
    var texture = 0.0;
    if sliders.texture_on != 0u {
        texture = clamp(logs_in[i] - texture_in[i], -0.5, 0.5) * sliders.texture * 0.7;
    }
    gain_out[i] = exp2(
        sliders.shadows * shadow * 2.0 + sliders.highlights * high * 2.0 + clarity + texture,
    );
}
// Range guidance: the part `quality::apply_local`'s `guide` adds to the log value.
fn guide(d: f32) -> f32 {
    return d / (1.0 + d * d);
}

// Sampling parameters, at the `S_*` offsets below; radial tables follow at `S_TABLES`.
@group(0) @binding(10) var<storage, read> photo: array<f32>;
@group(0) @binding(11) var<storage, read> gains: array<f32>;
@group(0) @binding(12) var<storage, read> sp: array<f32>;
@group(0) @binding(13) var<storage, read_write> reduced: array<f32>;
@group(0) @binding(14) var<storage, read_write> samples: array<f32>;
@group(0) @binding(15) var<storage, read_write> positions: array<f32>;

const S_WIDTH: u32 = 0u;
const S_HEIGHT: u32 = 1u;
const S_GAIN: u32 = 2u;
const S_OUT: u32 = 3u; // Output width, height.
const S_REGION: u32 = 5u; // x, y, width, height.
const S_SPREAD: u32 = 9u;
const S_CROP: u32 = 10u;
const S_ORIENTED: u32 = 14u;
const S_ZOOM: u32 = 16u;
const S_SIN: u32 = 17u;
const S_COS: u32 = 18u;
const S_TURNS: u32 = 19u;
const S_FLIP: u32 = 20u;
const S_INSET: u32 = 22u;
const S_TRANSFORM: u32 = 26u;
const S_HOMOGRAPHY: u32 = 27u;
const S_NOISE: u32 = 36u; // luma, chroma, luma detail, chroma detail, luma contrast, chroma smoothness.
const S_LENS: u32 = 42u;
const S_CENTER: u32 = 43u;
const S_HALF: u32 = 45u;
const S_FILL: u32 = 46u;
const S_AMOUNT: u32 = 47u;
const S_DISTORTION: u32 = 48u; // Offset (or -1) and length, for each radial table:
const S_RED: u32 = 50u;
const S_BLUE: u32 = 52u;
const S_VIGNETTING: u32 = 54u;
const S_VIGNETTING_AMOUNT: u32 = 56u;
const S_REDUCED: u32 = 57u; // Reduced width, height.
const S_COUNT: u32 = 59u; // Workgroups per row of the dispatch.
const S_TABLES: u32 = 64u;
const OUTSIDE: f32 = -3e38;

fn s(i: u32) -> f32 {
    return sp[i];
}
fn su(i: u32) -> u32 {
    return u32(sp[i]);
}
fn px(i: u32) -> vec3<f32> {
    let p = vec3(photo[3u * i], photo[3u * i + 1u], photo[3u * i + 2u]);
    if s(S_GAIN) != 0.0 {
        return p * gains[i];
    }
    return p;
}
fn table_eval(field: u32, r: f32) -> f32 {
    let at = u32(s(field));
    let len = su(field + 1u);
    var i = 0u;
    while i < len && sp[at + i] <= r {
        i++;
    }
    if i == 0u {
        return sp[at + len];
    }
    if i == len {
        return sp[at + 2u * len - 1u];
    }
    let t = (r - sp[at + i - 1u]) / (sp[at + i] - sp[at + i - 1u]);
    let a = sp[at + len + i - 1u];
    return a + (sp[at + len + i] - a) * t;
}
fn has(field: u32) -> bool {
    return s(field) >= 0.0;
}
// `pipeline::sample`: bilinear, clamped to the photo.
fn sample(x_in: f32, y_in: f32) -> vec3<f32> {
    let w = su(S_WIDTH);
    let h = su(S_HEIGHT);
    let x = clamp(x_in, 0.0, f32(w - 1u));
    let y = clamp(y_in, 0.0, f32(h - 1u));
    let ix = u32(x);
    let iy = u32(y);
    let fx = x - f32(ix);
    let fy = y - f32(iy);
    let x1 = min(ix + 1u, w - 1u);
    let y1 = min(iy + 1u, h - 1u);
    let a = px(iy * w + ix);
    let b = px(iy * w + x1);
    let c = px(y1 * w + ix);
    let d = px(y1 * w + x1);
    return (a * (1.0 - fx) + b * fx) * (1.0 - fy) + (c * (1.0 - fx) + d * fx) * fy;
}
fn detail_sample(x: f32, y: f32) -> vec3<f32> {
    let p = sample(x, y);
    let luma = s(S_NOISE);
    let chroma = s(S_NOISE + 1u);
    if luma == 0.0 && chroma == 0.0 {
        return p;
    }
    let center = (p.x + 2.0 * p.y + p.z) / 4.0;
    var sum = vec3(0.0);
    var total = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let q = sample(x + f32(dx), y + f32(dy));
            let lum = (q.x + 2.0 * q.y + q.z) / 4.0;
            let detail = (s(S_NOISE + 2u) + s(S_NOISE + 3u)) * 0.5;
            let threshold = 0.0025 * exp2((0.5 - detail) * 4.0);
            let d = lum - center;
            let w = 1.0 / (1.0 + d * d / threshold);
            sum += q * w;
            total += w;
        }
    }
    let avg = sum / total;
    let avgl = (avg.x + 2.0 * avg.y + avg.z) / 4.0;
    return center
        + (avgl - center) * luma * (1.0 - s(S_NOISE + 4u) * 0.5)
        + (p - center) * (1.0 - chroma)
        + (avg - avgl) * chroma * (0.5 + s(S_NOISE + 5u));
}
fn footprint_sample(x: f32, y: f32) -> vec3<f32> {
    let spread = s(S_SPREAD);
    if spread == 0.0 {
        return detail_sample(x, y);
    }
    var sum = vec3(0.0);
    for (var k = 0u; k < 4u; k++) {
        let dx = select(-1.0, 1.0, (k & 1u) == 1u);
        let dy = select(-1.0, 1.0, k >= 2u);
        sum += detail_sample(x + dx * spread, y + dy * spread) * 0.25;
    }
    return sum;
}
// `pipeline::LensWarp::sample`.
fn warp_sample(x: f32, y: f32) -> vec3<f32> {
    let cx = s(S_CENTER);
    let cy = s(S_CENTER + 1u);
    let dx = (x + 0.5 - cx) * s(S_FILL);
    let dy = (y + 0.5 - cy) * s(S_FILL);
    let r = sqrt(dx * dx + dy * dy) / s(S_HALF);
    var g = 1.0;
    if has(S_DISTORTION) {
        g = 1.0 + (table_eval(S_DISTORTION, r) - 1.0) * s(S_AMOUNT);
    }
    var scale = vec3(g);
    if has(S_RED) {
        scale = vec3(g * table_eval(S_RED, r), g, g * table_eval(S_BLUE, r));
    }
    let gx = cx + dx * scale.y - 0.5;
    let gy = cy + dy * scale.y - 0.5;
    var p: vec3<f32>;
    if scale.x == scale.y && scale.z == scale.y {
        p = footprint_sample(gx, gy);
    } else {
        p = vec3(
            footprint_sample(cx + dx * scale.x - 0.5, cy + dy * scale.x - 0.5).x,
            footprint_sample(gx, gy).y,
            footprint_sample(cx + dx * scale.z - 0.5, cy + dy * scale.z - 0.5).z,
        );
    }
    var gain = 1.0;
    if has(S_VIGNETTING) {
        let vx = gx + 0.5 - cx;
        let vy = gy + 0.5 - cy;
        gain = powf(table_eval(S_VIGNETTING, sqrt(vx * vx + vy * vy) / s(S_HALF)), s(S_VIGNETTING_AMOUNT));
    }
    return p * gain;
}
fn powf(x: f32, y: f32) -> f32 {
    if x <= 0.0 {
        return select(0.0, 1.0, y == 0.0);
    }
    return pow(x, y);
}
// `Geometry::source`: output position (u, v in 0..1) to photo pixel coordinates.
fn source(u: f32, v: f32) -> vec2<f32> {
    let ow = s(S_ORIENTED);
    let oh = s(S_ORIENTED + 1u);
    var x = s(S_CROP) + u * (s(S_CROP + 2u) - s(S_CROP));
    var y = s(S_CROP + 1u) + v * (s(S_CROP + 3u) - s(S_CROP + 1u));
    x = (x - 0.5) * ow / s(S_ZOOM);
    y = (y - 0.5) * oh / s(S_ZOOM);
    let sn = s(S_SIN);
    let cs = s(S_COS);
    var nx = (cs * x + sn * y) / ow + 0.5;
    var ny = (-sn * x + cs * y) / oh + 0.5;
    if s(S_TRANSFORM) != 0.0 {
        let long = max(ow, oh);
        let px = (nx - 0.5) * ow / long;
        let py = (ny - 0.5) * oh / long;
        let hm = S_HOMOGRAPHY;
        var w = s(hm + 6u) * px + s(hm + 7u) * py + s(hm + 8u);
        if !(w > 1e-6) { w = 1e-6; }
        nx = (s(hm) * px + s(hm + 1u) * py + s(hm + 2u)) / w * long / ow + 0.5;
        ny = (s(hm + 3u) * px + s(hm + 4u) * py + s(hm + 5u)) / w * long / oh + 0.5;
    }
    if s(S_FLIP) != 0.0 { nx = 1.0 - nx; }
    if s(S_FLIP + 1u) != 0.0 { ny = 1.0 - ny; }
    var ox = nx;
    var oy = ny;
    switch su(S_TURNS) {
        case 1u: { ox = ny; oy = 1.0 - nx; }
        case 2u: { ox = 1.0 - nx; oy = 1.0 - ny; }
        case 3u: { ox = 1.0 - ny; oy = nx; }
        default: {}
    }
    return vec2(
        (s(S_INSET) + ox * s(S_INSET + 2u)) * s(S_WIDTH) - 0.5,
        (s(S_INSET + 1u) + oy * s(S_INSET + 3u)) * s(S_HEIGHT) - 0.5,
    );
}
@compute @workgroup_size(256)
fn sample_region(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) local: u32) {
    let i = (group.y * su(S_COUNT) + group.x) * 256u + local;
    let rw = su(S_REGION + 2u);
    if i >= rw * su(S_REGION + 3u) { return; }
    let x = su(S_REGION) + i % rw;
    let y = su(S_REGION + 1u) + i / rw;
    let at = source((f32(x) + 0.5) / s(S_OUT), (f32(y) + 0.5) / s(S_OUT + 1u));
    let outside = s(S_TRANSFORM) != 0.0
        && (at.x < -0.5 || at.y < -0.5 || at.x > s(S_WIDTH) - 0.5 || at.y > s(S_HEIGHT) - 0.5);
    var p = vec3(1.0);
    var pos = vec2(OUTSIDE);
    if !outside {
        if s(S_LENS) != 0.0 {
            p = warp_sample(at.x, at.y);
        } else {
            p = footprint_sample(at.x, at.y);
        }
        pos = at;
    }
    samples[3u * i] = p.x;
    samples[3u * i + 1u] = p.y;
    samples[3u * i + 2u] = p.z;
    positions[2u * i] = pos.x;
    positions[2u * i + 1u] = pos.y;
}
// `pipeline::preview_source`: box integration of the toned photo.
@compute @workgroup_size(16, 16)
fn reduce_toned(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = su(S_REDUCED);
    let h = su(S_REDUCED + 1u);
    if id.x >= w || id.y >= h { return; }
    let iw = su(S_WIDTH);
    let ih = su(S_HEIGHT);
    let x0 = id.x * iw / w;
    let x1 = max((id.x + 1u) * iw / w, x0 + 1u);
    let y0 = id.y * ih / h;
    let y1 = max((id.y + 1u) * ih / h, y0 + 1u);
    var out = vec3(0.0);
    for (var yy = y0; yy < y1; yy++) {
        for (var xx = x0; xx < x1; xx++) {
            out += px(yy * iw + xx);
        }
    }
    let n = f32((x1 - x0) * (y1 - y0));
    let i = id.y * w + id.x;
    reduced[3u * i] = out.x / n;
    reduced[3u * i + 1u] = out.y / n;
    reduced[3u * i + 2u] = out.z / n;
}
