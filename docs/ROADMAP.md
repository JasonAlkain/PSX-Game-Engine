# Roadmap

Milestones follow the product brief and are completed in order. Each must leave
the project runnable.

| # | Milestone | State |
| --- | --- | --- |
| 1 | Foundation: toolchain, workspace, logging, window lifecycle, camera, textured test mesh, fmt + CI | Implemented; Windows runtime check pending (see STATUS) |
| 2 | PS1 visual slice: internal-resolution target, integer upscale/letterbox, materials, vertex snapping, affine UVs, colour reduction, dithering, fog, comparison scene, saveable style settings | Implemented; Windows runtime check pending (see STATUS) |
| 3 | Authoring loop: versioned scenes, persistent IDs, static glTF import, editor panels (egui), transform editing, undo/redo, save/load, CLI validate/batch | Implemented; Windows runtime check pending (see STATUS) |
| 4 | Playable PS1 MVP: game template, input actions, physics, audio, interactions, menus, child-process play mode, host export, exploration demo | Not started |
| 5 | PS2 workflow: PS2 preset, filtered/mipmapped textures, skeletal animation, local lights, shadow option | Not started |
| 6 | Hardening and platforms: performance measurement, regression coverage, Linux/macOS validation | Not started |

## Next increment (start of Milestone 4)

1. Named input actions (keyboard, mouse, gamepad via `gilrs`) with a
   versioned bindings file; replace fixed keys in the runtime.
2. Runtime world: build a `hecs` world from a `Scene` document, with
   gameplay components (player start, collider, trigger, interactable,
   sound source) added to the scene format as version 2 with a migration
   from version 1.
3. `rapier3d` character controller and static colliders generated from
   meshes; `kira` audio with spatial sources.
4. Game template app that loads a project's startup scene, plus play mode in
   the editor as a child process that never touches the editor's unsaved
   state; host export (`psx build`) of a runnable folder.
5. Exploration demo built from the example room: walk, open a door, pick up
   an item, menu with resume/settings/quit.
