# Roadmap

Milestones follow the product brief and are completed in order. Each must leave
the project runnable.

| # | Milestone | State |
| --- | --- | --- |
| 1 | Foundation: toolchain, workspace, logging, window lifecycle, camera, textured test mesh, fmt + CI | Implemented; Windows runtime check pending (see STATUS) |
| 2 | PS1 visual slice: internal-resolution target, integer upscale/letterbox, materials, vertex snapping, affine UVs, colour reduction, dithering, fog, comparison scene, saveable style settings | Not started |
| 3 | Authoring loop: versioned scenes, persistent IDs, static glTF import, editor panels (egui), transform editing, undo/redo, save/load, CLI validate/batch | Not started |
| 4 | Playable PS1 MVP: game template, input actions, physics, audio, interactions, menus, child-process play mode, host export, exploration demo | Not started |
| 5 | PS2 workflow: PS2 preset, filtered/mipmapped textures, skeletal animation, local lights, shadow option | Not started |
| 6 | Hardening and platforms: performance measurement, regression coverage, Linux/macOS validation | Not started |

## Next increment (start of Milestone 2)

1. Render the scene into an offscreen colour + depth target at a configurable
   internal resolution (default 320×240).
2. Composite pass: aspect-correct letterbox with integer scaling where the
   window allows, documented fallback when the window is smaller than the
   internal image.
3. A `RetroStyle` settings struct (independent toggles) feeding a uniform
   block, starting with vertex snapping and affine UVs, each covered by an
   offscreen image comparison test.
