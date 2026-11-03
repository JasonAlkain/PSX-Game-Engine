# Troubleshooting

If your problem is not here, check the log (see
[Crash reports and logs](crash-reports.md)) and
[open an issue](https://github.com/JasonAlkain/PSX-Game-Engine/issues/new?template=bug_report.yml).

## Building

**Linux: `The system library alsa required by crate alsa-sys was not found`.**
Install the ALSA headers: `sudo apt install libasound2-dev` (Fedora:
`alsa-lib-devel`).

**Linux: `Library libxkbcommon-x11.so could not be loaded`** when a window
opens. Install `libxkbcommon-x11-0` (Fedora: `libxkbcommon-x11`).

**Windows: `link.exe not found`.** Install the Visual Studio C++ build
tools (rustup offers to when it installs Rust).

**`error: toolchain '1.97…' is not installed`.** Run `rustup toolchain
install` in the repository; the version comes from `rust-toolchain.toml`.

**A project's game no longer builds after moving the engine.** Game crates
made by `copperframe new` depend on the engine checkout by absolute path.
Edit the `path = …` entries in `game/Cargo.toml` to the new location.

**Upgrading a project from "PSX Engine".** Change `psx_game` to
`copperframe_game` in `game/Cargo.toml` and the sources. Data files need no
change. See [MIGRATION.md](https://github.com/JasonAlkain/PSX-Game-Engine/blob/main/docs/MIGRATION.md#rename-to-copperframe).

## Graphics

**No window, or "no suitable graphics adapter".** Update the GPU driver.
Try forcing a backend with `WGPU_BACKEND=vulkan` or `WGPU_BACKEND=dx12`.
The OpenGL ES fallback (`gl`) is untested and may lack features the
renderer needs.

**GPU tests print `SKIPPED`.** No adapter was found. On Linux without a GPU
install `mesa-vulkan-drivers`. Set `COPPERFRAME_REQUIRE_GPU=1` to make a
skip a failure.

**Everything is slow.** Use `--release`; debug builds are much slower. On a
software renderer the game may run below 60 frames per second; the
simulation then drops ticks rather than falling behind, so movement takes
longer in real time. Measure with `copperframe bench`.

**Textures swim or warp on the floor.** That is affine texture mapping, a
PS1 trait. Subdivide large polygons, set `allow_affine: false` on that
material, or turn affine UVs off in the style.

**Geometry jitters.** That is vertex snapping. Lower `vertex_snap.grid` or
`strength`, or turn it off.

**The image is small with black bars.** Integer scaling picks the largest
whole-number scale that fits. Switch `presentation.scaling` to `fit`, or
make the window a multiple of the internal resolution.

## Scenes and the editor

**A magenta and black checker cube.** The entity's asset is missing or failed
to import. Check the Assets tab and the Console; run `copperframe validate`.

**`copperframe apply` exits with code 3.** Either the editor has the project
open (close it, or save and use the editor instead), or the batch's
`expected_revision` no longer matches the scene (inspect it again).

**"Scene changed on disk" when saving.** Another tool changed the scene
since the editor loaded it. Choose **Overwrite with my version** or reload
and lose your unsaved edits.

**"The project is open in a running editor".** The lock is released
by the OS when the editor exits, so this means an editor is still running.
Close it, or start with `--steal-lock` if it is hung.

**The behaviour list is empty or missing a new behaviour.** Use **Game >
Reload behaviour kinds** (or press Play), which rebuilds the game and reads
its schema. Build errors appear in the Console.

**Changed an asset file but the editor shows the old one.** There is no
file watching yet; click **Reimport** in the Assets tab.

## Gameplay

**The player falls through the floor.** The floor needs a collider (a box,
or `mesh` for static geometry). Primitives do not get one automatically.

**Nothing happens when I press E on an object.** The object (or a child)
needs a solid, non-sensor collider, a behaviour whose
`interaction_prompt` returns `Some`, and must be within the player's
`reach` (2 m by default).

**A sensor never fires.** Sensors detect other colliders: the player's
capsule, or bodies. Check `sensor` is on and the collision layers and masks
overlap.

**A point light or blob shadow does not show.** Check the style has
`shading.local_lights` or `shadows.enabled` on. Only the 8 nearest lights
and 8 nearest shadows are drawn. Shadows only fall on lit, upward-facing
surfaces.

**The animator has no clips.** The model needs a skin and animation clips;
`copperframe import` lists what it found.

**No sound.** Check the volume sliders and that the source has `autoplay`
or is started from code. Without an audio device the game logs a warning
and runs silently.

**A headless test fails but the game plays fine.** Read `failures` and
`events` in the report; scripted look and movement are exact, so a level
change can move the player off the path. Adjust the script's ticks or look
values.

**Continue starts at the beginning of the level.** Saves keep the scene and
the game state map, not the player's position. Store progress in
`ctx.state_mut()` and restore it in `start`.
