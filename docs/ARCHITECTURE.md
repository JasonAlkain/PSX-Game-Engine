# Architecture

## Workspace

Dependency direction is strictly downward:

```
apps/psx_sandbox ─► psx_runtime ─┬─► psx_render ─► psx_core
                                 └─► psx_core

psx_render: wgpu, glam        psx_core: serde, serde_json (no GPU, no window)
```

| Crate | Owns today | Will own (per brief) |
| --- | --- | --- |
| `psx_core` | `FixedStep` accumulator; `RetroStyle` settings, presets, validation and versioned JSON file | Stable IDs, shared types, project configuration |
| `psx_render` | `GpuContext`, `Presenter`, `RetroRenderer` (scene + composite passes), `RenderView`, `Material`, layout/fit policy, `Camera`, `MeshData` (cube, quad, plane, sphere), `TextureData`, WGSL | PS2 extensions, sprites, draw extraction |
| `psx_runtime` | winit `ApplicationHandler`, `InputState`, `FlyCamera`, comparison scene, style key controls, logging | Gameplay scheduling, physics/audio integration |

`psx_scene`, `psx_assets`, `psx_editor` and `psx_cli` are not created until
their milestone needs them. `psx_render` takes any `wgpu::SurfaceTarget` and
does not depend on winit.

## Dependency set (resolved 2026-11-02)

| Crate | Version | Why this version |
| --- | --- | --- |
| Rust toolchain | 1.97.0 stable, edition 2024 | Current stable at time of writing; wgpu 30 needs ≥ 1.87 |
| `wgpu` / `naga` | 30.0.x | Latest; required by `egui-wgpu` 0.37 |
| `winit` | 0.30.13 | `egui-winit` 0.37 requires `^0.30.13`; 0.31 is still beta |
| `egui`, `egui-wgpu`, `egui-winit` | 0.37 (planned, Milestone 3) | Compatible with the two rows above; not added until the editor exists |
| `glam` | 0.34 | Latest; projection helpers live in `glam::camera` |
| `bytemuck`, `pollster` | 1.x | GPU buffer casting; blocking on wgpu futures |
| `tracing`, `tracing-subscriber`, `thiserror`, `anyhow` | latest | Diagnostics and errors |
| `serde`, `serde_json` | 1.0 | Style settings file (Milestone 2) |

`hecs`, `rapier3d`, `kira`, `gltf`, `image`, `uuid` and `clap` are
deferred to the milestone that uses them, so no unused dependencies are built.
Versions are centralised in the root `Cargo.toml`; `Cargo.lock` is committed.

## Conventions

- **World space:** right-handed, +Y up, 1 unit = 1 meter.
- **Camera:** looks along local −Z, +X right. Yaw rotates about world +Y
  (positive = turn left), pitch about camera +X (positive = look up),
  pitch clamped to ±(90° − 0.01 rad).
- **Matrices:** `glam` column-major; `clip = proj * view * model * v`.
- **Clip space:** wgpu/WebGPU NDC, Y up, depth 0 at near and 1 at far
  (`glam::camera::rh::proj::directx::perspective`). Depth test `Less`, cleared to 1.0,
  format `Depth32Float`.
- **Winding:** counter-clockwise front faces, back faces culled.
- **Texture coordinates:** origin top-left, +V down (wgpu and glTF convention).
- **Colour space:** textures uploaded as `Rgba8UnormSrgb` (sampling returns
  linear). Vertex colours, material tints and lights are linear. Colours in
  style settings (background, fog) are sRGB 0–255 values and converted to
  linear once on the CPU. The scene target is linear `Rgba16Float`. The
  composite pass encodes to sRGB, dithers and quantises in sRGB-encoded space,
  then decodes back to linear when the output is an `*Srgb` format (the
  hardware re-encodes to the same 8-bit value) or writes the encoded value
  directly when it is not. Gamma is applied exactly once on both paths; a test
  compares the two.
- **Uniform layout:** `FrameUniforms`, `DrawUniforms` and `CompositeUniforms`
  (Rust) are checked against the WGSL structs' sizes and member offsets with
  naga in unit tests; vertex attribute locations and the interpolation
  qualifiers of the UV/colour varyings are checked too. All members are
  `vec4`/`mat4x4` so no implicit padding exists. Per-draw uniforms live in one
  buffer per view with dynamic offsets aligned to
  `min_uniform_buffer_offset_alignment`.

## Rendering (Milestone 2)

Per frame and per `RenderView`:

