// Direct WGSL translation of the APK's cell_frag.glsl, lip_frag.glsl,
// ker_frag.glsl and gamete_frag.glsl. The original vertex shaders multiply
// positions and radii by 50 before evaluating these fragment equations.

struct Cell {
    center: vec2f,
    radius: f32,
    scale: f32,
    color: vec4f,
    kind: u32,
    angle: f32,
    phase: f32,
    reserve: f32,
    signals: vec2f,
    activity: vec2f,
    animation_phase: f32,
    padding0: f32,
    padding1: f32,
    padding2: f32,
    auxiliary: vec4f,
    clip_start: u32,
    clip_count: u32,
    clip_padding0: u32,
    clip_padding1: u32,
};

struct View {
    origin: vec2f,
    size: vec2f,
};

@group(0) @binding(0) var<storage, read> cells: array<Cell>;
@group(0) @binding(1) var<uniform> view: View;
@group(0) @binding(2) var photo_texture: texture_2d<f32>;
@group(0) @binding(3) var photo_sampler: sampler;
@group(0) @binding(4) var<storage, read> clip_planes: array<vec4f>;

struct Varying {
    @builtin(position) position: vec4f,
    @location(0) p: vec2f,
    @location(1) @interpolate(flat) color: vec3f,
    @location(2) @interpolate(flat) radius: f32,
    @location(3) @interpolate(flat) kind: u32,
    @location(4) @interpolate(flat) angle: f32,
    @location(5) @interpolate(flat) phase: f32,
    @location(6) @interpolate(flat) reserve: f32,
    @location(7) @interpolate(flat) signals: vec2f,
    @location(8) @interpolate(flat) activity: vec2f,
    @location(9) @interpolate(flat) animation_phase: f32,
    @location(10) @interpolate(flat) auxiliary: vec4f,
    @location(11) @interpolate(flat) clip_start: u32,
    @location(12) @interpolate(flat) clip_count: u32,
};

@vertex
fn vertex(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> Varying {
    let corners = array<vec2f, 6>(
        vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
        vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0)
    );
    let cell = cells[instance_index];
    let corner = corners[vertex_index];
    let point = cell.center + corner * cell.radius;
    let normalized = (point - view.origin) / view.size;
    var output: Varying;
    output.position = vec4f(normalized.x * 2.0 - 1.0, 1.0 - normalized.y * 2.0, 0.0, 1.0);
    output.p = corner * (cell.radius / cell.scale) * 50.0;
    output.color = cell.color.rgb;
    output.radius = (cell.radius / cell.scale) * 50.0;
    output.kind = cell.kind;
    output.angle = cell.angle;
    output.phase = cell.phase;
    output.reserve = cell.reserve;
    output.signals = cell.signals;
    output.activity = cell.activity;
    output.animation_phase = cell.animation_phase;
    output.auxiliary = cell.auxiliary;
    output.clip_start = cell.clip_start;
    output.clip_count = cell.clip_count;
    return output;
}

