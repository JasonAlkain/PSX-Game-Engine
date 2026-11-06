# Known limitations

Copperframe is an early prototype. This is what it does not do yet, or does
only partly. The authoritative, dated list is in
[STATUS.md](https://github.com/JasonAlkain/PSX-Game-Engine/blob/main/docs/STATUS.md#known-limitations),
together with what has and has not been verified on each platform.

## Platforms

- Windows 11 is the primary target. Automated Windows checks ran on
  2026-11-05, but hands-on Windows runtime checks remain outstanding.
  Linux has been exercised with a software GPU. The manual Windows checklist is in
  [PLATFORMS.md](https://github.com/JasonAlkain/PSX-Game-Engine/blob/main/docs/PLATFORMS.md).
- macOS is not built or tested.
- The OpenGL ES fallback is untested.
- Performance has only been measured on a software rasteriser.

## Rendering

- Transparent objects sort per object, not per triangle.
- At most 8 point lights and 8 blob shadows per view; lights do not cast
  shadows; blob shadows are discs without real occlusion.
- A project can define a shared look; scenes can use that look or override
  it. Fog, background and lighting remain scene-specific. Per-material
  style overrides are limited (including `allow_affine`).
- No sprites or billboards yet.
- Tiling on primitive meshes is supported through `tile_size` (except
  spheres). Imported models use their authored UVs.

## Animation

- One skin per model, 64 joints, no morph targets.
- CUBICSPLINE keys are approximated as linear.
- Two-clip crossfades only (no layered or additive blending); poses update
  at 60 Hz without interpolation.
- Colliders of skinned models use the rest pose.

## Editor

- Move, rotate and scale gizmos are available, with grid, vertex and model
  snapping. Multi-selection and cross-scene copy/paste are not available.
- The hierarchy supports search, collapsing, reorder and drag-and-drop
  reparenting. A rigidbody must remain at the scene root.
- The editor watches assets and scripts for changes and reimports/reloads
  them. Moving an asset outside the editor still requires keeping its
  `.meta.json` sidecar with it.
- Picking tests triangles on the CPU; fine for rooms, not for huge scenes.
- High-DPI layout has only been run at scale 1.0.

## Gameplay

- No gamepad input.
- Lua scripts use an instruction budget and sandbox; C# scripts are trusted
  .NET code, require the .NET 10 runtime for players, and cannot be
  hot-reloaded into an already running game.
- Saves keep the scene and game state, not the player's position.
- Collider sizes scale with the entity.
- glTF: no morph targets or PBR textures; glTF samplers are ignored.
- C# scripts are not sandboxed and an endless loop in one hangs the game
  (Lua scripts are stopped). Players of a game with C# scripts need the
  .NET 10 runtime. A running game keeps the C# scripts it started with.

## Projects and export

- A generated game crate depends on the engine checkout by path.
- Exports are about 30 MB because binaries keep line tables for crash
  backtraces.
