// Composite pass: maps the internal image into a rectangle of the output
// target with nearest-neighbour texel selection, and applies the retro image
// processing per *source* texel, so every output pixel covering one internal
// pixel gets exactly the same value (the dither pattern is anchored to
// internal pixels, not window pixels).
//
// Pass order (per output pixel):
//   1. pick the source texel: floor((pixel - rect.origin) / rect.size * src_size)
//   2. read linear RGB from the Rgba16Float scene image, clamp to [0, 1]
//   3. encode linear -> sRGB (exact piecewise sRGB curve)
//   4. add the 4x4 Bayer offset for that texel, scaled by dither strength,
//      in units of one quantisation step
//   5. round to `levels` steps per channel (levels = 2^bits - 1; 255 when
//      colour reduction is off)
//   6. if the output target is an *Srgb format, decode back to linear so the
//      hardware re-encodes to the same value; otherwise write the encoded value.
// Gamma is therefore applied exactly once on every path.

struct CompositeUniforms {
    // xy: rectangle origin in output pixels, zw: rectangle size in pixels.
    rect: vec4<f32>,
    // xy: source image size in texels. zw unused.
    source_size: vec4<f32>,
    // x: quantisation levels per channel (2^bits - 1),
    // y: dither amplitude in steps (0 = off),
    // z: 1.0 when the output target is NOT sRGB (write encoded values).
    // w unused.
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: CompositeUniforms;
@group(0) @binding(1) var scene_image: texture_2d<f32>;

@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    // One triangle covering the whole viewport.
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

// Classic 4x4 Bayer matrix value in [0, 15] built from bit interleaving.
fn bayer4(texel: vec2<u32>) -> f32 {
    let x = texel.x & 3u;
    let y = texel.y & 3u;
    let xy = x ^ y;
    let v = ((xy & 1u) << 3u) | ((y & 1u) << 2u) | (xy & 2u) | ((y & 2u) >> 1u);
    return f32(v);
}

@fragment
fn fs_composite(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let source_size = params.source_size.xy;
    let local = (position.xy - params.rect.xy) / params.rect.zw;
    let max_texel = vec2<i32>(source_size) - vec2<i32>(1);
    let texel = clamp(vec2<i32>(floor(local * source_size)), vec2<i32>(0), max_texel);

    let linear = clamp(textureLoad(scene_image, texel, 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    let levels = params.color.x;
    let offset = (bayer4(vec2<u32>(texel)) + 0.5) / 16.0 - 0.5;
    let encoded = linear_to_srgb(linear) + vec3<f32>(offset * params.color.y / levels);
    let quantised = clamp(round(encoded * levels) / levels, vec3<f32>(0.0), vec3<f32>(1.0));

    let out = select(srgb_to_linear(quantised), quantised, params.color.z > 0.5);
    return vec4<f32>(out, 1.0);
}