@fragment
fn fragment(input: Varying) -> @location(0) vec4f {
    let NR = 0.25;  // .005 * 50
    let EW = 0.1;   // .002 * 50
    let EWH = 0.05; // .001 * 50
    let ds = dot(input.p, input.p);
    let r = input.radius;
    if ds > r * r {
        discard;
    }

    // CellWorld records the shared chord of each overlapping pair. Keep the
    // half of this body's disk on its side of every chord and shade the cut
    // edge with the same membrane width as the outer circumference.
    var contact_tp = 0.0;
    for (var i = 0u; i < input.clip_count; i += 1u) {
        let chord = clip_planes[input.clip_start + i];
        let projected = dot(input.p, chord.xy);
        if projected > chord.z {
            discard;
        }
        if chord.z > 0.000001 {
            contact_tp = max(contact_tp, clamp(r * projected / chord.z, 0.0, r));
        }
    }
    let warped_p = input.p * max(contact_tp / sqrt(max(ds, 0.000001)), 1.0);

    var rgb = 0.5 * input.color;
    var alpha = 1.0;
    let inner = max(abs(r - EW), 0.000001);
    var srr = ds / (inner * inner);

    if input.kind == 2u {
        // photo_frag.glsl samples two separately rotated channels from
        // the APK's original small_dots.png texture.
        srr = max(srr, EW / max(EWH, r - contact_tp));
        if ds > NR * NR && srr < 1.0 {
            let stc = 0.5 * input.p / r * sqrt(srr);
            let c1 = cos(input.angle);
            let s1 = -sin(input.angle);
            let c2 = cos(input.phase);
            let s2 = -sin(input.phase);
            let uv1 = vec2f(0.5, 0.5) + vec2f(stc.x * c1 - stc.y * s1, stc.x * s1 + stc.y * c1);
            let uv2 = vec2f(0.5, 0.5) + vec2f(stc.x * c2 - stc.y * s2, stc.x * s2 + stc.y * c2);
            let pl = textureSampleLevel(photo_texture, photo_sampler, uv1, 0.0).r
                + textureSampleLevel(photo_texture, photo_sampler, uv2, 0.0).g;
            if pl > 0.5 {
                rgb = vec3f(0.0, 0.6, 0.1);
                alpha = 1.0;
            } else {
                rgb = input.color;
                alpha = 0.4;
            }
        }
    } else if input.kind == 6u {
        // buoy_frag.glsl: shaped air cavity and the blue channel of
        // small_dots.png, evaluated in the original cell-local coordinates.
        srr = max(srr, EW / max(EWH, r - contact_tp));
        let orientation = vec2f(cos(input.angle), sin(input.angle));
        let stc = warped_p / r;
        let shifted = stc - orientation * 0.55;
        if dot(shifted, shifted) * r * r > NR * NR && srr < 1.0 {
            let offset = stc - orientation * 0.6;
            let cavity_r = length(stc) + 0.1 / max(dot(offset, offset), 0.000001);
            if cavity_r < 0.9 {
                let x = dot(stc, orientation);
                let y = dot(stc, vec2f(orientation.y, -orientation.x));
                let xy_length = sqrt(max(x * x + y * y, 0.000001));
                let pole_distance = max((x - 0.6) * (x - 0.6) + y * y, 0.000001);
                let gx = x / xy_length - 0.2 * (x - 0.6) / (pole_distance * pole_distance);
                let gy = y * (1.0 / xy_length - 0.2 / (pole_distance * pole_distance));
                let grad = sqrt(gx * gx + gy * gy);
                let inner = 0.9 - 0.08 * grad;
                if cavity_r < inner {
                    let dir1 = input.p / r + orientation * 0.3;
                    let second_angle = vec2f(cos(input.phase), sin(input.phase));
                    let dir = vec2f(
                        dot(dir1, second_angle),
                        dir1.x * second_angle.y - dir1.y * second_angle.x
                    );
                    let s = cavity_r * cavity_r / max(inner * inner, 0.000001);
                    let tr = (1.0 - s) + s * cavity_r / max(inner * length(dir), 0.000001);
                    let blue = textureSampleLevel(photo_texture, photo_sampler, vec2f(0.5, 0.5) + 0.5 * dir * tr, 0.0).b;
                    rgb = input.color * 0.75;
                    alpha = select(0.0, 0.7, blue > 0.2);
                } else {
                    rgb = input.color * 0.5;
                    alpha = 1.0;
                }
            } else {
                rgb = input.color;
                alpha = 0.4;
            }
        }
    } else if input.kind == 14u {
        // ves_frag.glsl: moving secretion vesicles arranged on two offset
        // logarithmic lattices, with transparent centers.
        srr = max(srr, EW / max(EWH, r - contact_tp));
        if length(warped_p) > r {
            discard;
        }
        if ds > NR * NR {
            let distance = length(warped_p);
            let ratio = 1.7320508075688772;
            let inv_scale = 9.0 * ratio * 0.5 / 3.141592653589793;
            let rho = log(distance / r) * inv_scale - 0.2 * input.animation_phase;
            let theta = (atan2(warped_p.x, warped_p.y) + input.angle) / ratio * inv_scale;
            let p1 = vec2f(fract(rho), fract(theta) * ratio) - vec2f(0.5, 0.5 * ratio);
            let p2 = vec2f(fract(rho - 0.5), fract(theta - 0.5) * ratio) - vec2f(0.5, 0.5 * ratio);
            let minr = min(dot(p1, p1), dot(p2, p2));
            if minr < 0.2 * 0.2 {
                rgb = 0.5 * input.color;
                let comp = max(0.2 - 0.056 / distance, 0.0);
                alpha = select(1.0, 0.0, minr < comp * comp);
            } else if srr < 1.0 {
                rgb = input.color;
                alpha = 0.5;
            }
        }
    } else if input.kind == 15u {
        // stem_frag.glsl: red and blue growth vesicles are placed from the
        // cell's two gene controlled stem outputs.
        srr = max(srr, EW / max(EWH, r - contact_tp));
        if length(warped_p) > r {
            discard;
        }
        if ds > NR * NR && srr < 1.0 {
            let distance = length(warped_p);
            let rho = log(max((r - NR) / max(distance - NR, 0.000001), 0.000001));
            var theta = atan2(warped_p.x, warped_p.y) + input.angle;
            if theta > 3.141592653589793 {
                theta -= 6.283185307179586;
            }
            if theta < -3.141592653589793 {
                theta += 6.283185307179586;
            }
            var opposite = theta + 3.141592653589793;
            if theta > 0.0 {
                opposite = theta - 3.141592653589793;
            }
            let red_offset = vec2f(rho - input.activity.x, opposite);
            let blue_offset = vec2f(rho - input.activity.y, theta);
            if dot(red_offset, red_offset) < input.activity.x * input.activity.x {
                rgb = input.color * 0.75 + vec3f(0.3, 0.0, 0.0);
                alpha = 1.0;
            } else if dot(blue_offset, blue_offset) < input.activity.y * input.activity.y {
                rgb = input.color * 0.75 + vec3f(0.0, 0.0, 0.3);
                alpha = 1.0;
            } else {
                rgb = input.color;
                alpha = 0.5;
            }
        }
    } else if input.kind == 12u {
        // muscle_frag.glsl: the contracting four-band pattern follows the
        // two saved muscle activity values (Cell.aa and Cell.ab).
        srr = max(srr, EW / max(EWH, r - contact_tp));
        if ds > NR * NR && srr < 1.0 {
            let orientation = vec2f(cos(input.angle), sin(input.angle));
            let x = dot(warped_p, orientation);
            let y = dot(warped_p, vec2f(orientation.y, -orientation.x));
            let bend = input.activity.y * 0.7;
            let stretch = (1.0 + 0.4 * input.activity.x) / sqrt(max(1.0 - bend * bend, 0.000001));
            let denominator = max(abs(r - EW), 0.000001);
            let x1 = x / denominator * stretch;
            let y1 = (y / denominator + bend) * stretch;
            let bend_denominator = max(abs(1.0 - 1.5 * bend * bend), 0.000001);
            let x2 = (atan2(2.0 * y1, 1.0 - x1 * x1 - y1 * y1) - 2.0 * bend * stretch) / bend_denominator;
            let d1 = (x1 - 1.0) * (x1 - 1.0) + y1 * y1;
            let d2 = (x1 + 1.0) * (x1 + 1.0) + y1 * y1;
            let y2 = -0.5 * log(max(d1 / max(d2, 0.000001), 0.000001));
            let factor = 8.4 / 3.141592653589793;
            let v = fract(clamp(x2 * factor, -4.0, 4.0));
            let w = abs(sin(2.0 * y2 * factor)) * 0.1;
            if v > 0.3 - w && v < 0.7 + w {
                rgb = 0.75 * input.color;
                alpha = 1.0;
            } else {
                rgb = input.color;
                alpha = 0.5;
            }
        }
    } else if input.kind == 10u || input.kind == 11u || input.kind == 13u {
        // nerve_frag.glsl: the two alternating signal marks are driven by
        // Cell.V[0] and Cell.V[1], copied from the cell's live W signals.
        srr = max(srr, EW / max(EWH, r - contact_tp));
        if ds > NR * NR {
            let pr = length(warped_p);
            let rho = (pr - NR) / max(abs(r - NR - EW), 0.000001) - 0.5;
            let theta = (atan2(input.p.x, input.p.y) + input.angle) * (3.0 / 3.141592653589793);
            let part = floor(theta);
            let p1 = vec2f(rho, (theta - part - 0.5) * 2.0);
            let minr = dot(p1, p1);
            if minr < 0.4 * 0.4 {
                let comp = max(0.4 - 0.056 / pr, 0.0);
                if minr < comp * comp {
                    let parity = part - 2.0 * floor(part / 2.0);
                    var signal: f32;
                    if parity < 0.5 {
                        signal = input.signals.x * 0.05;
                        if signal > 0.0 {
                            rgb = vec3f(0.83, 0.7, 0.0);
                        } else {
                            rgb = vec3f(0.83, 0.0, 0.7);
                        }
                    } else {
                        signal = input.signals.y * 0.05;
                        if signal > 0.0 {
                            rgb = vec3f(0.42, 0.7, 1.0);
                        } else {
                            rgb = vec3f(0.7, 1.0, 0.42);
                        }
                    }
                    alpha = min(abs(signal), 1.0);
                }
            } else if srr < 1.0 {
                rgb = input.color;
                alpha = 0.5;
            }
        }
    } else if input.kind == 4u || input.kind == 5u {
        // lip_frag.glsl and ker_frag.glsl
        srr = max(srr, EW / max(EWH, r - contact_tp));
        if ds > NR * NR && srr < 1.0 {
            rgb = input.color;
            alpha = 0.5;
            if input.kind == 4u {
                if srr < 0.3 + 0.7 * (input.reserve / 1.8) {
                    rgb = vec3f(0.55, 0.4, 0.3) * 0.7 + input.color * 0.3;
                    alpha = 1.0;
                }
            }
        }
        if input.kind == 5u {
            let inner2 = max(abs(r - 2.0 * EW), 0.000001);
            let inner3 = max(abs(r - 3.0 * EW), 0.000001);
            let srr2 = max(ds / (inner2 * inner2), 2.0 * EW / max(EWH, r - contact_tp));
            let srr3 = max(ds / (inner3 * inner3), 3.0 * EW / max(EWH, r - contact_tp));
            if srr2 < 1.0 && srr3 > 1.0 {
                rgb = 0.5 * input.color;
                alpha = 1.0;
            }
        }
    } else if input.kind == 16u {
        // gamete_frag.glsl: its membrane width varies with the seventh
        // angular harmonic, and its nucleus is open on one side.
        let orientation = vec2f(cos(input.angle), sin(input.angle));
        let a = dot(input.p, orientation) / r;
        let b = dot(input.p, vec2f(-orientation.y, orientation.x)) / r;
        let a2 = a * a;
        let a3 = a2 * a;
        let a5 = a3 * a2;
        let a7 = a5 * a2;
        let b2 = b * b;
        let b4 = b2 * b2;
        let b6 = b4 * b2;
        let harmonic = a7 - 21.0 * a5 * b2 + 35.0 * a3 * b4 - 7.0 * a * b6;
        let ew = EW * (1.1 + 5.0 * harmonic * harmonic);
        let gamete_inner = max(abs(r - ew), 0.000001);
        srr = max(ds / (gamete_inner * gamete_inner), ew / max(ew * 0.5, r - contact_tp));
        let forward = vec2f(cos(input.phase), sin(input.phase));
        if (ds > NR * NR || dot(input.p, forward) > 0.0) && srr < 1.0 {
            rgb = input.color;
            alpha = 0.4;
        }
    } else {
        // cell_frag.glsl, shared by phagocytes, flagellocytes,
        // devorocytes, glueocytes, virocytes, nitrocytes and ciliocytes.
        srr = max(srr, 1.0 - r + contact_tp + EW);
        if ds > NR * NR && srr < 1.0 {
            rgb = input.color;
            alpha = 0.5;
        }
    }
    return vec4f(rgb * alpha, alpha);
}

