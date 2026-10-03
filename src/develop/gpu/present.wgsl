// Preview finishing after the develop stage, on the developed pixels still on the
// device: sharpening, then vignettes and grain, the clipping overlay, the monitor
// profile and 8-bit encoding into the texture the viewport draws. Ports of
// `quality::sharpen_with_radius`, `effects::spatial_finish_scaled`, `Rendered::rgb8`
// and `Rendered::histogram`; the CPU versions are the reference.
struct Params {
    // Developed buffer size, and the rectangle of it that is shown.
    width: u32, height: u32, crop_x: u32, crop_y: u32,
    crop_w: u32, crop_h: u32, radius: u32, sharpen: u32,
    amount: f32, threshold: f32, clipping: u32, lut_size: u32,
    // Output pixel of the buffer's first pixel, and the whole output's size.
    origin_x: u32, origin_y: u32, full_w: u32, full_h: u32,
    scale: f32, grain: f32, grain_size: f32, grain_roughness: f32,
    grain_seed: u32, vignette: f32, vignette_roundness: f32, vignette_midpoint: f32,
    vignette_feather: f32, vignette_highlights: f32, vignette_blend: u32, lens_vignette: f32,
    lens_vignette_midpoint: f32, effects: u32, count: u32, pad: u32,
};
@group(0) @binding(0) var<storage, read_write> pixels: array<f32>;
@group(0) @binding(1) var<storage, read_write> scratch: array<f32>;
@group(0) @binding(2) var<storage, read> weights: array<f32>;
@group(0) @binding(3) var<uniform> p: Params;
@group(0) @binding(4) var<storage, read> lut: array<f32>;
@group(0) @binding(5) var<storage, read_write> histogram: array<atomic<u32>>;
@group(0) @binding(6) var shown: texture_storage_2d<rgba8unorm, write>;

fn rgb(i: u32) -> vec3<f32> {
    return vec3(pixels[3u * i], pixels[3u * i + 1u], pixels[3u * i + 2u]);
}
fn luminance(v: vec3<f32>) -> f32 {
    return 0.2126 * v.r + 0.7152 * v.g + 0.0722 * v.b;
}
@compute @workgroup_size(16, 16)
fn blur_horizontal(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.width || id.y >= p.height { return; }
    var value = 0.0;
    for (var k = 0u; k <= 2u * p.radius; k++) {
        let x = u32(clamp(i32(id.x) + i32(k) - i32(p.radius), 0, i32(p.width) - 1));
        value += luminance(rgb(id.y * p.width + x)) * weights[k];
    }
    scratch[id.y * p.width + id.x] = value;
}
@compute @workgroup_size(16, 16)
fn sharpen(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.width || id.y >= p.height { return; }
    var blur = 0.0;
    for (var k = 0u; k <= 2u * p.radius; k++) {
        let y = u32(clamp(i32(id.y) + i32(k) - i32(p.radius), 0, i32(p.height) - 1));
        blur += scratch[y * p.width + id.x] * weights[k];
    }
    let i = id.y * p.width + id.x;
    let color = rgb(i);
    let d = luminance(color) - blur;
    var mask = 1.0;
    if p.threshold != 0.0 { mask = clamp(abs(d) / p.threshold, 0.0, 1.0); }
    let delta = clamp(d * p.amount * 2.0 * mask, -0.08, 0.08);
    let result = clamp(color + vec3(delta), vec3(0.0), vec3(1.0));
    pixels[3u * i] = result.r;
    pixels[3u * i + 1u] = result.g;
    pixels[3u * i + 2u] = result.b;
}

