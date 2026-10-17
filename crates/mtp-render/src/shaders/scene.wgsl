struct Globals {
    view_proj: mat4x4<f32>,
    // xy: grid size in cells, z: grid line strength, w: pixels per point
    grid: vec4<f32>,
    ground_color: vec4<f32>,
    line_color: vec4<f32>,
    // x, y, width, height of the battle map in cells
    image_rect: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(1) @binding(0) var image_tex: texture_2d<f32>;
@group(1) @binding(1) var image_sampler: sampler;

struct VertexIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct InstanceIn {
    @location(2) offset: vec3<f32>,
    @location(3) rot: f32,
    @location(4) scale: vec3<f32>,
    // 0 tile, 1 ghost, 2 refused ghost, 3 placeholder for a missing STL
    @location(5) kind: f32,
    @location(6) color: vec4<f32>,
};

struct TileOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) color: vec4<f32>,
    @location(2) kind: f32,
    // Position inside the turned footprint, in cells, for edge lines
    @location(3) cell: vec2<f32>,
    @location(4) size: vec2<f32>,
};

fn rot_z(p: vec2<f32>, a: f32) -> vec2<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec2<f32>(c * p.x - s * p.y, s * p.x + c * p.y);
}

// The model transform shared with picking and the Python version: turn
// inside the unit square, scale to the turned footprint, move into place.
@vertex
fn vs_tile(v: VertexIn, i: InstanceIn) -> TileOut {
    let local = vec3<f32>(rot_z(v.pos.xy - vec2<f32>(0.5), i.rot) + vec2<f32>(0.5), v.pos.z);
    let world = i.offset + local * i.scale;
    let n = vec3<f32>(rot_z(v.normal.xy, i.rot), v.normal.z) / i.scale;
    var out: TileOut;
    out.clip = g.view_proj * vec4<f32>(world, 1.0);
    out.normal = normalize(n);
    out.color = i.color;
    out.kind = i.kind;
    out.cell = local.xy * i.scale.xy;
    out.size = i.scale.xy;
    return out;
}

fn shade(n: vec3<f32>, base: vec3<f32>) -> vec3<f32> {
    let key = max(dot(n, normalize(vec3<f32>(-0.3, -0.5, 1.0))), 0.0) * 0.6;
    let fill = max(dot(n, normalize(vec3<f32>(0.8, 0.4, 0.3))), 0.0) * 0.25;
    let rim = max(dot(n, normalize(vec3<f32>(0.2, 0.9, -0.3))), 0.0) * 0.1;
    return base * min(0.2 + key + fill + rim, 1.0);
}

@fragment
fn fs_tile(in: TileOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // STL winding can't be trusted, so light whichever side faces us
    let n = select(-in.normal, in.normal, front);
    var rgb = shade(n, in.color.rgb);
    // A thin darker line round the top of each tile, so neighbours stay
    // apart when seen from above
    if abs(n.z) > 0.5 {
        let d = min(min(in.cell.x, in.size.x - in.cell.x), min(in.cell.y, in.size.y - in.cell.y));
        let w = max(fwidth(d), 1e-5);
        let edge = 1.0 - smoothstep(0.5 * w, 1.5 * w * g.grid.w, d);
        rgb = rgb * (1.0 - 0.45 * edge);
    }
    // Refused ghosts and missing tiles are hatched, so colour isn't the
    // only signal
    if in.kind > 1.5 {
        let stripe = fract((in.clip.x + in.clip.y) / (10.0 * g.grid.w));
        if stripe < 0.5 {
            rgb = rgb * 0.55;
        }
    }
    return vec4<f32>(rgb, in.color.a);
}

@fragment
fn fs_mask(in: TileOut) -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 0.0, 1.0);
}

struct GroundOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec2<f32>,
    @location(1) uv: vec2<f32>,
};

fn quad(vi: u32) -> vec2<f32> {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    return corners[vi];
}

@vertex
fn vs_ground(@builtin(vertex_index) vi: u32) -> GroundOut {
    let c = quad(vi);
    let p = c * g.grid.xy;
    var out: GroundOut;
    out.clip = g.view_proj * vec4<f32>(p, 0.0, 1.0);
    out.world = p;
    out.uv = c;
    return out;
}

@fragment
fn fs_ground(in: GroundOut) -> @location(0) vec4<f32> {
    return vec4<f32>(g.ground_color.rgb, 1.0);
}

@vertex
fn vs_image(@builtin(vertex_index) vi: u32) -> GroundOut {
    let c = quad(vi);
    let p = g.image_rect.xy + c * g.image_rect.zw;
    var out: GroundOut;
    out.clip = g.view_proj * vec4<f32>(p, 0.0, 1.0);
    out.world = p;
    // Images are stored top row first; the grid's y points up
    out.uv = vec2<f32>(c.x, 1.0 - c.y);
    return out;
}

@fragment
fn fs_image(in: GroundOut) -> @location(0) vec4<f32> {
    return textureSample(image_tex, image_sampler, in.uv);
}

// Anti-aliased grid lines, with a stronger line every 2 cells so 1" squares
// stand out.
fn grid_line(p: vec2<f32>) -> f32 {
    let w = fwidth(p);
    let d = abs(fract(p - 0.5) - 0.5) / max(w, vec2<f32>(1e-6));
    return 1.0 - min(min(d.x, d.y), 1.0);
}

@fragment
fn fs_grid(in: GroundOut) -> @location(0) vec4<f32> {
    let w = fwidth(in.world);
    // Fade the cell lines out before they turn into a grey blur
    let fade = clamp(1.5 - max(w.x, w.y) * 4.0, 0.0, 1.0);
    let fine = grid_line(in.world) * 0.22 * fade;
    let inch = grid_line(in.world * 0.5) * 0.6;
    let edge = 1.0 - min(min(in.world.x, g.grid.x - in.world.x), min(in.world.y, g.grid.y - in.world.y)) / max(w.x, 1e-6);
    let k = max(max(fine, inch), clamp(edge, 0.0, 1.0)) * g.grid.z;
    return vec4<f32>(g.line_color.rgb, k);
}