fn appendage_reach(kind: u32, radius: f32) -> f32 {
    switch kind {
        case 1u: { return 4.0 * radius; }
        case 3u, 8u: { return radius + 0.45; }
        case 7u, 10u, 17u: { return radius + 0.6; }
        case 9u: { return radius + 0.675; }
        case 11u: { return radius + 0.3; }
        default: { return 0.0; }
    }
}

@vertex
fn appendage_vertex(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> Varying {
    let corners = array<vec2f, 6>(
        vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
        vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0)
    );
    let cell = cells[instance_index];
    let corner = corners[vertex_index];
    let shader_radius = cell.radius / cell.scale * 50.0;
    let extent = appendage_reach(cell.kind, shader_radius);
    let point = cell.center + corner * (extent / 50.0 * cell.scale);
    let normalized = (point - view.origin) / view.size;
    var output: Varying;
    output.position = vec4f(normalized.x * 2.0 - 1.0, 1.0 - normalized.y * 2.0, 0.0, 1.0);
    output.p = corner * extent;
    output.color = cell.color.rgb;
    output.radius = shader_radius;
    output.kind = cell.kind;
    output.angle = cell.angle;
    output.phase = cell.phase;
    output.reserve = cell.reserve;
    output.signals = cell.signals;
    output.activity = cell.activity;
    output.animation_phase = cell.animation_phase;
    output.auxiliary = cell.auxiliary;
    output.clip_start = cell.clip_start;
    output.clip_count = cell.clip_count;
    return output;
}

