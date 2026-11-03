# Known limitations

Copperframe is an early prototype. This is what it does not do yet, or does
only partly. The authoritative, dated list is in
[STATUS.md](https://github.com/JasonAlkain/PSX-Game-Engine/blob/main/docs/STATUS.md#known-limitations),
together with what has and has not been verified on each platform.

## Platforms

- Windows 11 is the primary target, but the engine has so far been run
  only on Linux with a software GPU. CI builds and tests on Windows. The
  manual Windows checklist is in
  [PLATFORMS.md](https://github.com/JasonAlkain/PSX-Game-Engine/blob/main/docs/PLATFORMS.md).
- macOS is not built or tested.
- The OpenGL ES fallback is untested.
- Performance has only been measured on a software rasteriser.

## Rendering

- Transparent objects sort per object, not per triangle.
- At most 8 point lights and 8 blob shadows per view; lights do not cast
  shadows; blob shadows are discs without real occlusion.
- Style is per scene (and per player setting); there are no per-material
  style overrides beyond `allow_affine`.
- No sprites or billboards yet.
- The demo floor's texture is stretched over the whole floor (no UV tiling
  control yet).

## Animation

- One skin per model, 64 joints, no morph targets.
- CUBICSPLINE keys are approximated as linear.
- Two-clip crossfades only (no layered or additive blending); poses update
  at 60 Hz without interpolation.
- Colliders of skinned models use the rest pose.

## Editor

- The gizmo only translates; rotation and scale are edited as numbers.
- No multi-selection, copy and paste between scenes, or drag-and-drop
  reparenting.
- No file watching: reimport assets by hand. Moving an asset outside the
  editor requires moving its `.meta.json` with it.
- Picking tests triangles on the CPU; fine for rooms, not for huge scenes.
- High-DPI layout has only been run at scale 1.0.

## Gameplay

- No gamepad input.
- Saves keep the scene and game state, not the player's position.
- Collider sizes scale with the entity.
- glTF: no morph targets or PBR textures; glTF samplers are ignored.

## Projects and export

- A generated game crate depends on the engine checkout by path.
- Exports are about 30 MB because binaries keep line tables for crash
  backtraces.
