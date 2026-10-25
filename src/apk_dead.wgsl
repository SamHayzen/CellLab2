// Direct translation of the APK's dead_vert.glsl and dead_frag.glsl.
struct Particle {
    center: vec2f,
    quad_radius: f32,
    world_half: f32,
    color: vec4f,
    age: f32,
    radius: f32,
    pad: vec2f,
};
struct View {
    origin: vec2f,
    size: vec2f,
};
@group(0) @binding(0) var<storage, read> particles: array<Particle>;
@group(0) @binding(1) var<uniform> view: View;
@group(0) @binding(2) var lookup: texture_2d<f32>;
@group(0) @binding(3) var lookup_sampler: sampler;

struct Varying {
    @builtin(position) position: vec4f,
    @location(0) p: vec2f,
    @location(1) @interpolate(flat) color: vec3f,
    @location(2) @interpolate(flat) age: f32,
    @location(3) @interpolate(flat) radius: f32,
};

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> Varying {
    let corners = array<vec2f, 6>(
        vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
        vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0)
    );
    let particle = particles[instance_index];
    let corner = corners[vertex_index];
    let screen = particle.center + corner * particle.quad_radius;
    let normalized = (screen - view.origin) / view.size;
    var output: Varying;
    output.position = vec4f(normalized.x * 2.0 - 1.0, 1.0 - normalized.y * 2.0, 0.0, 1.0);
    output.p = corner * particle.world_half;
    output.color = particle.color.rgb;
    output.age = particle.age;
    output.radius = particle.radius;
    return output;
}

@fragment
fn fragment(input: Varying) -> @location(0) vec4f {
    let inverse_radius = 1.0 / max(input.radius, 0.000001);
    let distance_squared = dot(input.p, input.p);
    let st = sqrt(max(input.age, 0.0)) * 0.105185;
    let uv = vec2f(distance_squared * inverse_radius * inverse_radius * 0.22596,
                   st * inverse_radius * 0.285212);
    let rho = textureSample(lookup, lookup_sampler, uv).r;
    let alpha = 0.7 * rho;
    return vec4f(0.8 * input.color * alpha, alpha);
}