fn spike_height(x: f32, cap_radius: f32, height: f32, half_width: f32, cap_center: f32, transition: f32) -> f32 {
    if x < transition || x > cap_center {
        return height * (1.0 - x / half_width);
    }
    return cap_radius - sqrt(max(cap_radius * cap_radius - (x - cap_center) * (x - cap_center), 0.0));
}

fn periodic_spike_x(tp: vec2f, count: f32, radius: f32) -> f32 {
    let n_hat = count / (2.0 * 3.141592653589793);
    var a = 2.0 * fract(n_hat * atan2(tp.y, tp.x));
    if a > 1.0 {
        a = 2.0 - a;
    }
    return (1.0 - a) * radius * 3.141592653589793 / count;
}

fn stereo_spike(tp: vec2f, d: f32, radius: f32, count: f32, n: f32, length: f32) -> bool {
    let pi = 3.141592653589793;
    let n_hat = count / (2.0 * pi);
    let phi = atan2(tp.y, tp.x);
    let phi_floor = floor(phi * n_hat) / n_hat;
    let a = 2.0 * n_hat * (phi - phi_floor);
    let x = (1.0 - a) * radius * pi / count;
    var angle = atan2(d, x);
    let d2 = pi / (n - 1.0 + 2.0 * 1.075);
    let d1 = d2 * 1.075;
    let factor = length / (1.0 / (d1 - d2) + 1.0 / d1);
    if angle > 0.5 * pi {
        angle = pi - angle;
    }
    if angle > d1 {
        angle = fract((angle - d1) / d2) * d2 + d1 - d2;
    }
    let first = max(abs(angle), 0.000001);
    let second = max(abs(2.0 * d1 - d2 - angle), 0.000001);
    let ml = (1.0 / first + 1.0 / second) * factor;
    return x * x + d * d < ml * ml;
}

