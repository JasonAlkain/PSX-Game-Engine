# Architecture overview

This page is the map. The detailed design notes (rendering passes, uniform
layouts, surface handling, the tick order, caches) live in
[docs/ARCHITECTURE.md](https://github.com/JasonAlkain/PSX-Game-Engine/blob/main/docs/ARCHITECTURE.md),
which is kept up to date with every change.

## Crates

```text
apps/copperframe_sandbox ─► runtime
crates/copperframe_editor ─┬─► runtime ─┬─► render ─► core
 (lib + bin)               │            ├─► assets ─┬─► scene ─► core
apps/copperframe_cli ──────┴─► assets   └─► scene   └─► core

apps/copperframe_player ─┐
game crates ─────────────┴─► game ─┬─► runtime (window, renderer, scene GPU data)
 (examples/room/game)              └─► assets, scene, core
```

Dependencies only point downward.

| Crate | Responsibility | Must not depend on |
| --- | --- | --- |
| `copperframe_core` | Fixed-step timing, stable IDs, atomic file writes, `RetroStyle` and its file format, crash reports | GPU, window |
| `copperframe_scene` | The scene document, components, the versioned scene file, `Command`s, undo/redo `History`, batches | GPU, window |
| `copperframe_assets` | Projects, asset sidecars and database, glTF/texture/audio import, animation data, validation, finding and running a project's game, export | GPU, window |
| `copperframe_render` | wgpu device and surface, the retro renderer (scene and composite passes), materials, meshes, WGSL shaders | winit |
| `copperframe_runtime` | winit event loop, input, fly camera, logging, GPU resources for scenes, picking, branding | |
| `copperframe_game` | Gameplay: the `Game`/`Behavior` API, the `hecs` simulation, rapier3d physics, kira audio, input actions, the player app, headless runs, benchmarks, the shared game `main` | |
| `copperframe_editor` | The egui editor (a library, so its session logic is tested without a window, plus the binary) | `copperframe_game` |
| `apps/copperframe_cli` | The `copperframe` tool | `copperframe_game` |
| `apps/copperframe_player` | The stock player for projects without a game crate | |
| `apps/copperframe_sandbox` | The renderer comparison scene | |

## Key decisions

**The scene is a document, not an ECS.** Scenes are plain data: a list of
entities with optional components. That keeps saving, diffing, undo and CLI
edits exact. At play time the `Simulation` builds a `hecs` world from the
document and never writes back.

**One command layer.** Every edit, from the editor or the CLI, is a
`copperframe_scene::Command`. Commands validate first and change nothing on
failure; undo restores exact state. There is no other edit path.

**Games are separate processes.** The editor and CLI never link gameplay
code. They build and run the project's game executable and talk to it
through its [command-line flags](../reference/game-options.md) and JSON
(`--schema`, `--headless`, `--bench`). A broken game cannot crash the
editor, and game crates build with their own toolchain.

**Style is data read per frame.** Everything that differs between PS1 and
PS2 is a `RetroStyle` field. Textures always upload full mip chains; the
style only picks samplers and shader paths. Switching styles never
reimports or re-uploads.

**Deterministic simulation.** A fixed 60 Hz tick; behaviours change the
world only through `Ctx`; input arrives as actions. The same input gives
the same result windowed and headless, so scripted tests are meaningful.

**Versioned, strict files.** Every file has a `format` and `version`, older
versions migrate on load, newer ones and unknown fields are rejected, and
saves are atomic.

## Rendering in one paragraph

Each view renders the world into a linear `Rgba16Float` target at the
internal resolution (vertex snapping, affine UVs, fog, lighting and
skinning happen here), then a composite pass draws it into the window:
each output pixel fetches exactly one internal texel, encodes to sRGB,
applies the Bayer dither and quantises, so every upscaled block is
uniform. Integer or fit placement decides the rectangle and letterbox bars.
See "Rendering" and "PS2 workflow" in ARCHITECTURE.md.

## Conventions

Right-handed, +Y up, metres; cameras look down −Z; wgpu clip space with
depth 0 at near; counter-clockwise front faces; UV origin top-left;
textures sRGB, lighting linear, style colours sRGB 0–255. Details are in
ARCHITECTURE.md "Conventions".

## Where to start reading code

| To understand | Read |
| --- | --- |
| The renderer | `crates/copperframe_render/src/retro.rs` and `src/shaders/` |
| Style settings | `crates/copperframe_core/src/style.rs` |
| Scene data and commands | `crates/copperframe_scene/src/scene.rs`, `command.rs`, `batch.rs` |
| The game API | `crates/copperframe_game/src/api.rs`, then `sim.rs` |
| The editor | `crates/copperframe_editor/src/session.rs` (logic), `ui.rs` (panels) |
| A complete game | `examples/room/game/src/lib.rs` |
