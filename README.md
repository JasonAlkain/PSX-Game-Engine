# PSX Engine

A Rust game engine for making original 3D games with PlayStation 1 and
PlayStation 2-inspired visuals on modern PCs. It is not an emulator or a
console SDK. "PSX Engine" is a working name.

**Status: Milestone 2 (PS1 visual slice) — early prototype, not production-ready.**
The sandbox renders a comparison scene through the PS1-style renderer:
low internal resolution with integer upscaling, vertex snapping, affine
texture mapping, colour reduction, ordered dithering, fog, and opaque /
cutout / blended materials. Every effect can be toggled live and the style
saved to a file. The editor, scene files and gameplay are not implemented
yet. See [docs/STATUS.md](docs/STATUS.md) and
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

The offscreen image tests (one per effect) and the comparison-scene camera
sweep need a graphics adapter and print `SKIPPED` without one. Set
`PSX_REQUIRE_GPU=1` to make a missing adapter a failure. Debug images land
in `target/tmp/`.

## Layout

| Path | Purpose |
| --- | --- |
| `crates/psx_core` | Fixed-step timing, retro style settings and their JSON file format. No GPU/window deps. |
| `crates/psx_render` | wgpu device, surface presenter, retro renderer (scene + composite passes), materials, fit policy, camera, mesh/texture data, WGSL shaders. |
| `crates/psx_runtime` | winit event loop, input sampling, fly camera, comparison scene, style keys, frame loop, logging. |
| `apps/psx_sandbox` | Sandbox application showing the comparison scene. |

## License

MIT, see [LICENSE](LICENSE).
