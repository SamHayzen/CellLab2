// Direct translation of link_vert.glsl and link_frag.glsl.
struct Link {
    first: vec2f,
    second: vec2f,
    left: vec2f,
    right: vec2f,
    first_projection: f32,
    second_projection: f32,
    pad: vec2f,
};
struct View {
    origin: vec2f,
    size: vec2f,
};
@group(0) @binding(0) var<storage, read> links: array<Link>;
@group(0) @binding(1) var<uniform> view: View;

struct Varying {
    @builtin(position) position: vec4f,
    @location(0) tp: vec3f,
};

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> Varying {
    let link = links[instance_index];
    var point: vec2f;
    var tp: vec3f;
    switch vertex_index {
        case 0u: { point = link.first; tp = vec3f(0.0, 1.0, link.first_projection); }
        case 1u: { point = link.left; tp = vec3f(-1.0, 0.0, -1.0); }
        case 2u: { point = link.right; tp = vec3f(1.0, 0.0, 1.0); }
        case 3u: { point = link.second; tp = vec3f(0.0, 1.0, link.second_projection); }
        case 4u: { point = link.left; tp = vec3f(-1.0, 0.0, -1.0); }
        default: { point = link.right; tp = vec3f(1.0, 0.0, 1.0); }
    }
    let normalized = (point - view.origin) / view.size;
    var output: Varying;
    output.position = vec4f(normalized.x * 2.0 - 1.0, 1.0 - normalized.y * 2.0, 0.0, 1.0);
    output.tp = tp;
    return output;
}

@fragment
fn fragment(input: Varying) -> @location(0) vec4f {
    let angle = input.tp.x / (1.0 - input.tp.y);
    let x = mix(angle, input.tp.z, 1.0 - abs(angle) * input.tp.y);
    if fract(x * 3.0 - 0.3) < 0.4 && x < 0.7 && x > -0.7 {
        let alpha = max((input.tp.y - 1.0) * (input.tp.y - 1.0)
                        + input.tp.x * input.tp.x - 0.1, 0.0);
        return vec4f(0.0, 0.0, 0.0, alpha);
    }
    return vec4f(0.0);
}