// `effects::hash`: integer hash to [-1, 1].
fn hash(x: i32, y: i32, seed: u32) -> f32 {
    var v = (bitcast<u32>(x) * 0x9e3779b9u) ^ (bitcast<u32>(y) * 0x85ebca6bu) ^ seed;
    v ^= v >> 16u;
    v *= 0x7feb352du;
    v ^= v >> 15u;
    v *= 0x846ca68bu;
    v ^= v >> 16u;
    return f32(v) / 4294967295.0 * 2.0 - 1.0;
}
fn grain_noise(x: f32, y: f32, size: f32, seed: u32) -> f32 {
    let sx = x / size;
    let sy = y / size;
    let ix = i32(floor(sx));
    let iy = i32(floor(sy));
    let fa = sx - f32(ix);
    let fb = sy - f32(iy);
    let a = fa * fa * (3.0 - 2.0 * fa);
    let b = fb * fb * (3.0 - 2.0 * fb);
    let n = hash(ix, iy, seed) * (1.0 - a) + hash(ix + 1, iy, seed) * a;
    let m = hash(ix, iy + 1, seed) * (1.0 - a) + hash(ix + 1, iy + 1, seed) * a;
    return n * (1.0 - b) + m * b;
}
// Rust's `round`: halves away from zero (WGSL's `round` goes to even).
fn round_away(v: f32) -> f32 {
    return sign(v) * floor(abs(v) + 0.5);
}
// `pow` for a non-negative base, which WGSL leaves undefined at zero.
fn power(base: f32, exponent: f32) -> f32 {
    if base <= 0.0 { return 0.0; }
    return pow(base, exponent);
}
fn spatial(color: vec3<f32>, x: u32, y: u32) -> vec3<f32> {
    var c = color;
    let nx = ((f32(x) + 0.5) / f32(p.full_w) - 0.5) * 2.0;
    let ny = ((f32(y) + 0.5) / f32(p.full_h) - 0.5) * 2.0;
    let shape = exp2(-p.vignette_roundness * 1.5 + 1.0);
    let distance = power(power(abs(nx), shape) + power(abs(ny), shape), 1.0 / shape);
    let start = p.vignette_midpoint * 0.9;
    let feather = max(p.vignette_feather * 0.9 + 0.05, 0.05);
    let t = clamp((distance - start) / feather, 0.0, 1.0);
    let mask = t * t * (3.0 - 2.0 * t);
    let l = luminance(c);
    let protect = 1.0 - p.vignette_highlights * (l * l * l * l);
    if p.vignette_blend == 1u {
        var toward = 1.0;
        if p.vignette < 0.0 { toward = 0.0; }
        c += (vec3(toward) - c) * abs(p.vignette) * mask * protect;
    } else {
        c *= exp2(p.vignette * mask * protect * 2.0);
    }
    let lens = clamp(
        max(nx * nx + ny * ny - p.lens_vignette_midpoint, 0.0) / (2.0 - p.lens_vignette_midpoint),
        0.0,
        1.0,
    );
    let gain = exp2(-p.lens_vignette * lens * 2.0);
    let size = 0.75 + p.grain_size * 5.0;
    var gx = f32(x);
    var gy = f32(y);
    if p.scale != 1.0 {
        gx = (f32(x) + 0.5) / p.scale - 0.5;
        gy = (f32(y) + 0.5) / p.scale - 0.5;
    }
    let coarse = grain_noise(gx, gy, size, p.grain_seed) * min(size * p.scale, 1.0) / min(size, 1.0);
    let fine = hash(i32(round_away(gx)), i32(round_away(gy)), p.grain_seed ^ 0x21f09u)
        * min(p.scale, 1.0);
    let noise = (coarse * (1.0 - p.grain_roughness) + fine * p.grain_roughness) * p.grain * 0.13
        * max(4.0 * l * (1.0 - l), 0.2);
    return clamp(c * gain + vec3(noise), vec3(0.0), vec3(1.0));
}
// The monitor profile as a lattice over 8-bit input, `lut_size` points per axis at
// equal byte steps, interpolated trilinearly.
fn lut_at(r: u32, g: u32, b: u32) -> vec3<f32> {
    let i = 3u * ((r * p.lut_size + g) * p.lut_size + b);
    return vec3(lut[i], lut[i + 1u], lut[i + 2u]);
}
fn monitor(bytes: vec3<f32>) -> vec3<f32> {
    let pos = bytes / 255.0 * f32(p.lut_size - 1u);
    let i = min(vec3<u32>(floor(pos)), vec3(p.lut_size - 2u));
    let f = pos - vec3<f32>(i);
    let c00 = mix(lut_at(i.x, i.y, i.z), lut_at(i.x + 1u, i.y, i.z), f.x);
    let c10 = mix(lut_at(i.x, i.y + 1u, i.z), lut_at(i.x + 1u, i.y + 1u, i.z), f.x);
    let c01 = mix(lut_at(i.x, i.y, i.z + 1u), lut_at(i.x + 1u, i.y, i.z + 1u), f.x);
    let c11 = mix(lut_at(i.x, i.y + 1u, i.z + 1u), lut_at(i.x + 1u, i.y + 1u, i.z + 1u), f.x);
    return floor(mix(mix(c00, c10, f.y), mix(c01, c11, f.y), f.z) + 0.5);
}
@compute @workgroup_size(16, 16)
fn present(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.crop_w || id.y >= p.crop_h { return; }
    let bx = p.crop_x + id.x;
    let by = p.crop_y + id.y;
    var color = rgb(by * p.width + bx);
    if p.effects != 0u {
        color = spatial(color, p.origin_x + bx, p.origin_y + by);
    }
    let clamped = clamp(color, vec3(0.0), vec3(1.0));
    if p.count != 0u {
        let bins = vec3<u32>(clamped * 255.0);
        atomicAdd(&histogram[bins.r], 1u);
        atomicAdd(&histogram[256u + bins.g], 1u);
        atomicAdd(&histogram[512u + bins.b], 1u);
    }
    var bytes = floor(clamped * 255.0 + 0.5);
    if p.lut_size > 1u {
        bytes = monitor(bytes);
    }
    if p.clipping != 0u {
        if any(color >= vec3(0.999)) {
            bytes = vec3(255.0, 40.0, 40.0);
        } else if all(color <= vec3(0.001)) {
            bytes = vec3(40.0, 80.0, 255.0);
        }
    }
    textureStore(shown, vec2(id.x, id.y), vec4(bytes / 255.0, 1.0));
}
