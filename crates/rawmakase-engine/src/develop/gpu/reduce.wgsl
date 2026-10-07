// Box reduction of a presented image, for the Navigator and library thumbnails.
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var reduced: texture_storage_2d<rgba8unorm, write>;
@compute @workgroup_size(16, 16)
fn reduce(@builtin(global_invocation_id) id: vec3<u32>) {
    let from_size = textureDimensions(source);
    let to_size = textureDimensions(reduced);
    if id.x >= to_size.x || id.y >= to_size.y { return; }
    let x0 = id.x * from_size.x / to_size.x;
    let x1 = max((id.x + 1u) * from_size.x / to_size.x, x0 + 1u);
    let y0 = id.y * from_size.y / to_size.y;
    let y1 = max((id.y + 1u) * from_size.y / to_size.y, y0 + 1u);
    var sum = vec4(0.0);
    for (var y = y0; y < y1; y++) {
        for (var x = x0; x < x1; x++) {
            sum += textureLoad(source, vec2(x, y), 0);
        }
    }
    textureStore(reduced, vec2(id.x, id.y), sum / f32((x1 - x0) * (y1 - y0)));
}
