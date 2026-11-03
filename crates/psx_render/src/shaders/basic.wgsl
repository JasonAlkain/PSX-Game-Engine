// Milestone 1 forward shader: textured, vertex-coloured, vertex-lit triangles.
//
// Colour space: the base texture is an *Srgb format, so sampling returns
// linear values. Vertex colours and lighting are linear. The render target is
// an *Srgb format, so the hardware encodes on write; this shader never applies
// gamma itself.
//
// Uniform layout (std140-like WGSL rules) is checked against the Rust
// `FrameUniforms` struct by a unit test in gpu.rs.

struct FrameUniforms {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    // xyz: unit direction *towards* the light, world space. w unused.
    light_dir: vec4<f32>,
    // rgb: light colour (linear). a unused.
    light_color: vec4<f32>,
    // rgb: ambient colour (linear). a unused.
    ambient: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: FrameUniforms;
@group(1) @binding(0) var base_texture: texture_2d<f32>;
@group(1) @binding(1) var base_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    let world = frame.model * vec4<f32>(in.position, 1.0);
    // Valid for rotation + uniform scale, which is all Milestone 1 uses.
    let n = normalize((frame.model * vec4<f32>(in.normal, 0.0)).xyz);
    let diffuse = max(dot(n, frame.light_dir.xyz), 0.0);
    let lit = frame.ambient.rgb + frame.light_color.rgb * diffuse;

    var out: VertexOutput;
    out.clip_position = frame.view_proj * world;
    out.uv = in.uv;
    out.color = vec4<f32>(in.color.rgb * lit, in.color.a);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let texel = textureSample(base_texture, base_sampler, in.uv);
    return texel * in.color;
}
