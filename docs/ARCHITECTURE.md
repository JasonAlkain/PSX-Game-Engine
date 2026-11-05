# Architecture

## Workspace

Dependency direction is strictly downward:

Crate names below drop the `copperframe_` prefix (`runtime` is
`copperframe_runtime`, and so on).

```
apps/copperframe_sandbox ─► runtime
crates/copperframe_editor ─┬─► runtime ─┬─► render ─► core
 (lib + bin)               │            ├─► assets ─┬─► scene ─► core
apps/copperframe_cli ──────┴─► assets   └─► scene   └─► core

apps/copperframe_player ─┐
game crates ─────────────┴─► game ─┬─► runtime (window, renderer, scene GPU data)
 (examples/room/game)              ├─► audio ─► assets, scene, core
                                   └─► assets, scene, core

crates/copperframe_editor ─► audio (sound preview and mixer panel)

render: wgpu, glam          core / scene / assets: no GPU, no window
game: hecs, rapier3d, kira  the editor and CLI never link game; they run a
                            game executable as a child process
```

| Crate | Owns today | Will own (per brief) |
| --- | --- | --- |
| `copperframe_core` | `FixedStep`; `EntityId`/`AssetId`/`SceneId` (UUID v4 newtypes); `atomic_write`; `RetroStyle` settings, presets, validation and versioned JSON file | Project configuration shared with gameplay |
| `copperframe_scene` | `Scene` document (entities, hierarchy, `Transform`, mesh/material, collider, audio and behaviour components, settings), versioned scene file (v3, migrates v1 and v2), light/animator/shadow components, `Command`/`Change`, `History` (undo/redo), command batches | |
| `copperframe_assets` | `Project`, editor lock, asset `.meta.json` sidecars, `AssetDatabase`, glTF (static and skinned, with clips), PNG/JPEG and WAV/OGG import, `anim` (skeletons, poses, clip sampling, blending), project validation, `GameTarget` (find/build/run a project's game), `export` (standalone folder), test fixtures | Cooked asset cache |
| `copperframe_render` | `GpuContext`, `Presenter`, `RetroRenderer` (scene + composite passes), `RenderView`, `Material`, layout/fit policy, `Camera`, `MeshData` (cube, quad, plane, sphere), `TextureData`, WGSL | PS2 extensions, sprites |
| `copperframe_runtime` | winit `ApplicationHandler`, `InputState`, `FlyCamera`, comparison scene, style key controls, logging, `scene_gpu::SceneResources` (GPU assets for a scene, fallbacks, picking) | Gameplay scheduling, physics/audio integration |
| `copperframe_editor` | `EditorSession` (scene + history + assets + save/reload), egui UI in dockable panels, toolbar, move/rotate/scale gizmos with grid, vertex and model snapping, asset browser, viewport geometry and picking rays, editor camera, collider wireframes, gameplay inspectors, Play/Stop (child process), Build menu jobs (validate, export, test script, benchmark), project hub (recent projects, new project from a template), `assets/`+`scenes/` folder watcher, `copperframe_editor` binary | |
| `copperframe_audio` | The project mixer on kira (`Mixer`: master plus one sub-track per `AudioBus`, volumes, mute, solo, limiter, player gains), peak meters (`Meter`, `MeterEffect`), and `Preview` (the editor's sound player) | |
| `copperframe_game` | Gameplay API (`Game`, `Behavior`, `Registry`), `Simulation` (hecs world from a scene), input actions, rapier3d physics, kira audio, built-in `psx.*` behaviours, windowed player (menus, HUD, settings, saves), headless script runner, `copperframe_game::main` launcher, `animation` (per-entity clip playback and crossfades) | |
| `apps/copperframe_player` | Stock player: `copperframe_game::main` with only engine behaviours, for projects without a game crate | |
| `apps/copperframe_cli` | `copperframe` binary: `new` (templates and game crate from `copperframe_assets::template`), `schema`, `validate`, `import`, `inspect`, `apply`, `test-scene`, `build` | |

`copperframe_render` takes any `wgpu::SurfaceTarget` and does not depend on winit.
The editor is a library so its session logic is tested without a window.

## Dependency set (resolved 2026-11-02, extended 2026-11-03)

| Crate | Version | Why this version |
| --- | --- | --- |
| Rust toolchain | 1.97.0 stable, edition 2024 | Current stable at time of writing; wgpu 30 needs ≥ 1.87 |
| `wgpu` / `naga` | 30.0.x | Latest; required by `egui-wgpu` 0.37 |
| `winit` | 0.30.13 | `egui-winit` 0.37 requires `^0.30.13`; 0.31 is still beta |
| `egui`, `egui-wgpu`, `egui-winit` | 0.37 | Editor UI; compatible with the two rows above. `egui` uses `default_fonts`; `egui-wgpu` without default features |
| `egui_tiles` | 0.18 (`serde`) | Dockable editor panels. `egui_dock` has no release for egui 0.37 yet |
| `dunce` | 1.0 | Canonical paths without the Windows verbatim prefix, which cargo rejects in manifests |
| `glam` | 0.34 (`serde`) | Latest; projection helpers live in `glam::camera`; vectors serialise as arrays |
| `uuid` | 1.x (`v4`, `serde`) | Persistent IDs |
| `gltf` | 1.4 (`import`, `utils`, `names`, `KHR_materials_unlit`) | glTF document parsing; buffers and URIs are resolved by our own bounded code |
| `image` | 0.25 (`png`, `jpeg` only) | Texture decoding with size/allocation limits |
| `base64` | 0.22 | `data:` URIs in `.gltf` files |
| `clap` | 4.x (`derive`) | `copperframe` CLI |
| `bytemuck`, `pollster` | 1.x | GPU buffer casting; blocking on wgpu futures |
| `tracing`, `tracing-subscriber`, `thiserror`, `anyhow` | latest | Diagnostics and errors |
| `serde`, `serde_json` | 1.0 | Style, scene, batch, project and asset meta files |

| `hecs` | 0.11 | Runtime ECS built from the scene document |
| `rapier3d` | 0.36 | Physics and kinematic character controller. It uses glam 0.33 internally (through `glamx`); `copperframe_game::physics` converts at the boundary |
| `kira` | 0.12 (`cpal`, `wav`, `pcm`, `ogg`, `vorbis`) | Audio buses, tweens, spatial tracks, one listener. Linux needs `libasound2-dev` to build |
| `directories` | 6 | Per-user config and data folders for player settings and saves |
 Versions are centralised in the root `Cargo.toml`;
`Cargo.lock` is committed.

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
- **Entity transforms:** local translation, rotation (unit quaternion) and
  scale; `world = parent_world * T * R * S`. Euler angles appear only at the
  edges (inspector, `rotation_euler_deg` in batches): degrees `[x, y, z]`,
  applied as yaw (Y), then pitch (X), then roll (Z), i.e. `Ry * Rx * Rz`.
  Scale components must be at least `1e-4` in magnitude (negative allowed).
- **Colour space:** textures uploaded as `Rgba8UnormSrgb` (sampling returns
  linear). Vertex colours, material tints and lights are linear. Colours in
  style settings (background, fog) are sRGB 0–255 values and converted to
  linear once on the CPU. The scene target is linear `Rgba16Float`. The
  composite pass encodes to sRGB, dithers and quantises in sRGB-encoded space,
  then decodes back to linear when the output is an `*Srgb` format (the
  hardware re-encodes to the same 8-bit value) or writes the encoded value
  directly when it is not. Gamma is applied exactly once on both paths; a test
  compares the two.
- **Emissive:** materials carry a linear `emissive` colour (from glTF
  `emissiveFactor`) added after lighting and before fog.
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
   - Textures: repeat; nearest or linear filtering and mipmaps per style
     (Milestone 5, below).
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
`RetroStyle::preset()` reports `PS1`, `PS2`, `Clean` or `Custom` (changed).
Style version 2 adds `textures`, `shading` and `shadows` sections; a
version 1 file loads with the PS1 values for them.

## PS2 workflow (Milestone 5)

One project renders in either style without rebuilding content: everything
that differs between PS1 and PS2 is a `RetroStyle` field, read per frame.

- **Presets:** `RetroStyle::ps2()` is 640×448 with aspect fit, no vertex
  snapping, perspective-correct UVs, full 8-bit colour without dither,
  linear filtering with trilinear mipmaps, per-pixel lighting, blob shadows
  at 0.6 opacity; fog and atmosphere equal the PS1 preset. The player's
  Settings > Graphics style (As authored / PS1 / PS2) applies a preset's
  look while keeping the scene's fog and atmosphere.
- **Textures:** every texture uploads a full mip chain (box-filtered in
  linear space for sRGB data). The style picks one of four samplers
  (nearest/linear × mipmaps on/off); "mipmaps off" clamps the LOD to 0, so
  switching never re-uploads. Bind groups: group 0 frame uniforms and the
  sampler (one bind group per sampler variant), group 1 per-draw uniforms
  (dynamic offset), group 2 the texture, group 3 the joint palette
  (`array<mat4x4, 64>`, dynamic offset).
- **Local lights:** point lights with a range, falloff `(1 − (d/r)²)²`
  (exactly zero at the range), at most `MAX_LOCAL_LIGHTS` = 8 per view, the
  nearest to the camera. Evaluated per vertex (PS1: the mesh's triangles
  show in the light pools) or per pixel (PS2). `shading.local_lights` turns
  them off.
- **Blob shadows:** up to `MAX_BLOB_SHADOWS` = 8 casters nearest the
  camera darken lit, upward-facing (`n.y ≥ 0.3`) surfaces below the
  caster's bottom with a soft disc that fades with height. Unlit materials
  are never shadowed. `shadows.enabled` and `shadows.opacity` control them.
- **Skinning:** the vertex layout carries `JOINTS_0` (u16×4) and weights
  (72-byte vertices). Skinned draws multiply the bind-space position and
  normal by the weighted palette before the model matrix; vertex snapping
  happens after skinning. Unskinned draws ignore the palette.

### Skeletal animation

`copperframe_assets::anim` holds the CPU side. A `Skeleton` lists joints parents
first (the importer reorders the skin's joints by node depth and remaps
`JOINTS_0`), each with a rest `Trs` and, for root joints, the transform of
any non-joint ancestors and the import scale. `palette(pose)` returns
`world_joint × inverse_bind` per joint. An `AnimationClip` has channels of
STEP or LINEAR keys per joint property; sampling outside the keys holds the
first/last key, rotations use normalised lerp. `Pose::blend` mixes two poses
per joint (translation and scale lerp, rotation nlerp). At most 64 joints.

At runtime (`copperframe_game::animation`) an entity with an `animator` and a
skinned model gets an `Animator`: the current clip (time, speed, loop,
running), an optional clip being faded out (or the rest pose), and the
palette. It advances in tick phase 8; a non-looping clip that reaches its
end logs `GameEvent::AnimationFinished` once. Behaviours call
`Ctx::play_animation(e, clip, fade_secs, looping)` (crossfade from the
current pose), `play_animation_at` (with speed), `stop_animation` (fade to
rest), `animation`, `is_animation_playing` and `animation_clips`. The
renderer receives the palette per draw; physics colliders built from a
skinned model use its rest pose. The editor viewport previews autoplaying
clips with `SceneResources::preview_poses`.

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

## Scenes (Milestone 3)

The authored scene is a plain document, not an ECS: `Scene { id, name,
revision, settings, entities }`, where each `Entity` has a persistent
`EntityId`, a name, an optional parent ID, a local `Transform` and an
optional `MeshComponent`. Entities are stored in a list whose order is the
hierarchy panel order. Decision: a document model keeps save, diff, undo
and CLI edits simple and exact; the runtime ECS (`hecs`) arrives with
gameplay in Milestone 4 and is built from the document, not the other way
round.

A `MeshComponent` has a source, either `{"kind": "primitive", "shape":
"cube"|"plane"|"sphere"|"quad", "texture": <asset id>?, "tile_size": <metres>?}` or `{"kind":
"model", "asset": <asset id>}`, and a `MaterialOverride` whose optional fields
(`tint`, `alpha`, `lit`, `flat_shading`, `allow_affine`, `double_sided`)
replace the imported material's values. Settings hold the scene's
`RetroStyle`, lighting (ambient, light direction and colour) and the play
camera start.

Scene file: `{"format": "psx_scene", "version": 1, "scene": {...}}`, pretty
JSON with stable field order (saving twice gives identical text). Loading
checks the format, rejects version 0 and versions newer than the build,
rejects unknown fields anywhere, and runs `Scene::problems`: duplicate IDs,
missing parents, cycles, depth over 64, names over 256 bytes, non-finite or
out-of-range values, invalid style. Invalid scenes are never loaded or
saved. Saving uses `atomic_write` (`.tmp`, previous file to `.bak`,
rename).

`revision` counts saves and batch applications. The editor remembers the
revision it loaded; Save refuses if the file on disk has a different
revision (another tool changed it) and offers Overwrite or Reload.

## Commands, undo and batches

Every edit, from the editor or the CLI, is a `copperframe_scene::Command`
(JSON-tagged by `"op"`): `create_entity`, `delete_entity` (with
descendants), `duplicate_entity` (subtree, fresh IDs), `rename`,
`set_transform`, `set_parent` (`keep_world_transform`, default true),
`move_entity` (reorder among siblings and reparent; moves only that entity in the list),
`set_mesh`, `set_material`, `set_settings`. `copperframe schema` lists their fields.

`apply(scene, command)` validates first and changes nothing on failure. On
success it returns a `Change`: the before/after snapshot and index of every
touched entity (and of settings if changed), plus created IDs. Commands
never reorder untouched entities and append new ones, so reverting or
re-applying a change restores the exact list. `History` keeps up to 256
changes; continuous edits (one gizmo drag, one slider drag) share a merge
key and become one undo step. The dirty flag compares the current undo
position with the one recorded at save, so undoing back to the saved state
is clean.

A command batch is
`{"format": "psx_command_batch", "version": 1, "expected_revision": N?,
"commands": [...]}`. `apply_batch` runs on a copy: if any command fails or
the result is invalid, nothing is written and the error names the command
index. On success the revision increments and the report lists created IDs.
`copperframe apply` refuses while the editor lock is held (exit 3), and refuses a
stale `expected_revision` (exit 3).

## Projects and assets

A project directory contains `project.json` (`{"format": "psx_project",
"version": 1, "name", "startup_scene"}`), `assets/` and `scenes/`. Every path
stored in a project is relative to the project root with `/` separators, and
references may not leave the project.

Each source file under `assets/` has a sidecar `<file>.meta.json`
(`{"format": "psx_asset_meta", "version": 1, "id", "importer":
"gltf"|"texture", "settings": {"scale", "max_texture_size"}}`). The asset ID
lives in the sidecar, so moving a source together with its sidecar keeps
scene references. `AssetDatabase::scan` builds the ID → path table and
reports duplicate IDs, orphaned sidecars and unregistered sources.

Import (`copperframe_assets::import`) produces CPU data only (`ImportedModel`: parts
of `CpuMesh` + `ImportedMaterial` + optional `CpuTexture`); the runtime
uploads it. Importers never panic on input: buffer and accessor reads are
bounds-checked by our own reader, and the whole import runs inside
`catch_unwind` as a last guard against panics in dependencies.

glTF support matrix:

| Feature | Handling |
| --- | --- |
| `.gltf` (external or `data:` buffers/images), `.glb` | supported |
| Triangle lists, strips, fans | supported (strips/fans converted per the glTF spec) |
| POSITION, NORMAL, TEXCOORD_0, COLOR_0; u8/u16/u32 indices; normalized integer attributes | supported; normals computed when absent |
| Node hierarchy of the default scene | baked into vertices; mirrored nodes get their winding flipped |
| Base colour factor and texture (PNG/JPEG), alpha mode/cutoff, double-sided, emissive factor, `KHR_materials_unlit` | supported |
| One skin per model (≤ 64 joints), JOINTS_0/WEIGHTS_0 (renormalised), inverse bind matrices, animation channels on joints (translation/rotation/scale; STEP, LINEAR; CUBICSPLINE imported as LINEAR through its values with a warning) | supported (importer version 2) |
| Points, lines, morph targets, a second skin, channels on non-joint nodes, JOINTS_1/WEIGHTS_1, UV1+, metallic/roughness, normal/occlusion/emissive textures, samplers (filtering is a style setting) | ignored with a warning |
| Other required extensions (Draco, meshopt, …), sparse accessors | rejected |
| Out-of-range buffers/views/accessors/indices, non-finite values, `http(s)` URIs, paths outside the project | rejected |

Textures are decoded with `image` under limits (8192 px per side, 512 MiB
allocation), then reduced with nearest sampling to `max_texture_size` (the
retro look keeps hard texels).

`validate_project` checks the project file, every sidecar, every import and
every scene (schema, hierarchy, asset references that are unknown or of the
wrong kind) and returns diagnostics with severities; `copperframe validate` exits 1
if any is an error.

## GPU scene resources and picking

`copperframe_runtime::scene_gpu::SceneResources` uploads each referenced asset once
and turns a `Scene` into renderer draw items using world matrices.
A missing or failed asset draws as an unlit magenta/black checker cube so it
stays visible and selectable. If a reimport fails after a successful one,
the previous GPU data is kept and the status records the failure.
Primitives use the built-in meshes.

Picking casts a ray from a viewport pixel: the editor maps the pixel
through the same layout as the composite pass (letterbox and integer scale,
in physical pixels) to internal NDC, unprojects with the camera, and tests
the ray against CPU copies of each entity's triangles (Möller–Trumbore,
both faces). The nearest hit wins.

## Texture tiling

Primitive meshes are shared unit shapes, so `tile_size` is applied per draw and
needs no new mesh or re-upload when an entity is scaled. `scene_gpu::uv_tile`
turns the entity's world scale and tile size into repeats along local X, Y and
Z (`Material::uv_tile`, `DrawUniforms::uv_tile`), and the vertex shader scales a
vertex's UVs by the two axes its face spans, chosen by its normal. This only
suits flat faces, so spheres ignore it. Repeats are measured from each face's
own corner, so neighbouring faces are not continuous across an edge.

## Editor

`copperframe_editor` runs winit + egui (egui-winit, egui-wgpu). Each frame the world
renders through `RetroRenderer` into the Viewport panel's rectangle, then egui
draws on top using a non-sRGB view of the swapchain
(`Presenter::overlay_format`) because egui outputs gamma-space colours.

Panels live in an `egui_tiles` tree (`dock.rs`). Every panel paints its own
background except the Viewport, which must stay transparent because the image
is underneath egui; if the Viewport is closed or behind another tab, nothing
is rendered. Closing a panel hides its tile, so it reopens in place. The
layout and `ToolPrefs` are saved to `editor_layout.json` in the engine's data
folder (`format: copperframe_editor_layout`, `version: 1`) when a project
closes; an unreadable or newer file falls back to the defaults, because it is a
convenience and must never stop the editor opening.

The Viewport defaults to `ViewFrame::Fill`: the internal size follows the
panel's shape at a whole-number block size, keeping at least the style's line
count. This lives in `ViewportGeometry`, not `ViewLayout`: the game's own
presentation is unchanged and `ViewFrame::Game` shows it. `ViewLook::Sharp`
derives a style for the view only and never writes the scene. Transform tool
maths and snapping are in `gizmo.rs`, free of egui; every gizmo edit is a
`Command::SetTransform`, merged per drag.
`EditorSession` owns the scene, history, asset database and log; the UI only
issues commands through it. While open the editor holds
`<project>/.psx/editor.lock` (its pid, under an exclusive OS file lock); a
second editor or `copperframe apply` refuses while a running editor holds
it. The OS drops the lock when the editor exits or crashes, so a leftover
file is ignored and replaced automatically; `--steal-lock` only matters for
a hung editor that is still running.

The editor redraws on demand: on input, when egui asks for a repaint (with
its delay), and every frame only while something moves (splash, viewport
camera, gizmo drag). The animation preview runs at 30 Hz (10 Hz when the
window is in the background). While a game runs or a background job
reports, it wakes at 10 Hz to read their output. The status bar shows
"idle" when it is not redrawing continuously. Smoke runs with
`--frames` always redraw continuously. Closing with unsaved changes
asks Save / Quit without saving / Cancel.

## Gameplay runtime (Milestone 4)

### Game crates and the launcher

A project's gameplay code is a normal Rust binary crate in `<project>/game/`
(written by `copperframe new`). It implements `copperframe_game::Game` (title, behaviour
registration, optional per-tick hooks) and its `main` is one line:
`copperframe_game::main(|| Box::new(MyGame), concat!(env!("CARGO_MANIFEST_DIR"), "/.."))`. Every
game executable therefore accepts the same arguments:

| Flag | Meaning |
| --- | --- |
| `--project <dir>` | Project to load. Default: `<exe dir>/data` if it holds `project.json` (an export), else the development project |
| `--scene <file>` | Scene to start in (default: the project's startup scene) |
| `--play` | Editor play mode: skip the title screen, never read or write saves |
| `--schema` | Print the behaviour schema (`psx_game_schema`) and exit |
| `--headless --script <file> [--ticks n] [--report <file>]` | Run without window, GPU or audio; print a `psx_run_report` |
| `--frames <n>` | Exit after n frames (smoke tests) |
| `--user-dir <dir>` | Keep settings and saves here instead of the OS user folders |
| `--control stdin` | Editor control: `pause`, `resume`, `step [n]`, `quit` lines on stdin; status lines on stdout; stdin EOF closes the game (windowed only) |

Exit codes: 0 ok, 1 the scene cannot be played or an expectation failed, 2
usage, 4 project or file error. Logs go to stderr so stdout JSON stays clean.

`copperframe_assets::game::GameTarget` finds what to run: `game/Cargo.toml` is run
with `cargo run` from the crate folder (so its `rust-toolchain.toml`
applies; arguments are absolute paths); without a crate the stock
`copperframe_player` next to the running tool is used. The editor and the CLI only
talk to games through this process boundary.

### Simulation

`Simulation` builds a `hecs` world from a `Scene` (read only, never
written back) and advances it at 60 Hz. Tick order (also in
`copperframe_game::sim` docs): start new behaviours; game and behaviour
`fixed_update`; interaction (aim ray picks the nearest interactive entity
within reach, the interact action calls `on_interact`); deferred commands;
world transforms and kinematic targets; physics step; trigger events
(`on_trigger`) and commands; final transforms, audio emitters and listener.
Behaviours change the world only through `Ctx` (transforms, state map,
messages, sounds, spawn/despawn, finish), so headless and windowed runs are
identical for the same input. The renderer interpolates between the last
two ticks.

Transform policy: static colliders are built once from world transforms.
Kinematic bodies follow their entity's transform (set by behaviours) as
next-tick targets; dynamic bodies write poses back to the entity. Collider
sizes are multiplied by the entity's world scale. The player is a capsule
moved by rapier's `KinematicCharacterController`; because that controller
ignores kinematic bodies, a second shape-cast sweep stops it against moving
doors and platforms.

`GameState` is a string→JSON map that behaviours read and write; it is
what saves contain.

### Input

Named actions (`move_forward`, `move_back`, `move_left`, `move_right`,
`jump`, `interact`, `pause`, plus any the game declares) map to keys and
mouse buttons in `<project>/input.json` (`psx_input` v1). The player's
`settings.json` may overlay user bindings. Mouse look uses raw device
motion while the cursor is captured; sensitivity and invert-Y are user
settings. Losing focus releases the cursor and pauses. Gamepads are not
supported yet.

### Audio

kira, through `copperframe_audio::Mixer`: a main track (Master, with the
limiter and a meter) and one sub-track per bus (Music, SFX, Voice,
Ambient), each with a meter. The project's `audio` settings set each
track's volume and mute; the player's music and effects volumes (0..1)
multiply on top (effects also scales Voice and Ambient). A
scene `audio` component plays a sound asset on its bus
(volume in dB, looping, autoplay, optionally spatial with min/max
distance); behaviours can play, stop and fade sounds through `Ctx`. Spatial emitters follow their entity and the
single listener follows the camera. When no output device exists the
player logs a warning and runs silently; headless runs never open a
device. Pausing pauses the effects track with a short fade.

### Player, settings and saves

`copperframe_game::player` owns the window: title screen (New game / Continue /
Settings / Quit), HUD (crosshair, interaction prompt, messages), pause
menu, settings (volumes, sensitivity, invert-Y, fullscreen, vsync, key
rebinding, graphics style) and the finished screen.
Settings (`psx_player_settings` v1) live in the OS config folder and saves
(`psx_save` v1: scene path + `GameState`) in the OS data folder, under
`Copperframe Engine/<game title slug>`. Writes are atomic; a damaged file is
renamed to `*.json.bad` and defaults are used. A save is written when the
player quits to the title or quits from the pause menu, never in `--play`
mode, and is deleted when the game finishes.

### Formats added in Milestone 4

| Format | Where | Notes |
| --- | --- | --- |
| `psx_scene` v2 | `scenes/*.scene.json` | Adds `collider` (box, sphere, capsule, or triangle mesh for static bodies; static/kinematic/dynamic; sensor; offset), `audio` and `behaviors` (kind + scalar params). v1 files load and migrate |
| `psx_input` v1 | `<project>/input.json` | Action → bindings |
| `psx_game_schema` v1 | stdout of `--schema`, cached in `.psx/game_schema.json` | Behaviour kinds with typed fields; drives the editor inspector and `copperframe schema --project` |
| `psx_script` v1 | e.g. `scripts/*.script.json` | Steps `{ticks, hold, press, look}` plus `expect {finished, events}`; events match by subset |
| `psx_run_report` v1 | stdout or `--report` | Ticks, outcome, events, warnings, camera, `passed`, `failures` |
| `psx_player_settings` v1, `psx_save` v1 | OS user folders | See above |
| `psx_export` v1 | `export.json` in an export folder | Marks a folder that `copperframe build --replace` may overwrite |

### Formats changed in Milestone 5

| Format | Notes |
| --- | --- |
| `psx_scene` v3 | Optional `light` (`color` linear 0..1, `intensity` 0..16, `range` 0.05..100 m), `animator` (`clip`, `speed` −10..10, `looping`, `autoplay`; needs a model mesh) and `shadow` (`radius`, `offset`) entity fields; the style inside gains the v2 sections. v1 and v2 files load unchanged |
| `psx_style` v2 | `textures {filter: nearest|linear, mipmaps}`, `shading {per_pixel, local_lights}`, `shadows {enabled, opacity}`; v1 files get PS1 values |
| Importer version 2 | glTF skins and clips; `copperframe import` reports `joints` and `clips` |
| `psx_player_settings` v1 | Adds `graphics_style` (`scene`, `ps1`, `ps2`), defaulted when absent |
| Command batches v1 | New ops `set_light`, `set_animator`, `set_shadow` |

`copperframe validate` warns when an animator names a clip the model does not have
(listing the clips) or the model has no skin.

### Formats added in Milestone 6

| Format | Where | Notes |
| --- | --- | --- |
| `copperframe_play_status` v1 (0.10) | stdout lines after `@copperframe ` from `--control stdin` | `state` (`running`, `paused`, `stepping`, `quitting`), `tick`, optional `error` |
| `psx_bench` v1 | stdout of `--bench` or `--report` | Adapter (name, backend, device type, driver), output size, frames, build profile, and per style (`scene`, `ps1`, `ps2`): internal size, draws, triangles, `tick` and `frame` stats (`mean_ms`, `p50_ms`, `p95_ms`, `max_ms`) |

See [MIGRATION.md](MIGRATION.md) for every format version.

## Crash and bug reports

`copperframe_core::report` is installed first thing in every binary's
`main`. It sets a panic hook that, after the default message, writes
`crash_reports/<app>-<unix secs>-<pid>.txt` (system summary, adapter, panic
message and location, thread, forced backtrace, last 200 log lines) and keeps
the newest 20. The logging subscriber feeds every event, without colour
codes, to `report::record_log`, which keeps the in-memory tail and writes
`logs/<app>.log` (the previous run becomes `<app>.previous.log`; capped at
16 MiB). The log file is opened on the first line, so tools that do not log
leave none.

| Binary | Data folder | Report address |
| --- | --- | --- |
| editor, sandbox, CLI | `report::engine_data_dir()` (`%APPDATA%\Copperframe Engine\copperframe-engine\data`, `~/.local/share/copperframe-engine`) | `report::REPORT_EMAIL` |
| games (`copperframe_game::main`, including the stock player) | the game's `UserDirs` data folder | none |

Games never offer the engine address: an exported game belongs to its
author. When the editor runs Play it sets `COPPERFRAME_CRASH_DIR` to its own
data folder, so a crash in the game lands where the editor looks; after a
Play session exits with an error, and at startup, the editor shows unseen
reports (`*.txt`, not `*.seen.txt`) in a dialog.

Nothing is sent by the engine. "Email report" and Help → "Report a bug…"
build a `mailto:` URL (RFC 6068 percent-encoding, capped at 1 900
characters with a note to attach the full file when cut) and hand it to
the OS (`rundll32 url.dll,FileProtocolHandler` on Windows, `open` on macOS,
`xdg-open` elsewhere, never through a shell). The user reads and sends the
draft in their own mail program, so no credentials ship in the public
repository or binaries. A server-side collector would be a separate,
opt-in feature.

Release builds keep line tables (`[profile.release] debug =
"line-tables-only"`); the itch.io bundle ships each `.pdb` next to its
`.exe` so Windows backtraces have file and line numbers.

## Performance measurement and lifecycle (Milestone 6)

`copperframe_game::bench` runs a fresh `Simulation` per style with the same input
(idle or a script), renders each tick's state offscreen into an sRGB target
at the requested output size through the same `SceneResources` and
renderer calls as the player, and waits for the GPU after each frame. The
first 10 frames per style are not measured (pipeline creation, uploads).
`copperframe bench` builds the game in release first so numbers are not debug
numbers.

Caches and their lifetimes:

| Cache | Owner | Released |
| --- | --- | --- |
| CPU models (colliders, animation) | `Simulation` assets | on scene load, for assets the new scene does not reference |
| Decoded sounds | `Audio` | same (`Audio::retain_sounds`); playing sounds finish |
| GPU models and textures | `SceneResources` | player: when the scene path changes (`release_unused`); editor: kept for the session (undo may bring them back) |
| Primitive meshes, fallback textures | `SceneResources` | never (fixed size) |

### Play mode and export

Editor Play (F5) writes the current, possibly unsaved, scene to
`.psx/play/<scene>` and runs the game with `--project <abs> --scene <copy>
--play --control stdin`; the child's stdout/stderr stream into the console.
The authored scene and the editor's undo history are never touched.

Pause (F6), Resume and Step (F7) are lines on the child's stdin, defined
with the status reply in `copperframe_assets::play_control` (shared because
the editor does not link `copperframe_game`). The game's reader thread
feeds `copperframe_game::control::Pacing`: while paused only step ticks run
(at most 8 per frame), through the same `Simulation::tick` as normal play,
so stepping is deterministic like headless runs. Editor pause also pauses
audio, releases the cursor and suppresses the focus-loss pause menu. The
game prints `@copperframe {copperframe_play_status v1}` on stdout when it
has loaded, after each command and when a step finishes; the editor takes
those lines out of the console and shows the state on the toolbar.

Stop sends `quit` and closes stdin, so unload and `Game::shutdown` run, and
kills the child after 3 s if it is still there (at once while it is still
compiling, before its first status). A game whose stdin ends exits, so a
game left by a crashed editor, or by killing `cargo run` while the game
runs under it, closes by itself.

`copperframe build` (and Game > Export in the editor) builds the game in release,
then writes `<out>/<exe>`, `data/` (project.json, input.json, scenes,
assets), `NOTICES.txt` (licences of every crate in the binary's
dependency graph, from `cargo metadata`), `README.txt` and `export.json`.
It refuses to write into a non-empty folder that has no `export.json`, and
replaces an earlier export only with `--replace`. Unless `--no-verify` is
given, the export is then run headless from a temporary folder whose name
contains spaces, optionally with a script.
