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
- Dependency direction: `psx_core` and (later) `psx_scene` must not depend on a
  window, GPU or editor. `psx_render` must not depend on winit.
- Errors: no panics for recoverable failures (bad assets, missing devices,
  surface loss). Use `thiserror` in libraries, `anyhow` at application edges.
- `unsafe_code` is denied workspace-wide. If it ever becomes necessary, isolate
  it, document the invariants, and allow it locally.
- Do not land placeholder subsystems. A TODO or mocked success is not a feature.
- Retro effects stay independently switchable through `RetroStyle`
  (`psx_core::style`). A new effect gets a settings field with validation, a
  sandbox key, and an image test in `crates/psx_render/tests/retro_images.rs`.
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
GPU, install Mesa lavapipe (`mesa-vulkan-drivers`). Report exactly which checks
ran and which were skipped.
