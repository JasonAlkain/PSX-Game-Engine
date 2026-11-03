# Roadmap

Milestones follow the product brief and are completed in order. Each must leave
the project runnable.

| # | Milestone | State |
| --- | --- | --- |
| 1 | Foundation: toolchain, workspace, logging, window lifecycle, camera, textured test mesh, fmt + CI | Implemented; Windows runtime check pending (see STATUS) |
| 2 | PS1 visual slice: internal-resolution target, integer upscale/letterbox, materials, vertex snapping, affine UVs, colour reduction, dithering, fog, comparison scene, saveable style settings | Implemented; Windows runtime check pending (see STATUS) |
| 3 | Authoring loop: versioned scenes, persistent IDs, static glTF import, editor panels (egui), transform editing, undo/redo, save/load, CLI validate/batch | Implemented; Windows runtime check pending (see STATUS) |
| 4 | Playable PS1 MVP: game template, input actions, physics, audio, interactions, menus, child-process play mode, host export, exploration demo | Implemented as a PS1-style MVP; Windows runtime check pending (see STATUS) |
| 5 | PS2 workflow: PS2 preset, filtered/mipmapped textures, skeletal animation, local lights, shadow option | Implemented; Windows runtime check pending (see STATUS) |
| 6 | Hardening and platforms: performance measurement, regression coverage, Linux/macOS validation | Not started |

## Next increment (Milestone 6: hardening and platforms)

1. Performance measurement: frame-time and tick-time capture in the player
   and sandbox (`--bench`), a headless render benchmark over the example
   room, and recorded numbers per backend.
2. Lifecycle fixes found while measuring (resource churn on scene change,
   surface loss, long sessions).
3. Regression coverage: golden-image checks with tolerances for the PS1 and
   PS2 renders of the example room, and format migration fixtures for every
   version.
4. Migration documentation for every file format version.
5. Platform validation evidence for Linux (this container), with Windows and
   macOS manual checklists.