1. **Scene pass** (`scene.wgsl`) into the view's internal `Rgba16Float`
   colour and `Depth32Float` targets, cleared to the style background.
   Opaque and cutout items draw in submission order with depth writes;
   blended items follow, sorted back-to-front by model origin, with depth test
   but no depth writes (per-object sorting; intersecting transparent meshes
   can mis-order). Four scene pipelines exist (opaque/blended × culled/double
   sided), all created at start-up.
   - Lighting: per vertex (ambient + one directional light), or unlit.
     Flat shading uses a `@interpolate(flat)` copy of the lit colour (first
     vertex provokes); mesh normals are never modified.
   - Vertex snapping: clip X/Y → NDC → internal pixels, rounded to
     `grid` pixels, blended by `strength`, then multiplied back by W; Z and W
     are unchanged. Vertices with `w ≤ 1e-4` (at or behind the camera) or
     more than 64 NDC units off-screen are not snapped and are left to the
     clipper. Snapping is a render-only effect; gameplay and collision
     transforms are untouched.
   - Affine UVs: the vertex shader writes the same UV to a perspective
     varying and an `@interpolate(linear)` varying; the fragment shader picks
     one if the style enables affine mapping and the material allows it.
   - Fog: linear in true per-fragment distance from the camera (world position
     is interpolated, not distance), mixed in linear space before output.
   - Textures: nearest sampling, repeat, no mipmaps.
2. **Composite pass** (`composite.wgsl`) into the window: cleared to black
   (letterbox bars), then a full-screen triangle per slot clipped by a scissor
   rectangle. Each output pixel fetches exactly one internal texel
   (`textureLoad`, no filtering) and applies, in order: clamp → sRGB encode →
   4×4 Bayer offset (anchored to the internal texel, amplitude =
   `strength` quantisation steps) → round to `2^bits − 1` levels (255 when
   colour reduction is off) → output encoding as above. Because processing is
   per internal texel, every upscaled block is uniform.

The split comparison view renders a second `RenderView` with
`style_controls::reference_style` (all retro effects off, same fog and
camera limits) at the native size of the same rectangle, and composites the
two views into the left and right halves.

### Fit policy

`layout::place` decides where the internal image goes:

- `integer` (PS1 default): the largest whole-number scale that fits, centred.
  If the window is smaller than the internal image in either axis it falls
  back to `fit`.
- `fit`: the largest aspect-correct rectangle (rounded to whole pixels),
  centred; texel blocks may be uneven, including when downscaling.
- Low resolution off: the image renders at the native size of the `fit`
  rectangle (same aspect as the configured internal size).

The camera's aspect ratio is the internal image's aspect, never the
window's, so widescreen is a different internal size (e.g. 426×240), not a
stretch.

### Style settings file

`RetroStyle` is stored as pretty JSON:
`{"format": "psx_style", "version": 1, "style": {...}}`. Loading checks the
format tag, rejects versions newer than the build supports (and version 0),
rejects unknown fields, and validates every range (`RetroStyle::validate`).
Saving validates first, writes `<file>.tmp`, copies the previous file to
`<file>.bak`, then renames the temporary file into place. The sandbox loads
`psx_style.json` from the working directory (or `--style PATH`); an invalid
file is reported and left untouched while the PS1 preset is used.
`RetroStyle::preset()` reports `PS1`, `Clean` or `Custom` (changed).

## Frame model

Per `RedrawRequested`:

1. `FixedStep::advance(frame_time)` returns the tick count (60 Hz, at most 8
   per frame; excess time is dropped and logged) and an interpolation alpha.
2. Each tick: `InputState::begin_tick()` yields held keys, press edges and
   accumulated mouse motion; the fly camera and the spinning crate update.
   Style keys are handled immediately on key press (UI actions, not
   simulation input).
3. The previous and current simulation states are interpolated by alpha and
   rendered. Presentation rate never changes simulation speed.

Input edges are delivered to exactly one tick (the first that runs after the
event); taps shorter than a tick are not lost; OS key repeat is ignored; focus
loss clears held keys and releases mouse capture.

## Surface lifecycle

- `Resized` reconfigures the surface (internal targets follow the layout on
  the next frame); a zero size marks the
  window minimized, skips rendering and switches the loop to `ControlFlow::Wait`.
- `Suboptimal` frames are presented, and the surface is reconfigured before the
  next acquire (never while a frame is held).
- `Outdated` reconfigures; `Timeout`/`Occluded` skip the frame. Any skipped
  acquire waits 50 ms (`ControlFlow::WaitUntil`) before retrying, and an
  occluded window (`WindowEvent::Occluded(true)`) sleeps until it is visible.
- `Lost` first reconfigures the swapchain; a second consecutive `Lost` drops
  the old surface and creates a new one (Vulkan allows only one surface per
  window, so the old one must be dropped first). After 5 consecutive
  recreation failures the app exits with an error.
- `Destroyed` (native window gone) exits cleanly.
- Uncaptured wgpu errors are logged, not panicked on.

GPU pipelines, layouts and samplers are created once at start-up. Internal
targets are recreated only when a view's size changes (style or window
change). Per frame there are a few uniform writes, two render passes per
view and one submit.
