// Scene pass: draws meshes into the internal-resolution colour target.
//
// Colour space: base textures are *Srgb formats, so sampling returns linear
// values. Vertex colours, tints, lights and fog colour are linear. The scene
// target is Rgba16Float (linear). Encoding to sRGB, colour reduction and
// dithering happen later, in composite.wgsl. This shader never applies gamma.
//
// Uniform layouts are checked against the Rust structs `FrameUniforms` and
// `DrawUniforms` (retro.rs) by unit tests.

const MAX_LOCAL_LIGHTS: u32 = 8u;
const MAX_BLOB_SHADOWS: u32 = 8u;
const MAX_JOINTS: u32 = 64u;

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
    // x: 1.0 when affine UVs are enabled for this view, y: 1.0 for
    // per-pixel lighting, z: local light count, w: blob shadow count.
    options: vec4<f32>,
    // x: blob shadow opacity. y: affine correction distance in metres
    // (always > 0). zw unused.
    shadow_params: vec4<f32>,
    // Point lights, two entries each: (position.xyz, range) and
    // (linear colour × intensity, unused).
    lights: array<vec4<f32>, 16>,
    // Blob shadows: (centre x, bottom y, centre z, radius).
    blobs: array<vec4<f32>, 8>,
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
    // rgb: linear emitted colour added after shading (unaffected by
    // lights, still fogged). a unused.
    emissive: vec4<f32>,
    // x: 1.0 when this draw is skinned with the bound joint palette.
    skin: vec4<f32>,
    // xyz: texture repeats along local X, Y, Z. w: 1.0 = apply them.
    uv_tile: vec4<f32>,
};

struct JointPalette {
    joints: array<mat4x4<f32>, 64>,
};

@group(0) @binding(0) var<uniform> frame: FrameUniforms;
@group(0) @binding(1) var base_sampler: sampler;
@group(1) @binding(0) var<uniform> draw: DrawUniforms;
@group(2) @binding(0) var base_texture: texture_2d<f32>;
@group(3) @binding(0) var<uniform> palette: JointPalette;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec4<f32>,
    @location(4) joints: vec4<u32>,
    @location(5) weights: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    // Perspective-correct UVs (default interpolation).
    @location(0) uv_perspective: vec2<f32>,
    // The same UVs for affine mapping: (uv * k, k), perspective-interpolated,
    // divided in the fragment shader. See `affine_weight`.
    @location(1) uv_affine: vec3<f32>,
    @location(2) color_smooth: vec4<f32>,
    // Lighting from the provoking (first) vertex: flat shading.
    @location(3) @interpolate(flat) color_flat: vec4<f32>,
    // World position, interpolated so fog distance is exact per fragment
    // even on large triangles.
    @location(4) world_position: vec3<f32>,
    // World normal (per-pixel lighting and blob shadows).
    @location(5) world_normal: vec3<f32>,
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

// Ambient + directional + bounded point lights at a world position. Point
// lights fall off as (1 - (d / range)^2)^2, reaching exactly zero at range.
fn light_at(n: vec3<f32>, p: vec3<f32>) -> vec3<f32> {
    var lit = frame.ambient.rgb + frame.light_color.rgb * max(dot(n, frame.light_dir.xyz), 0.0);
    let count = min(u32(frame.options.z), MAX_LOCAL_LIGHTS);
    for (var i = 0u; i < count; i++) {
        let pos_range = frame.lights[i * 2u];
        let color = frame.lights[i * 2u + 1u].rgb;
        let to_light = pos_range.xyz - p;
        let d = length(to_light);
        if d >= pos_range.w {
            continue;
        }
        let x = d / pos_range.w;
        let window = (1.0 - x * x) * (1.0 - x * x);
        let l = to_light / max(d, 1.0e-4);
        lit += color * window * max(dot(n, l), 0.0);
    }
    return lit;
}

// 0 = no shadow, 1 = full blob shadow at this point.
fn blob_shadow(n: vec3<f32>, p: vec3<f32>) -> f32 {
    let count = min(u32(frame.options.w), MAX_BLOB_SHADOWS);
    var shadow = 0.0;
    if n.y < 0.3 {
        return 0.0;
    }
    for (var i = 0u; i < count; i++) {
        let b = frame.blobs[i];
        let r = max(b.w, 1.0e-3);
        // Height of the caster's bottom above this point; the caster does
        // not shadow itself (its surfaces are above its bottom).
        let h = b.y - p.y;
        if h < -0.02 || h > r * 4.0 {
            continue;
        }
        let d = length(b.xz - p.xz);
        let k = (1.0 - smoothstep(r * 0.35, r, d)) * (1.0 - clamp(h / (r * 4.0), 0.0, 1.0));
        shadow = max(shadow, k);
    }
    return shadow;
}

