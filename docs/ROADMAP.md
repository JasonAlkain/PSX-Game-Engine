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
| 6 | Hardening and platforms: performance measurement, lifecycle fixes, regression coverage, migration docs, platform validation evidence | Implemented; Linux validated on a software adapter, Windows/macOS checklists pending (see PLATFORMS) |

## After Milestone 6

All six milestones of the brief are implemented. What remains before the
engine can be called validated on its primary target:

1. Run the Windows 11 checklist in `docs/PLATFORMS.md` and record the
   results (and `copperframe bench` numbers on a hardware GPU) in STATUS.
2. Fix whatever that run finds; the macOS checklist after that.

Candidates for later work, not committed: gamepad input, a cooked asset
cache, sprites/billboards, per-triangle transparency sorting, stripped
release exports, interpolated animation palettes.
