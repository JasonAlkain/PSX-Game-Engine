# Roadmap

Milestones follow the product brief and are completed in order. Each must leave
the project runnable.

| # | Milestone | State |
| --- | --- | --- |
| 1 | Foundation: toolchain, workspace, logging, window lifecycle, camera, textured test mesh, fmt + CI | Implemented; Windows runtime check pending (see STATUS) |
| 2 | PS1 visual slice: internal-resolution target, integer upscale/letterbox, materials, vertex snapping, affine UVs, colour reduction, dithering, fog, comparison scene, saveable style settings | Implemented; Windows runtime check pending (see STATUS) |
| 3 | Authoring loop: versioned scenes, persistent IDs, static glTF import, editor panels (egui), transform editing, undo/redo, save/load, CLI validate/batch | Not started |
| 4 | Playable PS1 MVP: game template, input actions, physics, audio, interactions, menus, child-process play mode, host export, exploration demo | Not started |
| 5 | PS2 workflow: PS2 preset, filtered/mipmapped textures, skeletal animation, local lights, shadow option | Not started |
| 6 | Hardening and platforms: performance measurement, regression coverage, Linux/macOS validation | Not started |

## Next increment (start of Milestone 3)

1. `psx_core`: persistent UUIDs and a versioned scene file (entities with
   transform, mesh and material references), with round-trip and
   future-version tests. Add `uuid` then.
2. `psx_scene` crate on `hecs`: local/world transforms, hierarchy with cycle
   rejection, defined reparenting behaviour.
3. Static glTF import (`gltf`, `image`) into `MeshData`/`TextureData` with
   import fixtures (valid, missing buffer, malformed), then replace the
   hard-coded comparison scene with a scene file.
4. egui 0.37 editor shell; move the style keys into a style panel that shows
   the preset and whether it is customised.
