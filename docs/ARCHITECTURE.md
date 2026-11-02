# Architecture

## Workspace

Dependency direction is strictly downward:

```
apps/psx_sandbox ─► psx_runtime ─► psx_render ─► (wgpu, glam)
                         └──────► psx_core   ─► (std only)
```

| Crate | Owns today | Will own (per brief) |
| --- | --- | --- |
| `psx_core` | `FixedStep` accumulator | Stable IDs, shared types, configuration |
| `psx_render` | `GpuContext`, `Presenter`, `MeshRenderer`, `Camera`, `MeshData`, `TextureData`, WGSL | Materials, retro passes, upscale/composite |
| `psx_runtime` | winit `ApplicationHandler`, `InputState`, `FlyCamera`, logging | Gameplay scheduling, physics/audio integration |

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

`hecs`, `rapier3d`, `kira`, `gltf`, `image`, `serde`, `uuid` and `clap` are
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
  linear). Vertex colours, lights and clear colour are linear. The surface
  uses an sRGB format when the adapter offers one, so encoding happens once in
  hardware and shaders never apply gamma. A warning is logged if no sRGB
  surface format exists.
- **Uniform layout:** `FrameUniforms` (Rust) is checked against the WGSL
  struct's size and member offsets with naga in a unit test, and vertex
  attribute locations are checked against the shader's input struct.

## Frame model

Per `RedrawRequested`:

1. `FixedStep::advance(frame_time)` returns the tick count (60 Hz, at most 8
   per frame; excess time is dropped and logged) and an interpolation alpha.
2. Each tick: `InputState::begin_tick()` yields held keys, press edges and
   accumulated mouse motion; the fly camera and the test mesh update.
3. The previous and current simulation states are interpolated by alpha and
   rendered. Presentation rate never changes simulation speed.

Input edges are delivered to exactly one tick (the first that runs after the
event); taps shorter than a tick are not lost; OS key repeat is ignored; focus
loss clears held keys and releases mouse capture.

## Surface lifecycle

- `Resized` reconfigures the surface and depth buffer; a zero size marks the
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

GPU pipelines, layouts and samplers are created once at start-up; per frame
there is one uniform write, one render pass and one submit.