fn give_cx(x: f32, y: f32) -> f32 {
    let x2 = x * x;
    let y2 = y * y;
    let y3 = y2 * y;
    let y4 = y2 * y2;
    let numerator = y4 - 4.0 * y3 + (2.0 * x2 + 2.0) * y2 + (4.0 - 4.0 * x2) * y + x2 * x2 + 6.0 * x2 - 3.0;
    let denominator = 4.0 * y4 - 16.0 * y3 + (8.0 * x2 + 24.0) * y2 + (-16.0 * x2 - 16.0) * y + 4.0 * x2 * x2 + 8.0 * x2 + 4.0;
    return numerator / denominator;
}

fn give_cy(x: f32, y: f32) -> f32 {
    let x2 = x * x;
    let y2 = y * y;
    let y3 = y2 * y;
    let y4 = y2 * y2;
    let denominator = y4 - 4.0 * y3 + (2.0 * x2 + 6.0) * y2 + (-4.0 * x2 - 4.0) * y + x2 * x2 + 2.0 * x2 + 1.0;
    return -(2.0 * x * y - 2.0 * x) / denominator;
}

fn square_complex(z: vec2f) -> vec2f {
    return vec2f(z.x * z.x - z.y * z.y, 2.0 * z.x * z.y);
}

@fragment
fn appendage_fragment(input: Varying) -> @location(0) vec4f {
    let kind = input.kind;
    let p = input.p;
    let radius = input.radius;
    let dark = input.color * 0.5;
    let orientation = vec2f(cos(input.angle), sin(input.angle));
    let side = vec2f(-orientation.y, orientation.x);
    let tp = vec2f(dot(p, orientation), dot(p, side));
    var visible = false;

    if kind == 1u {
        // tail_frag.glsl on the APK's triangular flagellum primitive.
        let tx = -tp.x / radius;
        let ty = tp.y / radius;
        let triangle_width = sqrt(1.0 - 0.25 * 0.25) * (4.0 - tx) / 3.75;
        if tx >= 0.25 && tx <= 4.0 && abs(ty) <= triangle_width {
            let distance = length(p);
            let relative = (distance - radius) / radius;
            let ww = (4.0 - tx) * 0.1 * relative;
            let phase = -input.animation_phase * 112.5 + tx * 4.0;
            let my = sin(phase) * ww;
            let k = 4.0 * cos(phase) * ww;
            var width = (1.0 - tx / (4.0 * 0.7)) * 0.3 * (1.0 + 0.5 * k * k);
            let edge = 1.0 - relative / 0.1;
            if relative < 0.1 && edge * edge < 1.0 {
                width += 0.1 - 0.1 * sqrt(max(1.0 - edge * edge, 0.0));
            }
            visible = distance >= radius - 0.05 && abs(ty - my) < width;
        }
    } else if kind == 3u {
        let d = length(p) - radius;
        let x = periodic_spike_x(tp, 19.0, radius);
        let h = spike_height(x, 0.06, 0.45, 0.075, 0.075 + 0.06 * 0.84712708838, 0.075 + 0.06 * (0.84712708838 - 0.98639392383));
        // The APK draws this pass with depth testing after the cell body.
        // Its spike fragment shader also shades the disk, but those pixels
        // fail the depth test. Our overlay pass must clip them explicitly.
        visible = d >= 0.0 && d < h;
        for (var i = 0u; i < input.clip_count; i += 1u) {
            let chord = clip_planes[input.clip_start + i];
            if dot(p, chord.xy) > chord.z {
                visible = false;
            }
        }
    } else if kind == 7u {
        // glue_frag.glsl bends each of its 33 spikes with local spin and motion.
        let motion = input.auxiliary.yz;
        var d = length(p) - radius;
        if d >= -0.01 * radius {
            var bend = (input.auxiliary.x + dot(motion, vec2f(-p.y, p.x)) / radius) * d / radius * 30.0;
            d = sqrt(d * d + bend * bend * radius * radius);
            bend = (input.auxiliary.x + dot(motion, vec2f(-p.y, p.x)) / radius) * d / radius * 30.0;
            d = sqrt(d * d + bend * bend * radius * radius);
            let x = periodic_spike_x(vec2f(cos(atan2(tp.y, tp.x) - bend), sin(atan2(tp.y, tp.x) - bend)), 33.0, radius);
            let h = spike_height(x, 0.03, 0.6, 0.0375, 0.0375 + 0.03 * 0.93945122136, 0.0375 + 0.03 * (0.93945122136 - 0.99805257848));
            visible = d < h;
        }
    } else if kind == 8u {
        let d = length(p) - radius;
        let x = periodic_spike_x(tp, 17.0, radius);
        var h: f32;
        if x < 0.06 {
            h = 0.45;
        } else if x > 0.12 {
            h = 0.0;
        } else {
            h = 0.06 - sqrt(max(0.06 * 0.06 - (x - 0.12) * (x - 0.12), 0.0));
        }
        visible = d >= 0.0 && d < h;
        for (var i = 0u; i < input.clip_count; i += 1u) {
            let chord = clip_planes[input.clip_start + i];
            if dot(p, chord.xy) > chord.z {
                visible = false;
            }
        }
    } else if kind == 9u {
        // min_frag.glsl evaluates the same 36 Julia iterations per pixel.
        let d = length(p) - radius;
        if d >= -0.05 * radius {
            var a = 2.0 * fract(atan2(tp.y, tp.x) * (5.0 / (2.0 * 3.141592653589793)));
            if a > 1.0 { a = 2.0 - a; }
            let x = 1.563 + a / 3.0;
            let y = d * 0.4;
            let c = vec2f(give_cx(x, y), give_cy(x, y));
            var z = c;
            for (var i = 0; i < 9; i += 1) {
                z = square_complex(z) + c;
                z = square_complex(z) + c;
                z = square_complex(z) + c;
                z = square_complex(z) + c;
                if dot(z, z) > 4.0 { z = vec2f(2.0, 2.0); }
            }
            visible = dot(z, z) <= 4.0;
        }
    } else if kind == 10u || kind == 11u {
        let d = length(p) - radius + 0.0001;
        if d >= 0.0 {
            if kind == 10u {
                visible = stereo_spike(tp, d, radius, 2.0, 7.0, 0.6);
            } else {
                visible = stereo_spike(tp, d, radius, 9.0, 2.0, 0.3);
            }
        }
    } else if kind == 17u {
        // tax_frag.glsl: moving ciliary edge with two independent phases.
        let d = length(p) - radius;
        if d >= -0.01 * radius && length(tp) > 0.0 {
            let normalized = normalize(tp);
            let d12 = (normalized.x - 1.0) * (normalized.x - 1.0) + normalized.y * normalized.y;
            let d22 = (normalized.x + 1.0) * (normalized.x + 1.0) + normalized.y * normalized.y;
            let log_ratio = log(max(d12 / max(d22, 0.000001), 0.000001));
            var first = input.animation_phase;
            var second = input.auxiliary.w;
            if input.auxiliary.z > 0.5 {
                first = input.auxiliary.w;
                second = input.animation_phase;
            }
            let phase = select(second, first, normalized.y > 0.0) * 112.5;
            let w = abs(sin(log_ratio * 6.66 + phase)) * 0.075 * sqrt(d12 * d22) * radius;
            visible = d < w;
        }
    }
    if !visible {
        discard;
    }
    return vec4f(dark, 1.0);
}

