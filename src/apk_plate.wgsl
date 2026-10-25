// Direct WGSL translation of bg_vert.glsl and bg_frag.glsl from the APK.

struct Plate {
    origin: vec2f,
    size: vec2f,
    center: vec2f,
    radius: f32,
    padding: f32,
    dir: vec4f,
};

@group(0) @binding(0) var<uniform> plate: Plate;

struct Varying {
    @builtin(position) position: vec4f,
    @location(0) p: vec2f,
};

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32) -> Varying {
    let corners = array<vec2f, 6>(
        vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
        vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0)
    );
    let p = corners[vertex_index];
    let point = plate.center + p * plate.radius;
    let normalized = (point - plate.origin) / plate.size;
    var output: Varying;
    output.position = vec4f(normalized.x * 2.0 - 1.0, 1.0 - normalized.y * 2.0, 0.0, 1.0);
    output.p = p;
    return output;
}

@fragment
fn fragment(input: Varying) -> @location(0) vec4f {
    let ds0 = dot(input.p, input.p);
    if ds0 > 1.02 {
        discard;
    }
    if ds0 > 0.999 {
        return vec4f(0.0);
    }
    let range = max(plate.dir.z, 0.001);
    let ds = 1.0 + (1.0 - range) * (1.0 - ds0) / range;
    let py = dot(plate.dir.xy, input.p);
    let light = plate.dir.w * max((py * (1.0 - range) + range) / (ds * ds), 0.0);
    return vec4f(light * 0.5, light * 0.35, light * 0.25, 1.0);
}
