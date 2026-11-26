// Scene shader. Lighting follows the DS hardware model: colours are computed
// per vertex from up to four directional lights and the material colours,
// then multiplied with the texture. All maths is done on display-referred
// (gamma) values, as on the console.

struct Globals {
    view_proj: mat4x4<f32>,
    // xyz: direction the light travels, w: 1 if enabled.
    light_dir: array<vec4<f32>, 4>,
    light_color: array<vec4<f32>, 4>,
    // x: lighting on, y: textures on, z: vertex colours on, w: unused.
    options: vec4<f32>,
};

struct Material {
    // rgb: diffuse, a: polygon alpha.
    diffuse: vec4<f32>,
    // rgb: ambient, a: 1 if textured.
    ambient: vec4<f32>,
    // rgb: emission, a: light mask.
    emission: vec4<f32>,
    tex_row0: vec4<f32>,
    tex_row1: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;
@group(1) @binding(0) var<uniform> material: Material;
@group(1) @binding(1) var tex: texture_2d<f32>;
@group(1) @binding(2) var samp: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(7) m3: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) uv: vec2<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
    let world = model * vec4<f32>(in.pos, 1.0);

    var color = in.color;
    if (globals.options.z < 0.5) {
        color = vec3<f32>(1.0);
    }
    if (dot(in.normal, in.normal) > 0.25) {
        if (globals.options.x > 0.5) {
            let n = normalize((model * vec4<f32>(in.normal, 0.0)).xyz);
            let mask = u32(material.emission.a + 0.5);
            var c = material.emission.rgb;
            for (var i = 0u; i < 4u; i = i + 1u) {
                if (((mask >> i) & 1u) == 1u && globals.light_dir[i].w > 0.5) {
                    let d = max(0.0, -dot(globals.light_dir[i].xyz, n));
                    c = c + globals.light_color[i].rgb * (material.diffuse.rgb * d + material.ambient.rgb);
                }
            }
            color = min(c, vec3<f32>(1.0));
        } else {
            color = vec3<f32>(1.0);
        }
    }

    var out: VsOut;
    out.clip = globals.view_proj * world;
    out.color = color;
    let uv1 = vec3<f32>(in.uv, 1.0);
    out.uv = vec2<f32>(dot(material.tex_row0.xyz, uv1), dot(material.tex_row1.xyz, uv1));
    return out;
}

fn shade(in: VsOut) -> vec4<f32> {
    var texel = textureSample(tex, samp, in.uv);
    if (material.ambient.a < 0.5 || globals.options.y < 0.5) {
        texel = vec4<f32>(1.0);
    }
    return vec4<f32>(texel.rgb * in.color, texel.a * material.diffuse.a);
}

@fragment
fn fs_opaque(in: VsOut) -> @location(0) vec4<f32> {
    let c = shade(in);
    if (c.a < 0.5) {
        discard;
    }
    return vec4<f32>(c.rgb, 1.0);
}

@fragment
fn fs_blend(in: VsOut) -> @location(0) vec4<f32> {
    let c = shade(in);
    if (c.a < 0.02) {
        discard;
    }
    return c;
}

// Flat-coloured boxes for markers (NPCs, warps, triggers).

struct BoxIn {
    @location(0) pos: vec3<f32>,
    @location(4) m0: vec4<f32>,
    @location(5) m1: vec4<f32>,
    @location(6) m2: vec4<f32>,
    @location(7) m3: vec4<f32>,
    @location(8) color: vec4<f32>,
};

struct BoxOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) local: vec3<f32>,
};

@vertex
fn vs_box(in: BoxIn) -> BoxOut {
    let model = mat4x4<f32>(in.m0, in.m1, in.m2, in.m3);
    var out: BoxOut;
    out.clip = globals.view_proj * (model * vec4<f32>(in.pos, 1.0));
    out.color = in.color;
    out.local = in.pos;
    return out;
}

@fragment
fn fs_box(in: BoxOut) -> @location(0) vec4<f32> {
    // Darker towards the base so boxes read as solid shapes.
    let shade = 0.65 + 0.35 * clamp(in.local.y, 0.0, 1.0);
    return vec4<f32>(in.color.rgb * shade, in.color.a);
}