// GenomePreviewView draws marker_vert/marker_frag before the selected cell.
// Its center radius is the cell radius plus .01 world units; the ring is
// retained only within .003 units of that center radius.
struct MarkerVarying {
    @builtin(position) position: vec4f,
    @location(0) distance: vec2f,
    @location(1) @interpolate(flat) radius: f32,
    @location(2) @interpolate(flat) marker_enabled: f32,
};

@vertex
fn marker_vertex(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> MarkerVarying {
    let corners = array<vec2f, 6>(
        vec2f(-1.0, -1.0), vec2f(1.0, -1.0), vec2f(1.0, 1.0),
        vec2f(-1.0, -1.0), vec2f(1.0, 1.0), vec2f(-1.0, 1.0)
    );
    let cell = cells[instance_index];
    let corner = corners[vertex_index];
    let outer_radius = cell.radius + 0.015 * cell.scale;
    let point = cell.center + corner * outer_radius;
    let normalized = (point - view.origin) / view.size;
    var output: MarkerVarying;
    output.position = vec4f(normalized.x * 2.0 - 1.0, 1.0 - normalized.y * 2.0, 0.0, 1.0);
    output.distance = corner * (outer_radius / cell.scale);
    output.radius = cell.radius / cell.scale + 0.01;
    output.marker_enabled = cell.padding0;
    return output;
}

@fragment
fn marker_fragment(input: MarkerVarying) -> @location(0) vec4f {
    let distance = length(input.distance);
    if input.marker_enabled < 0.5 || distance > input.radius + 0.003 || distance < input.radius - 0.003 {
        discard;
    }
    // marker_frag.glsl writes 0.3 * (1, .66, 0) with blending disabled.
    return vec4f(0.3, 0.198, 0.0, 1.0);
}
