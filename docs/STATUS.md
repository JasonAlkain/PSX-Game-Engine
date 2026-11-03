# Status

Last updated: 2026-11-03 (Milestone 2).

## Working

Milestone 1 behaviour (window lifecycle, recoverable surface handling, fixed
60 Hz simulation with interpolation, input edges, fly camera, CI) is
unchanged. Milestone 2 adds the PS1 visual slice:

- **Internal-resolution rendering:** the world renders into a linear
  `Rgba16Float` + `Depth32Float` target (default 320×240, any 16–4096 size),
  then a composite pass upscales it into the window. With low resolution off
  the world renders at the native size of the on-screen rectangle.
- **Upscale and letterbox:** integer scaling (largest whole factor, centred,
  black bars) or aspect-correct fit. Windows smaller than the internal image
  fall back to fit. Camera aspect follows the internal image, never the window.
- **Vertex snapping:** clip X/Y quantised to the internal pixel grid with
  adjustable grid size (0.25–64 px) and strength; Z/W preserved; vertices at
  or behind the camera are left to the clipper.
- **Affine UVs:** `@interpolate(linear)` varying selected per frame, with a
  per-material opt-out.
- **Colour reduction and dithering:** 1–8 bits per channel in sRGB-encoded
  space, 4×4 Bayer dither anchored to internal pixels with adjustable
  strength. Independent of each other.
- **Fog and atmosphere:** linear distance fog (exact per-fragment distance),
  background colour, camera near/far.
- **Materials:** opaque, alpha cutout, alpha blended (sorted back-to-front per
  object), lit or unlit, smooth or flat shading, double-sided, tint; nearest
  sampled textures without mipmaps; vertex colours preserved.
- **Style settings:** `RetroStyle` with PS1 and Clean presets, `Custom`
  detection, validation, versioned JSON file with atomic save and `.bak`
  copy, future-version rejection.
- **Comparison scene** in `psx_sandbox`: room and fogged corridor with
  pillars, tiled floor, brick walls, gradient panel, smooth and flat spheres,
  cutout fence, glass pane and the spinning crate. Keys 1–7 toggle each
  effect, `[ ]` and `- =` adjust bits and snap grid, `P` switches preset, `C`
  shows a split view (left current style, right effects off), F5/F9
  save/reload. The window title shows the active style.

## Verification record

Environment: Ubuntu 24.04 container, Rust 1.97.0, Vulkan backend on Mesa
25.2.8 llvmpipe (software, "lavapipe"), Xvfb X11 display, no window manager.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `PSX_REQUIRE_GPU=1 cargo test --workspace --locked` | pass: 67 unit tests, 10 offscreen image tests, 2 scene GPU tests |
| `cargo build --workspace --release --locked` | pass |
| Image test: vertex snapping (grid 4 puts every edge on a multiple of 4; strength 0 equals off) | pass |
| Image test: floor through and behind the camera with snapping + affine, 7 poses (only valid colours, floor reaches the bottom edge) | pass |
| Image test: affine vs perspective boundary row matches the analytic prediction (±1.5 px); material opt-out stays perspective | pass |
| Image test: 5-bit levels only; no dither = identical rows; dither = 4-row period | pass |
| Image test: sRGB vs UNORM output agree within 1 (no double gamma) | pass |
| Image test: integer 2× upscale equals the internal image block for block, bars black; fit fallback for a small window | pass |
| Image test: cutout holes show what is behind (either draw order); blended pane drawn after opaque regardless of order | pass |
| Image test: fog before start / partial / fully fogged / off | pass |
| Image test: flat shading gives ≤ 1 colour per triangle | pass |
| Comparison scene: 192 frames over 8 positions (inside walls, crate, below floor, grazing near plane, outside) × 4 yaws × 3 pitches (±89°) × 2 snap grids, no validation errors | pass |
| Split view: left half equals PS1 render, right half equals clean render | pass |
| `psx_sandbox --frames 120` and `--split` under Xvfb | pass, no wgpu errors |
| Resize 960×720 → 500×300 (1× with bars) → 200×150 (fit downscale) → 1280×720 (3× pillarbox) → 961×721, unmap/remap | pass, images inspected |

Debug images from the tests are written to `target/tmp/*.ppm`.

## Not verified

- **Windows 11 runtime** (primary target): not run for Milestone 1 or 2. CI
  builds and tests on a Windows runner. Manual check:
  `cargo run -p psx_sandbox --release`, then press `C` for the split view,
  1–7 to toggle effects, F5 to save (`psx_style.json` appears in the working
  directory), restart to confirm it reloads, plus the Milestone 1 window
  checks (resize, minimize, monitors with different DPI, mouse capture).
- **Keyboard style controls in a real window:** synthetic key events are not
  delivered without a window manager here. The key mapping is unit tested
  (`style_controls`), and every effect is exercised through the renderer API.
- Hardware GPUs, DX12 and Metal backends: only lavapipe was available.
- Performance: not measured (software rendering FPS is not meaningful).

## Known limitations

- The OpenGL ES fallback backend is untested: it needs float render targets
  (`Rgba16Float`) and `noperspective` interpolation, which some GLES 3.0
  drivers lack. Vulkan, DX12 and Metal support both.
- Transparent objects are sorted per object by origin distance, not per
  triangle.
- Lighting is per vertex only (ambient + one directional light); no local
  lights or shadows yet.
- Style changes apply to the whole view; per-material and per-scene style
  overrides need the scene format (Milestone 3).
- The comparison scene is hard-coded Rust data; scenes become files in
  Milestone 3. Sprites/billboards and import-time texture size limits are not
  implemented yet (they need asset import).
- Game HUD is not implemented; when it is, it will render at native
  resolution after the composite pass.
- Key bindings are fixed; named input actions arrive with Milestone 4.
- If `wgpu-hal` fails to create a replacement surface on Vulkan/X11 it panics
  internally (`expect` in wgpu-hal 30.0.1); this can only be reached when a
  live window's surface is lost twice in a row.
