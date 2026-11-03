// Scene pass: draws meshes into the internal-resolution colour target.
//
// Colour space: base textures are *Srgb formats, so sampling returns linear
// values. Vertex colours, tints, lights and fog colour are linear. The scene
// target is Rgba16Float (linear). Encoding to sRGB, colour reduction and
// dithering happen later, in composite.wgsl. This shader never applies gamma.
//
// Uniform layouts are checked against the Rust structs `FrameUniforms` and
// `DrawUniforms` (retro.rs) by unit tests.

struct FrameUniforms {
    view_proj: mat4x4<f32>,
    // xyz: camera position (world). w unused.
    camera_pos: vec4<f32>,
    // xyz: unit direction *towards* the light (world). w unused.
    light_dir: vec4<f32>,
    // rgb: light colour. a unused.
    light_color: vec4<f32>,
    // rgb: ambient colour. a unused.
    ambient: vec4<f32>,
    // rgb: fog colour. a: 1.0 when fog is enabled.
    fog_color: vec4<f32>,
    // x: fog start distance, y: 1 / (end - start). zw unused.
    fog_range: vec4<f32>,
    // xy: colour target size in pixels, z: snap grid in pixels,
    // w: snap strength (0 disables snapping).
    snap: vec4<f32>,
    // x: 1.0 when affine UVs are enabled for this view. yzw unused.
    options: vec4<f32>,
};

struct DrawUniforms {
    model: mat4x4<f32>,
    // Inverse-transpose of the model matrix (upper 3x3 used).
    normal_matrix: mat4x4<f32>,
    // Linear RGBA multiplied with texture and vertex colour.
    tint: vec4<f32>,
    // x: alpha cutoff (negative = no cutout), y: 1.0 = lit,
    // z: 1.0 = flat shading, w: 1.0 = material accepts affine UVs.
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: FrameUniforms;
@group(1) @binding(0) var<uniform> draw: DrawUniforms;
@group(2) @binding(0) var base_texture: texture_2d<f32>;
@group(2) @binding(1) var base_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    // Perspective-correct UVs (default interpolation).
    @location(0) uv_perspective: vec2<f32>,
    // The same UVs interpolated linearly in screen space (affine mapping).
    @location(1) @interpolate(linear) uv_affine: vec2<f32>,
    @location(2) color_smooth: vec4<f32>,
    // Lighting from the provoking (first) vertex: flat shading.
    @location(3) @interpolate(flat) color_flat: vec4<f32>,
    // World position, interpolated so fog distance is exact per fragment
    // even on large triangles.
    @location(4) world_position: vec3<f32>,
};

// Smallest clip-space w for which snapping is applied. Vertices at or behind
// the camera plane are left untouched for the hardware clipper.
const SNAP_MIN_W: f32 = 1.0e-4;
// Vertices this far outside the viewport (in NDC units) are not snapped:
// they are clipped anyway and large values would lose float precision.
const SNAP_MAX_NDC: f32 = 64.0;

// Quantises clip-space X/Y to the internal pixel grid, keeping Z and W.
fn snap_vertex(clip: vec4<f32>) -> vec4<f32> {
    let strength = frame.snap.w;
    if strength <= 0.0 || clip.w <= SNAP_MIN_W {
        return clip;
    }
    let ndc = clip.xy / clip.w;
    if any(abs(ndc) > vec2<f32>(SNAP_MAX_NDC)) {
        return clip;
    }
    let size = frame.snap.xy;
    let grid = frame.snap.z;
    // NDC [-1, 1] -> pixels [0, size]; whole pixel numbers are pixel corners.
    let pixels = (ndc * 0.5 + 0.5) * size;
    let snapped_pixels = floor(pixels / grid + 0.5) * grid;
    let snapped_ndc = snapped_pixels / size * 2.0 - 1.0;
    let out_ndc = mix(ndc, snapped_ndc, strength);
    return vec4<f32>(out_ndc * clip.w, clip.z, clip.w);
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    let world = draw.model * vec4<f32>(in.position, 1.0);

    var lit = vec3<f32>(1.0);
    if draw.params.y > 0.5 {
        let n = normalize((draw.normal_matrix * vec4<f32>(in.normal, 0.0)).xyz);
        let diffuse = max(dot(n, frame.light_dir.xyz), 0.0);
        lit = frame.ambient.rgb + frame.light_color.rgb * diffuse;
    }
    let color = vec4<f32>(in.color.rgb * lit, in.color.a) * draw.tint;

    var out: VertexOutput;
    out.clip_position = snap_vertex(frame.view_proj * world);
    out.uv_perspective = in.uv;
    out.uv_affine = in.uv;
    out.color_smooth = color;
    out.color_flat = color;
    out.world_position = world.xyz;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let affine = frame.options.x > 0.5 && draw.params.w > 0.5;
    let uv = select(in.uv_perspective, in.uv_affine, affine);
    let texel = textureSample(base_texture, base_sampler, uv);
    let shade = select(in.color_smooth, in.color_flat, draw.params.z > 0.5);
    var color = texel * shade;

    if draw.params.x >= 0.0 && color.a < draw.params.x {
        discard;
    }

    if frame.fog_color.a > 0.5 {
        let view_distance = distance(in.world_position, frame.camera_pos.xyz);
        let f = clamp((view_distance - frame.fog_range.x) * frame.fog_range.y, 0.0, 1.0);
        color = vec4<f32>(mix(color.rgb, frame.fog_color.rgb, f), color.a);
    }
    return color;
}