// Scales a flat face's UVs by the repeats along the two axes it spans.
fn tiled_uv(uv: vec2<f32>, normal: vec3<f32>) -> vec2<f32> {
    if draw.uv_tile.w < 0.5 {
        return uv;
    }
    let n = abs(normal);
    if n.x > 0.5 {
        return uv * draw.uv_tile.zy;
    }
    if n.y > 0.5 {
        return uv * draw.uv_tile.xz;
    }
    return uv * draw.uv_tile.xy;
}

// Weight that turns perspective-correct interpolation into PS1-style affine
// interpolation beyond the correction distance D. Interpolating (uv * k, k)
// with perspective correction and dividing gives sum(l * uv / w') /
// sum(l / w') with w' = w / k = min(w, D): vertices nearer than D are
// weighted by their true depth (perspective-correct), vertices beyond it all
// share depth D (affine). Large polygons far away keep the PS1 swim, while a
// polygon the camera is right up against no longer warps without bound.
// k is 1 on and behind the camera plane, so near-plane clipping stays sane.
fn affine_weight(w: f32) -> f32 {
    return max(1.0, w / frame.shadow_params.y);
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var local_position = vec4<f32>(in.position, 1.0);
    var local_normal = vec4<f32>(in.normal, 0.0);
    if draw.skin.x > 0.5 {
        let j = min(in.joints, vec4<u32>(MAX_JOINTS - 1u));
        let skin = palette.joints[j.x] * in.weights.x + palette.joints[j.y] * in.weights.y
            + palette.joints[j.z] * in.weights.z + palette.joints[j.w] * in.weights.w;
        local_position = skin * local_position;
        local_normal = skin * local_normal;
    }
    let world = draw.model * local_position;
    // Zero-length normals (degenerate source data) stay zero instead of NaN.
    let raw_n = (draw.normal_matrix * local_normal).xyz;
    let n = select(vec3<f32>(0.0), normalize(raw_n), dot(raw_n, raw_n) > 1.0e-12);

    var lit = vec3<f32>(1.0);
    if draw.params.y > 0.5 && frame.options.y < 0.5 {
        lit = light_at(n, world.xyz);
    }
    let color = vec4<f32>(in.color.rgb * lit, in.color.a) * draw.tint;

    var out: VertexOutput;
    out.clip_position = snap_vertex(frame.view_proj * world);
    let uv = tiled_uv(in.uv, in.normal);
    out.uv_perspective = uv;
    let k = affine_weight(out.clip_position.w);
    out.uv_affine = vec3<f32>(uv * k, k);
    out.color_smooth = color;
    out.color_flat = color;
    out.world_position = world.xyz;
    out.world_normal = n;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Derivatives and implicit-LOD sampling need uniform control flow, so
    // they come before any branch.
    let face = normalize(cross(dpdx(in.world_position), dpdy(in.world_position)));
    let affine = frame.options.x > 0.5 && draw.params.w > 0.5;
    let uv = select(in.uv_perspective, in.uv_affine.xy / in.uv_affine.z, affine);
    let texel = textureSample(base_texture, base_sampler, uv);

    let flat_shading = draw.params.z > 0.5;
    let shade = select(in.color_smooth, in.color_flat, flat_shading);
    var color = texel * shade;

    if draw.params.y > 0.5 {
        let to_eye = frame.camera_pos.xyz - in.world_position;
        // The face normal (from screen-space derivatives) faces the camera.
        let face_n = select(face, -face, dot(face, to_eye) < 0.0);
        let wn = in.world_normal;
        let smooth_n = select(face_n, normalize(wn), dot(wn, wn) > 1.0e-6);
        let n = select(smooth_n, face_n, flat_shading);
        if frame.options.y > 0.5 {
            color = vec4<f32>(color.rgb * light_at(n, in.world_position), color.a);
        }
        let s = blob_shadow(n, in.world_position);
        color = vec4<f32>(color.rgb * (1.0 - frame.shadow_params.x * s), color.a);
    }
    color = vec4<f32>(color.rgb + draw.emissive.rgb, color.a);

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
