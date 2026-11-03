# PSX Engine

A Rust game engine for making original 3D games with PlayStation 1 and
PlayStation 2-inspired visuals on modern PCs. It is not an emulator or a
console SDK. "PSX Engine" is a working name.

**Status: Milestone 3 (authoring loop) — early prototype, not production-ready.**
The PS1-style renderer (low internal resolution with integer upscaling,
vertex snapping, affine texture mapping, colour reduction, ordered
dithering, fog, opaque / cutout / blended materials) now draws scenes stored
as versioned JSON files. A scene editor (hierarchy, inspector, assets,
console, viewport picking, translate gizmo, undo/redo, save/load) and the
`psx` command-line tool share one command layer, and static glTF models and
PNG/JPEG textures can be imported. Gameplay, physics, audio and play mode
are not implemented yet. See [docs/STATUS.md](docs/STATUS.md) and
[docs/ROADMAP.md](docs/ROADMAP.md).

## Requirements

- Rust via [rustup](https://rustup.rs). The toolchain version is pinned in
  `rust-toolchain.toml` and installed automatically on first build.
- A GPU and driver supporting Vulkan, DirectX 12, Metal or OpenGL ES 3
  (through `wgpu`). Only WebGPU default limits are required.
- Windows 11 x86-64 is the primary target. Linux builds and runs (tested on
  X11 with Mesa lavapipe); macOS is untested.
- Linux only: X11/Wayland client libraries, including `libxkbcommon-x11`.

## Editor and command-line tool

Open the example project in the editor:

```sh
cargo run -p psx_editor --release -- examples/room
```

Or create a new project and open it:

```sh
cargo run -p psx_cli --release -- new my_game --name "My game"
cargo run -p psx_editor --release -- my_game
```

A project is a folder with `project.json`, `assets/` (source files plus
`.meta.json` sidecars that hold each asset's stable ID) and `scenes/`.
Copy `.gltf`/`.glb` models and `.png`/`.jpg` textures into `assets/`; the
editor's Assets tab (or `psx import`) registers them.

Editor controls:

| Input | Action |
| --- | --- |
| Left click in the viewport | select (click empty space to clear) |
| Drag a gizmo arrow | move the selection along X (red), Y (green) or Z (blue); with Edit > Snap on, moves in steps (0.25 m by default) |
| Right mouse drag | look; with `W` `A` `S` `D` `E` `Q` to fly, `Shift` faster |
| Mouse wheel | dolly |
| `Ctrl+S` | save |
| `Ctrl+Z` / `Ctrl+Shift+Z` or `Ctrl+Y` | undo / redo |
| `Ctrl+D` / `Delete` | duplicate / delete the selection (with children) |

The hierarchy panel selects; the inspector's parent box reparents (keeping
the world position). The inspector edits name, transform
(position, Euler rotation in degrees, scale), mesh and material overrides;
Scene settings edit the style preset, lighting and camera start. The editor
holds `.psx/editor.lock` while open, warns before closing with unsaved
changes, and refuses to overwrite a scene that changed on disk unless you
choose to.

`psx` commands (add `--json` for machine-readable output; exit codes 0 ok,
1 validation or batch failure, 2 usage, 3 conflict or editor lock, 4 I/O):

```sh
psx new DIR [--name NAME]
psx validate [--project DIR]                 # project, metadata, imports, scenes
psx import [--project DIR] [FILE]            # register + test-import assets
psx inspect [--project DIR] [SCENE]          # entities with IDs and components
psx apply [--project DIR] [--scene SCENE] [--dry-run] BATCH  # all-or-nothing; BATCH may be -
psx schema                                   # formats, components, commands
```

Use `cargo run -p psx_cli --release -- <command>` or the built
`target/release/psx`. `examples/room/build_room.batch.json` is the batch
that built the example scene. Command batches and file formats are
documented in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Sandbox

The renderer comparison scene from Milestone 2:

```sh
cargo run -p psx_sandbox --release
```

Camera: click in the window to capture the mouse, move the mouse to look,
`W` `A` `S` `D` to move, `E`/`Q` up/down, `Shift` to move faster, `Esc` to
release the mouse. Close the window to quit.

Style (the window title shows the current state):

| Key | Action |
| --- | --- |
| `1` | low-resolution rendering on/off |
| `2` | vertex snapping on/off |
| `3` | affine texture mapping on/off |
| `4` | colour reduction on/off |
| `5` | dithering on/off |
| `6` | fog on/off |
| `7` | integer scaling / aspect fit |
| `[` `]` | colour bits per channel −/+ |
| `-` `=` | snap grid finer/coarser |
| `P` | next preset (PS1, Clean) |
| `C` | split view: current style on the left, effects off on the right |
| `F5` / `F9` | save / reload style settings |
| `F1` | print the key list to the log |

Options: `--style PATH` chooses the style settings file (default
`psx_style.json` in the current directory; loaded at start if it exists),
`--split` starts in split view, and `--frames N` exits after presenting N
frames (for smoke tests).
Set `RUST_LOG=debug` for more logging, and `WGPU_BACKEND=vulkan|dx12|metal|gl`
to force a graphics backend.

## Tests

```sh
cargo test --workspace
```

The offscreen image tests (one per effect), the comparison-scene camera
sweep and the scene-resource test need a graphics adapter and print `SKIPPED` without one. Set
`PSX_REQUIRE_GPU=1` to make a missing adapter a failure. Debug images land
in `target/tmp/`.

## Layout

| Path | Purpose |
| --- | --- |
| `crates/psx_core` | Fixed-step timing, stable IDs, atomic file writes, retro style settings and their JSON file format. No GPU/window deps. |
| `crates/psx_scene` | Scene document (entities, hierarchy, transforms, mesh/material components), versioned scene file, commands, undo/redo history, command batches. No GPU/window deps. |
| `crates/psx_assets` | Projects, asset metadata and database, static glTF and texture import, validation. No GPU/window deps. |
| `crates/psx_render` | wgpu device, surface presenter, retro renderer (scene + composite passes), materials, fit policy, camera, mesh/texture data, WGSL shaders. |
| `crates/psx_runtime` | winit event loop, input sampling, fly camera, comparison scene, style keys, frame loop, logging, GPU scene resources and picking. |
| `crates/psx_editor` | egui scene editor (library + `psx_editor` binary). |
| `apps/psx_cli` | `psx` command-line tool. |
| `apps/psx_sandbox` | Sandbox application showing the comparison scene. |
| `examples/room` | Example project: a small room built from imported models and a texture. |

## License

MIT, see [LICENSE](LICENSE).
