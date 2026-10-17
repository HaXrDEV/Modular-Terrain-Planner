// Copies the resolved scene and draws a screen-space outline around the
// pixels of the selection mask.

struct Params {
    outline: vec4<f32>,
    // x: thickness in physical pixels
    params: vec4<f32>,
};

@group(0) @binding(0) var scene_tex: texture_2d<f32>;
@group(0) @binding(1) var mask_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> p: Params;

@vertex
fn vs_full(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = vec2<i32>(pos.xy);
    let color = textureLoad(scene_tex, c, 0);
    if textureLoad(mask_tex, c, 0).r > 0.5 {
        return color;
    }
    let size = vec2<i32>(textureDimensions(mask_tex)) - vec2<i32>(1);
    let t = i32(p.params.x);
    let dirs = array<vec2<i32>, 8>(
        vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1),
        vec2<i32>(1, 1), vec2<i32>(-1, 1), vec2<i32>(1, -1), vec2<i32>(-1, -1),
    );
    for (var s = 1; s <= t; s++) {
        for (var d = 0; d < 8; d++) {
            let q = clamp(c + dirs[d] * s, vec2<i32>(0), size);
            if textureLoad(mask_tex, q, 0).r > 0.5 {
                return p.outline;
            }
        }
    }
    return color;
}
