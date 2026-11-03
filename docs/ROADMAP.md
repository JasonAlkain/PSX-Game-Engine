# Roadmap

Milestones follow the product brief and are completed in order. Each must leave
the project runnable.

| # | Milestone | State |
| --- | --- | --- |
| 1 | Foundation: toolchain, workspace, logging, window lifecycle, camera, textured test mesh, fmt + CI | Implemented; Windows runtime check pending (see STATUS) |
| 2 | PS1 visual slice: internal-resolution target, integer upscale/letterbox, materials, vertex snapping, affine UVs, colour reduction, dithering, fog, comparison scene, saveable style settings | Implemented; Windows runtime check pending (see STATUS) |
| 3 | Authoring loop: versioned scenes, persistent IDs, static glTF import, editor panels (egui), transform editing, undo/redo, save/load, CLI validate/batch | Implemented; Windows runtime check pending (see STATUS) |
| 4 | Playable PS1 MVP: game template, input actions, physics, audio, interactions, menus, child-process play mode, host export, exploration demo | Implemented as a PS1-style MVP; Windows runtime check pending (see STATUS) |
| 5 | PS2 workflow: PS2 preset, filtered/mipmapped textures, skeletal animation, local lights, shadow option | Not started |
| 6 | Hardening and platforms: performance measurement, regression coverage, Linux/macOS validation | Not started |

## Next increment (Milestone 5: PS2 workflow)

1. PS2 style preset alongside PS1 and Clean: higher internal resolution,
   no vertex snapping or affine UVs by default, 8-bit colour, filtered
   textures. Switching presets must not require rebuilding content.
2. Texture filtering per material (nearest or linear) and generated
   mipmaps; the PS1 preset keeps nearest without mipmaps.
3. glTF skins and animation clips: import joints, inverse bind matrices and
   sampled clips; GPU skinning in the retro pipeline (snapping applied after
   skinning); `animator` scene component with clip, speed, loop and a
   basic two-clip crossfade; play from behaviours.
4. Bounded local lights: point lights with a range, up to a fixed number
   per object, per-vertex in PS1 style and per-pixel in PS2 style.
5. A practical shadow option: blob shadows (projected decals) for
   characters, switchable per entity.
6. Extended import fixtures (skinned, animated, multi-clip, bad skins) and
   an animated character in the demo; image tests that UVs and geometry stay
   stable under the PS2 preset.
