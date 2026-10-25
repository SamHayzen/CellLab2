// APK food_vert.glsl and food_frag.glsl, with instanced quads.
struct Food {
    center: vec2f,
    radius: f32,
    pad: f32,
    color: vec4f,
};
struct View {
    origin: vec2f,
    size: vec2f,
};
@group(0) @binding(0) var<storage, read> food: array<Food>;
@group(0) @binding(1) var<uniform> view: View;

struct Varying {
    @builtin(position) position: vec4f,
    @location(0) p: vec2f,
    @location(1) @interpolate(flat) color: vec4f,
};

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> Varying {
    let corners = array<vec2f, 6>(
        vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
        vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0)
    );
    let item = food[instance_index];
    let corner = corners[vertex_index];
    let screen = item.center + corner * item.radius;
    let normalized = (screen - view.origin) / view.size;
    var output: Varying;
    output.position = vec4f(normalized.x * 2.0 - 1.0, 1.0 - normalized.y * 2.0, 0.0, 1.0);
    output.p = corner;
    output.color = item.color;
    return output;
}

@fragment
fn fragment(input: Varying) -> @location(0) vec4f {
    if dot(input.p, input.p) > 1.0 {
        discard;
    }
    return vec4f(input.color.rgb * input.color.a, input.color.a);
}
