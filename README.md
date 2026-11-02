# PSX Engine

A Rust game engine for making original 3D games with PlayStation 1 and
PlayStation 2-inspired visuals on modern PCs. It is not an emulator or a
console SDK. "PSX Engine" is a working name.

**Status: Milestone 1 (foundation) — early prototype, not production-ready.**
The engine opens a window and renders a lit, textured test cube with a fly
camera. Retro rendering effects, the editor, scenes and gameplay are not
implemented yet. See [docs/STATUS.md](docs/STATUS.md) and
[docs/ROADMAP.md](docs/ROADMAP.md).

## Requirements

- Rust via [rustup](https://rustup.rs). The toolchain version is pinned in
  `rust-toolchain.toml` and installed automatically on first build.
- A GPU and driver supporting Vulkan, DirectX 12, Metal or OpenGL ES 3
  (through `wgpu`). Only WebGPU default limits are required.
- Windows 11 x86-64 is the primary target. Linux builds and runs (tested on
  X11 with Mesa lavapipe); macOS is untested.
- Linux only: X11/Wayland client libraries, including `libxkbcommon-x11`.

## Build and run

```sh
cargo run -p psx_sandbox --release
```

Controls: click in the window to capture the mouse, move the mouse to look,
`W` `A` `S` `D` to move, `E`/`Q` up/down, `Shift` to move faster, `Esc` to
release the mouse. Close the window to quit.

`psx_sandbox --frames N` exits after presenting N frames (for smoke tests).
Set `RUST_LOG=debug` for more logging, and `WGPU_BACKEND=vulkan|dx12|metal|gl`
to force a graphics backend.

## Tests

```sh
cargo test --workspace
```

The offscreen render test needs a graphics adapter and prints `SKIPPED`
without one. Set `PSX_REQUIRE_GPU=1` to make a missing adapter a failure.

## Layout

| Path | Purpose |
| --- | --- |
| `crates/psx_core` | Fixed-step timing; later stable IDs and config. No GPU/window deps. |
| `crates/psx_render` | wgpu device, surface presenter, camera, mesh/texture upload, WGSL shaders. |
| `crates/psx_runtime` | winit event loop, input sampling, fly camera, frame loop, logging. |
| `apps/psx_sandbox` | Milestone 1 test application. |

## License

MIT, see [LICENSE](LICENSE).
