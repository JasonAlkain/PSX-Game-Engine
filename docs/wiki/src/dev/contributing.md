# Contributing

Bug reports, ideas and pull requests are welcome on
[GitHub](https://github.com/JasonAlkain/PSX-Game-Engine). Use the issue
templates for [bugs](https://github.com/JasonAlkain/PSX-Game-Engine/issues/new?template=bug_report.yml)
and [feature requests](https://github.com/JasonAlkain/PSX-Game-Engine/issues/new?template=feature_request.yml).

The repository's
[AGENTS.md](https://github.com/JasonAlkain/PSX-Game-Engine/blob/main/AGENTS.md)
is the authoritative list of rules for people and AI agents changing the
code. This page summarises it.

## Setting up

Follow [Installing and building](../getting-started/installing.md). For the
GPU tests on Linux without a GPU, install Mesa's software Vulkan driver:

```sh
sudo apt install mesa-vulkan-drivers libasound2-dev
```

## Checks before a change is done

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
COPPERFRAME_REQUIRE_GPU=1 cargo test --workspace   # drop the variable if no adapter exists
cargo build --workspace --release
cargo run -p copperframe_cli -- validate --project examples/room
```

`COPPERFRAME_REQUIRE_GPU=1` turns a skipped GPU test into a failure. Say in
your pull request which checks ran and which were skipped.

CI runs the same on Linux (with lavapipe, plus the demo walkthrough and a
short benchmark) and on Windows, and builds this wiki.

## Rules that matter most

- **Dependencies:** versions live only in the root `Cargo.toml`
  `[workspace.dependencies]`; commit `Cargo.lock`. wgpu, winit and egui
  must stay mutually compatible.
- **Dependency direction:** `core`, `scene` and `assets` never depend on a
  window, GPU or editor; `render` never depends on winit; the editor and CLI
  never link `copperframe_game`.
- **One command layer:** every scene edit is a `copperframe_scene::Command`.
  Commands leave the scene untouched on failure, never reorder entities they
  do not touch, and append new ones.
- **File formats:** a change bumps the format version, adds a migration
  from the previous version, a fixture of the old version, a test that loads
  it, and an entry in `docs/MIGRATION.md`. Saves go through
  `copperframe_core::atomic_write`. The `psx_*` names stay.
- **No panics on bad input:** importers must not panic on any file; add a
  fixture test for each new failure mode. Use `thiserror` in libraries and
  `anyhow` at application edges. `unsafe_code` is denied.
- **No placeholders:** a TODO or a mocked success is not a feature.
- **Retro effects stay independent:** a new effect gets a `RetroStyle` field
  with validation, a sandbox key, and an image test
  (`crates/copperframe_render/tests/retro_images.rs`, or `ps2_images.rs` for
  style-dependent features).
- **Gameplay:** the simulation never writes the authored scene and behaves
  the same headless and windowed. A change to gameplay, physics or the demo
  keeps `cargo test -p lever_room` (the walkthrough) passing.
- **Reference images:** a visual change to the example room regenerates them
  with `COPPERFRAME_UPDATE_GOLDEN=1 cargo test -p copperframe_runtime --test
  scene_resources` and says why in the commit. Never loosen the tolerance.
- **Logs** go to stderr; `--schema`, `--headless` and `--bench` print only
  JSON on stdout.
- **Crash reports** never send anything: no SMTP credentials, API keys or
  upload endpoints in the engine.

## Documentation

Keep documentation in the same pull request as the change:

| Changed | Update |
| --- | --- |
| Anything a user sees (editor, CLI, player, file formats, API) | The relevant page in this wiki (`docs/wiki/src/`) |
| A convention or subsystem design | `docs/ARCHITECTURE.md` |
| A file format or the game API | `docs/MIGRATION.md` |
| What has been verified | `docs/STATUS.md`, and `docs/PLATFORMS.md` for platform results |

To preview the wiki locally, install [mdBook](https://rust-lang.github.io/mdBook/)
and serve it:

```sh
cargo install mdbook --version 0.4.52 --locked
mdbook serve docs/wiki --open
```

New pages must be listed in `docs/wiki/src/SUMMARY.md`. Images go in
`docs/wiki/src/images/`.

## Tests worth knowing

| Test | Covers |
| --- | --- |
| `crates/copperframe_render/tests/retro_images.rs`, `ps2_images.rs` | Every retro effect, rendered offscreen and checked pixel by pixel |
| `crates/copperframe_runtime/tests/scene_resources.rs` | The example room in PS1 and PS2 against reference images (llvmpipe only) |
| `crates/copperframe_editor` session tests | Create, edit, save, reopen, undo; CLI edits of the same project |
| `apps/copperframe_cli/tests/cli.rs` | Every CLI command end to end |
| `crates/copperframe_assets/src/import/gltf/tests.rs` | glTF failure modes |
| `examples/room/game/tests/walkthrough.rs` | The demo can be completed |
