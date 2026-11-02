# Status

Last updated: 2026-11-02 (Milestone 1).

## Working

- Cargo workspace (edition 2024, toolchain pinned to 1.97.0) with centralised
  dependency versions and committed `Cargo.lock`.
- Logging through `tracing` (`RUST_LOG` overrides; wgpu `log` output bridged).
- Window and event lifecycle: create on `resumed`, resize, minimize (render
  paused, event loop sleeps), occlusion, DPI change (logged; follows with a
  resize), focus loss, close, external window destruction.
- Recoverable surface handling (suboptimal/outdated/timeout/occluded/lost).
- Fixed 60 Hz simulation with bounded catch-up and render interpolation.
- Input sampling with reliable press edges across ticks.
- Fly camera (mouse capture with locked→confined fallback).
- Forward renderer: textured, vertex-coloured, vertex-lit cube with depth
  testing, back-face culling and nearest-neighbour sampling.
- CI workflow: Linux (fmt, clippy, tests with GPU required on lavapipe) and
  Windows (clippy, tests, release build).

## Verification record

Environment: Ubuntu 24.04 container, Rust 1.97.0, Vulkan backend on Mesa
25.2.8 llvmpipe (software, "lavapipe"), Xvfb X11 display, no window manager.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| `PSX_REQUIRE_GPU=1 cargo test --workspace` | pass (32 unit tests + offscreen render test) |
| `cargo build --workspace --release` | pass |
| Offscreen render (64×64): cube visible, checker texture contrast, corners = clear colour, no validation errors | pass on lavapipe |
| `psx_sandbox --frames 120` under Xvfb | pass, exit 0, no wgpu errors |
| Interactive window resize (960×720 → 500×300 → 1200×500 and others) | pass, image aspect-correct |
| Unmap/remap window | pass |
| External window destruction | clean exit (found and fixed a crash path, see below) |

Found during testing: when the native window was destroyed externally, wgpu
reported the surface lost and recreating it panicked inside `wgpu-hal`. The
app now exits on `WindowEvent::Destroyed`, skips redraws while exiting, tries
a swapchain reconfigure before recreating a lost surface, and drops the old
surface before creating its replacement.

## Not verified

- **Windows 11 runtime** (primary target): not run. CI builds and tests on a
  Windows runner, but the windowed app on a real GPU (DX12/Vulkan), minimize
  via the taskbar, DPI changes between monitors, and mouse capture need a
  manual check:
  `cargo run -p psx_sandbox --release`, then resize, minimize/restore, drag
  between monitors with different scaling, click to capture, Esc to release,
  Alt-Tab away while capturing.
- Real keyboard/mouse input in a window (synthetic X events were not delivered
  without a window manager). Input and camera logic are unit tested.
- macOS and Wayland: not run.
- Hardware GPUs: only the software rasteriser was available.
- Performance: no measurements yet (software-rendered FPS is not meaningful).

## Known limitations

- Normals use the model matrix directly, valid only for rotation and uniform
  scale.
- Only one hard-coded mesh and texture; no asset loading yet.
- Key bindings are fixed data in `FlyBindings`; named actions and remapping
  arrive with Milestone 4.
- If `wgpu-hal` fails to create a replacement surface on Vulkan/X11 it panics
  internally (`expect` in wgpu-hal 30.0.1); this can only be reached when a
  live window's surface is lost twice in a row.
