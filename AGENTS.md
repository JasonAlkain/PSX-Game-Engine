# Agent and contributor notes

Read this before changing the repository.

- Brief: the product brief lives outside the repo; the milestone plan and status
  are in `docs/ROADMAP.md` and `docs/STATUS.md`. Keep both accurate.
- Architecture and conventions (coordinates, depth, colour space, buffer
  layout): `docs/ARCHITECTURE.md`. Update it when a convention changes.
- Toolchain: pinned in `rust-toolchain.toml` (stable, edition 2024). No nightly.
- Dependencies: versions live only in the root `Cargo.toml`
  `[workspace.dependencies]`. Crates use `foo.workspace = true`. Commit `Cargo.lock`.
  wgpu, winit and egui versions must stay mutually compatible (see ARCHITECTURE).
- Dependency direction: `psx_core`, `psx_scene` and `psx_assets` must not
  depend on a window, GPU or editor. `psx_render` must not depend on winit.
  The editor and the CLI share one command layer: every scene edit is a
  `psx_scene::Command`. Do not add an edit path that bypasses it.
- File formats (scene, batch, project, asset meta, style) are versioned JSON
  with a `format` tag. Changing a format means bumping its version, adding a
  migration from the previous version, and a test that loads the old one.
  Saves go through `psx_core::atomic_write`.
- Commands must leave the scene untouched on failure, must not reorder
  entities they do not touch, and append new entities (undo relies on it).
- Importers must not panic on any input; add a fixture test for each new
  failure mode (`crates/psx_assets/src/import/gltf/tests.rs`).
- Errors: no panics for recoverable failures (bad assets, missing devices,
  surface loss). Use `thiserror` in libraries, `anyhow` at application edges.
- `unsafe_code` is denied workspace-wide. If it ever becomes necessary, isolate
  it, document the invariants, and allow it locally.
- Do not land placeholder subsystems. A TODO or mocked success is not a feature.
- Retro effects stay independently switchable through `RetroStyle`
  (`psx_core::style`). A new effect gets a settings field with validation, a
  sandbox key, and an image test in `crates/psx_render/tests/retro_images.rs`.
- `examples/room` must keep passing `psx validate --project examples/room`.
- Gameplay lives in `psx_game`. The editor and CLI must not link it: they
  run a game executable (`psx_assets::game::GameTarget`) as a child process
  and read its `--schema`. Keep `psx_game::launch` flags and exit codes
  stable; document new ones in ARCHITECTURE.
- The simulation must never write the authored `Scene`, and must behave the
  same headless and windowed for the same input. Behaviours change the world
  only through `Ctx`.
- A change to gameplay, physics or the demo must keep
  `psx test-scene --project examples/room --script
  examples/room/scripts/walkthrough.script.json` passing (it also runs as
  `cargo test -p lever_room`).
- Behaviour parameters are scalars (bool, number, string, entity); add a
  `FieldDescriptor` for each so the editor can show it.
- PS1/PS2 differences must stay style settings read per frame: switching
  styles may not require reimporting or re-uploading content. Renderer
  features that differ by style get an image test in
  `crates/psx_render/tests/ps2_images.rs`.
- Animation state lives in `psx_game::animation::Animator`; behaviours
  drive it through `Ctx::play_animation`/`stop_animation`. The renderer
  only receives joint palettes.
- Logs go to stderr; stdout of `--schema` and `--headless` is JSON only.
- Uniform structs shared with WGSL use only `vec4`/`mat4x4` members and are
  checked by the naga layout tests in `psx_render::retro`.

## Checks before a change is done

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
PSX_REQUIRE_GPU=1 cargo test --workspace   # drop the env var if no adapter exists
cargo build --workspace --release
```

`PSX_REQUIRE_GPU=1` turns a skipped GPU test into a failure. On Linux without a
GPU, install Mesa lavapipe (`mesa-vulkan-drivers`). Building
`kira` on Linux needs `libasound2-dev`. Report exactly which checks
ran and which were skipped.
