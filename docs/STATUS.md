# Status

Last updated: 2026-11-03 (crash and bug reporting; renamed to Copperframe Engine; Milestone 6: hardening and platform evidence; all six milestones implemented, Windows 11 runtime checks outstanding).

## Editor responsiveness and lock recovery (2026-11-03)

The editor redraws on demand instead of every frame (idle new project: about
2.3 CPU cores to 0 on lavapipe; example room with its animation preview: 2.3
to 0.3). A lock file left by a crashed editor no longer blocks the project:
the editor holds an OS file lock, so a leftover file is detected and
replaced. CI cancels superseded runs and has timeouts; Dependabot proposes
weekly updates. Verified on Linux (Xvfb + lavapipe); unverified on Windows 11.

## Crash and bug reporting (2026-11-03)

Every engine binary saves a crash report and a log file in the user's data
folder; the editor offers to email unseen crash reports (and Help → "Report
a bug…") as a draft in the user's own mail program, to
`zacharyaars+copperframeengine@gmail.com`. Games keep reports locally and
never offer that address. Release builds keep line tables. Verified on Linux
(Xvfb + lavapipe): a real startup panic produced a report, the editor showed
it on the next start. Opening the mail draft is unverified on Windows 11.
See [IMPROVEMENTS.md](IMPROVEMENTS.md) and ARCHITECTURE "Crash and bug
reports".

## Rename to Copperframe (2026-11-03)

Crates, binaries, the CLI (`copperframe`), window titles, test environment
variables and the OS settings/save folder now use the Copperframe name; data
formats (`psx_*` tags, `psx_style.json`, `.psx/`, `psx.*` behaviours) are
unchanged. See [MIGRATION.md](MIGRATION.md#rename-to-copperframe). The
editor, player and sandbox have the Copperframe window icon, and the editor
and player title screen open with a skippable splash. Verified on Linux
(Xvfb + lavapipe); the icon and splash are unverified on Windows 11.

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
  object), lit or unlit, smooth or flat shading, double-sided, tint; vertex
  colours preserved. Texture filtering and mipmaps are style settings
  (Milestone 5).
- **Style settings:** `RetroStyle` with PS1 and Clean presets, `Custom`
  detection, validation, versioned JSON file with atomic save and `.bak`
  copy, future-version rejection.
- **Comparison scene** in `copperframe_sandbox`: room and fogged corridor with
  pillars, tiled floor, brick walls, gradient panel, smooth and flat spheres,
  cutout fence, glass pane and the spinning crate. Keys 1–7 toggle each
  effect, `[ ]` and `- =` adjust bits and snap grid, `P` switches preset, `C`
  shows a split view (left current style, right effects off), F5/F9
  save/reload. The window title shows the active style.

## Milestone 3: authoring loop

- **Scenes:** versioned JSON scene files (`copperframe_scene`) with persistent UUID
  entity IDs, parent hierarchy (cycles, missing parents, depth > 64
  rejected), local transforms, primitive or imported-model meshes with
  per-entity material overrides, and per-scene style, lighting and camera
  start. Unknown fields and future versions are rejected; saves are atomic
  with a `.bak` copy; revisions detect concurrent edits.
- **Commands and undo:** nine commands shared by editor and CLI; failed
  commands change nothing; undo/redo restores exact state; drags merge into
  one step; dirty tracking survives undo back to the saved state.
- **CLI (`copperframe`):** `new`, `schema`, `validate`, `import`, `inspect`,
  `apply` (all-or-nothing batches, `--dry-run`, `expected_revision`,
  refuses while the editor is open), text or `--json` output, documented
  exit codes.
- **Assets:** `.meta.json` sidecars hold stable asset IDs; static glTF
  (`.gltf`/`.glb`) and PNG/JPEG import with the support matrix in
  ARCHITECTURE; malformed, truncated, out-of-project and unsupported files
  fail with a message, never a panic. A failed reimport keeps the last good
  GPU data; missing assets draw a magenta checker.
- **Editor (`copperframe_editor`):** egui menu bar, hierarchy, inspector (name,
  parent, position/rotation/scale, mesh, material overrides), scene
  settings (style preset and fields, lighting, camera start), assets tab
  (register, add to scene, reimport), console, status bar with dirty and
  revision state. Viewport renders with the scene's retro style; click to
  pick, translate gizmo with optional snapping, right-drag fly camera.
  Unsaved-changes and save-conflict dialogs; editor lock.
- **Example project:** `examples/room` (floor, walls, pillars, crates,
  imported models and texture), built by `examples/room/build_room.batch.json`.

Completion criteria from the brief:

| Criterion | Evidence |
| --- | --- |
| A room assembled in the editor survives restart with the same entities and assets | `copperframe_editor::session` test `room_survives_restart_cli_edit_and_undo` (create, import, save, reopen, compare IDs); manual Xvfb run: gizmo move, File > Save, restart shows the move |
| It can be modified through the CLI | same test applies a batch with `copperframe_scene::apply_batch`; `apps/copperframe_cli/tests/cli.rs` runs `copperframe new/import/apply/validate/inspect` end to end, including lock refusal |
| It stays valid after undo/redo and failed imports | same test undoes/redoes every step and validates; `scene_resources` GPU test breaks a model file and checks the last import is kept and drawn; 10 glTF fixture failure tests |

## Milestone 4: playable PS1-style MVP

Milestone 4 delivered the playable PS1-style MVP; Milestone 5 (below) adds
the PS2 workflow on top of it.

- **Gameplay runtime (`copperframe_game`):** `Game`/`Behavior` API with a registry
  and typed behaviour schemas; `Simulation` builds a `hecs` world from a
  scene and runs fixed 60 Hz ticks in a documented order; state map,
  messages, spawn/despawn, finish. Built-in `psx.player` (first-person
  character), `psx.camera_zone` (fixed cameras) and `psx.spin`.
- **Scene format v2:** collider, audio source and behaviour components,
  with v1 migration. Editor inspectors for all three (behaviour fields come
  from the game's schema) and collider wireframes (Edit > Show colliders).
- **Physics:** rapier3d static/kinematic/dynamic bodies, sensors with
  enter/exit events, ray casts, capsule character controller that also
  stops against moving kinematic bodies.
- **Input:** named actions in `input.json`, user rebinding in settings,
  mouse sensitivity and invert-Y, cursor capture, focus loss pauses.
- **Audio:** kira music/effects buses with volumes, looping, fades,
  spatial emitters and one listener; a missing audio device is a warning,
  not an error.
- **Player:** title screen, HUD (crosshair, prompts, messages), pause,
  settings, Continue from a save, finished screen; settings and saves in the
  OS user folders.
- **Headless runs:** `--headless --script` and `copperframe test-scene` run the
  same simulation without window, GPU or audio and print a JSON report.
- **Editor play mode:** F5 / Play runs the game as a child process on a
  copy of the current scene; Stop kills it; console shows its logs.
- **Export:** `copperframe build` and Game > Export produce a standalone folder
  (executable, data, licence notices) and verify it by running it.
- **Templates:** `copperframe new` writes a game crate (`game/`) with an example
  behaviour; `--no-game` projects use the stock `copperframe_player`.
- **Demo:** `examples/room` is "The Lever Room": walk, inspect objects, a
  locked door, pull a lever to open it, walk a corridor under a fixed
  camera with a humming machine, reach the exit.

Completion criteria from the brief:

| Criterion | Evidence |
| --- | --- |
| Edit the demo | Editor under Xvfb: inspector shows collider, audio and behaviour components with schema fields; collider wireframes; `apply` batches `build_demo` and `fixed_camera` build the demo through the command layer |
| Play it | Editor F5 launched the game on the `.psx/play` copy, logs streamed into the console, Stop ended it. Windowed player under Xvfb: title, walking, mouse look, "It won't budge" interaction, pause, settings, rebinding saved, quit-to-title save and Continue, fixed camera showing the player body, finished screen |
| Stop without changing the authored scene | `examples/room/game/tests/walkthrough.rs` checks the scene file bytes are unchanged after a full play; the simulation never writes the `Scene` |
| Export it | `copperframe build --project examples/room --out "<dir>/Lever Room"`: 18 files, 29.3 MiB, notices for 464 crates |
| Complete it in the standalone package | The exported executable, run from a folder whose path has spaces, completed `scripts/walkthrough.script.json` (535 ticks, `Finished`, all expected events). A windowed run of the export started, reached the game, and received keyboard input; completing it by synthetic mouse input under Xvfb was not achieved (see Not verified) |

## Milestone 5: PS2 workflow

- **PS2 preset** next to PS1 and Clean (editor Scene settings, sandbox `P`):
  640×448 fit, no snapping, perspective UVs, full colour, filtered
  mipmapped textures, per-pixel lighting. Style file v2 (`textures`,
  `shading`, `shadows`); v1 files get PS1 values.
- **Filtered and mipmapped textures:** full mip chains are always uploaded;
  the style chooses nearest/linear and mipmaps on/off per frame, so
  switching never reimports or re-uploads.
- **Skeletal animation:** glTF skins (≤ 64 joints, parent-first reordering)
  and clips (STEP/LINEAR, CUBICSPLINE approximated with a warning) import as
  importer version 2; GPU skinning in the retro pipeline (snapping after
  skinning); `animator` component (clip, speed, loop, autoplay);
  `Ctx::play_animation` crossfades between clips, `stop_animation` fades to
  rest, `AnimationFinished` events; editor viewport previews autoplaying
  clips; `copperframe import` lists joints and clips; `copperframe validate` warns about
  unknown clips.
- **Bounded local lights:** `light` component (colour, intensity, range),
  up to 8 nearest the camera, per vertex (PS1) or per pixel (PS2).
- **Shadow option:** `shadow` component draws a blob shadow on lit,
  upward-facing surfaces below the entity; style switch and opacity.
- **Scene format v3** with the three components (v1 and v2 load unchanged),
  editor inspectors for each, batch ops `set_light`, `set_animator`,
  `set_shadow`.
- **Player:** Settings > Graphics style (As authored / PS1 / PS2) renders
  the same content in either style.
- **Demo:** *The Lever Room* gains the caretaker (a generated skinned
  character with `idle` and `wave` clips, `room.caretaker` behaviour:
  talk, crossfade to the wave, fade back to idle), a ceiling lamp, coloured
  lights on the exit and the machine, and blob shadows (built by
  `examples/room/visuals.batch.json`).

Completion criteria from the brief:

| Criterion | Evidence |
| --- | --- |
| The same project can use either style without rebuilding content | `scene_resources::example_room_renders_in_both_styles_from_the_same_content` renders `examples/room` with its PS1 style and the PS2 preset from one upload (captures `target/tmp/room_ps1.ppm`, `room_ps2.ppm` inspected: caretaker, crates, pillars, blob shadow under the caretaker); windowed `lever_room --play` under Xvfb with `graphics_style` `scene` and `ps2` (screenshots inspected); style switching is a per-frame sampler/uniform choice |
| An animated character works | `copperframe_game` animation unit tests (clip end reported once, crossfade midpoint, reversed and held playback, non-finite input); `simulation::animated_models_play_crossfade_and_report_finished_clips`; `walkthrough::the_caretaker_waves_then_returns_to_idle` (scripted walk up, talk, `wave`, `AnimationFinished`, back to `idle`); GPU test `skinned_models_draw_in_their_animated_pose`; renderer image test `skinning_moves_vertices`; editor under Xvfb shows the caretaker's animator, light and shadow inspectors with the model's clips |
| Geometry and UVs are stable under the PS2 preset | Image test `ps2_preset_keeps_geometry_and_uvs_where_perspective_puts_them`: over six sub-pixel camera moves the far edge stays within 1 px and the texture boundary within 1.5 px of the perspective projection, while the same content under PS1 is off by more than 4 px (affine warping) |

## Milestone 6: hardening and platforms

- **Performance measurement:** every game executable has `--bench`
  (offscreen, no window or audio; real simulation; authored, PS1 and PS2
  styles; mean/p50/p95/max tick and frame times; adapter and driver in the
  report) and `copperframe bench --project` builds the game in release and runs it.
  CI runs a short benchmark on Linux as a smoke test.
- **Lifecycle:** a scene change frees cached CPU models and decoded sounds
  the new scene does not use, and the player releases GPU models and
  textures of the previous scene (`SceneResources::release_unused`). A
  soak test restarts the scene 50 times and checks entity, body, emitter
  and cache counts stay constant.
- **Regression coverage:** reference images of the example room (two
  poses × PS1/PS2, 320×240, caretaker mid-idle) in
  `crates/copperframe_runtime/tests/golden/`, compared with a tolerance on the
  llvmpipe reference adapter (as on CI); committed fixtures for scene v1
  and v2, style v1 and Milestone 4 player settings.
- **Docs:** [MIGRATION.md](MIGRATION.md) (every format version, importer
  versions, game API changes per milestone) and
  [PLATFORMS.md](PLATFORMS.md) (what ran where, and the Windows/macOS/
  hardware-Linux checklists).

Performance on the reference software adapter (llvmpipe, LLVM 20.1.2, 256
bits, Vulkan, in a container; release build; the walkthrough script as
input; 300 measured frames per style; frame = tick + render + wait for the
GPU):

| Output | Style (internal) | Frame mean | Frame p95 | Frame max | Tick mean |
| --- | --- | --- | --- | --- | --- |
| 1280×720 | PS1 (320×240) | 8.3 ms | 10.4 ms | 14.4 ms | 0.14 ms |
| 1280×720 | PS2 (640×448) | 12.1 ms | 14.9 ms | 20.8 ms | 0.16 ms |
| 640×480 | PS1 (320×240) | 5.4 ms | 7.2 ms | 13.9 ms | 0.12 ms |
| 640×480 | PS2 (640×448) | 8.3 ms | 10.0 ms | 22.7 ms | 0.14 ms |

The scene has 27 draws and 786 triangles. The simulation tick is far below
its 16.7 ms budget; on this CPU rasteriser even the PS2 style at 720p stays
under 16.7 ms at the 95th percentile. Hardware GPU numbers have not been
measured.

## Verification record

Environment: Ubuntu 24.04 container, Rust 1.97.0, Vulkan backend on Mesa
25.2.8 llvmpipe (software, "lavapipe"), Xvfb X11 display, no window manager.

| Check | Result |
| --- | --- |
| `cargo fmt --all -- --check` | pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | pass |
| `COPPERFRAME_REQUIRE_GPU=1 cargo test --workspace --locked` | pass: 207 tests, 1 ignored (a documentation example) |
| M5 image tests: linear filtering, mip averaging (trilinear distant texels ≈ the texture mean), point lights bounded and lit per vertex or per pixel, blob shadows (on lit floors only), skinning, PS2 geometry/UV stability | pass |
| M5 GPU tests: skinned model drawn in its animated pose; example room in PS1 and PS2 from the same resources, no validation errors | pass, captures inspected |
| M5 editor under Xvfb (debug): opens `examples/room`, Scene settings show PS2 and the new style switches, selecting Caretaker shows Animator (clip list from the model), Point light and Blob shadow inspectors; `--frames` exit, no wgpu validation errors | pass, screenshots inspected |
| M5 player under Xvfb (debug): `lever_room --play --frames 900` with graphics style "as authored" and "PS2" | pass, screenshots inspected |
| M6 reference images (example room, 2 poses × PS1/PS2) on llvmpipe; soak test of 50 restarts; `--bench` and `copperframe bench` (release, numbers above); migration fixtures | pass |
| M5 `copperframe build` of `examples/room` (release): 20 files, 29.6 MiB, 464 crates in NOTICES; the exported executable completed the walkthrough script headless (535 ticks, finished, no warnings) | pass |
| `copperframe test-scene --project examples/room --script examples/room/scripts/walkthrough.script.json` | pass, 535 ticks, finished |
| `copperframe build` of `examples/room` (release) with the walkthrough verification | pass |
| `copperframe new` then `copperframe test-scene`, with and without a game crate | pass |
| M4 windowed player and editor play mode under Xvfb (see the table above) | pass, screenshots inspected |
| `cargo build --workspace --release --locked` | pass |
| `copperframe validate --project examples/room` | pass, 0 errors, 0 warnings |
| M3 image test: emissive adds after lighting and is fogged | pass |
| M3 GPU test: imported model colour, missing-asset fallback, failed reimport keeps last import, ray picking | pass |
| M3 editor under Xvfb (release): opens `examples/room`, renders with PS1 style, click picking selects, gizmo drag moves a parent with its child, File > Save writes revision 2 and `.bak`, Edit > Undo marks dirty, quit with unsaved changes shows the dialog, "Quit without saving" exits and removes the lock, `--frames 120` exits cleanly; no wgpu validation errors | pass, screenshots inspected |
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
| `copperframe_sandbox --frames 120` and `--split` under Xvfb | pass, no wgpu errors |
| Resize 960×720 → 500×300 (1× with bars) → 200×150 (fit downscale) → 1280×720 (3× pillarbox) → 961×721, unmap/remap | pass, images inspected |

Debug images from the tests are written to `target/tmp/*.ppm`.

## Not verified

- **Windows 11 runtime** (primary target): not run for Milestones 1 to 6;
  see the checklist in [PLATFORMS.md](PLATFORMS.md). CI
  builds and tests on a Windows runner. Manual check:
  `cargo run -p copperframe_sandbox --release`, then press `C` for the split view,
  1–7 to toggle effects, F5 to save (`psx_style.json` appears in the working
  directory), restart to confirm it reloads, plus the Milestone 1 window
  checks (resize, minimize, monitors with different DPI, mouse capture).
- **Keyboard style controls in a real window:** synthetic key events are not
  delivered without a window manager here. The key mapping is unit tested
  (`style_controls`), and every effect is exercised through the renderer API.
- **Editor keyboard shortcuts and fly camera** (Ctrl+S/Z/Y/D, Delete,
  right-drag + WASD): same reason; the menu items they trigger were
  exercised with the mouse. Windows manual check:
  `cargo run -p copperframe_editor --release -- examples/room`, move a crate with the
  gizmo, Ctrl+S, close, reopen, confirm the crate stayed; Ctrl+Z then close
  and confirm the unsaved-changes dialog; run
  `cargo run -p copperframe_cli --release -- inspect --project examples/room`.
- **High-DPI editor layout:** picking accounts for the scale factor in code
  and unit tests, but only scale 1.0 was run.
- Hardware GPUs, DX12 and Metal backends, macOS: only lavapipe on Linux
  was available.
- Performance on hardware GPUs: only the software adapter was measured
  (table above). Run `copperframe bench --project examples/room` on the target
  machine.
- **Audio output:** the container has no sound device. Audio playback paths
  are exercised headless (emitters, buses, fades are computed) and the
  missing-device path was seen in every windowed run; hearing the demo
  needs a real machine. Windows manual check:
  `cargo run -p lever_room --release`, confirm the machine hum gets louder
  near the machine and the volume sliders work.
- **Completing the demo windowed in the exported package:** under Xvfb the
  game received synthetic keys, but synthetic relative mouse motion arrives
  through the X server's pointer acceleration and cursor capture, so the
  scripted view turns were not reproducible and the lever could not be
  aimed at. The same executable completed the walkthrough headless.
  Windows manual check: `cargo run -p copperframe_cli --release -- build --project
  examples/room --out "%USERPROFILE%\Desktop\Lever Room"`, run
  `lever_room.exe` from that folder, finish the demo, quit, and confirm
  settings appear under `%APPDATA%\Copperframe Engine\the-lever-room\config`.
- **M5 in a window:** talking to the caretaker and switching the graphics
  style from the Settings menu were not driven by synthetic input here
  (focus and relative mouse issues as above); both are covered headless and
  by the settings file. Windows manual check: `cargo run -p lever_room
  --release`, walk to the caretaker on the right, press E, watch the wave
  blend back to idle; open Settings and switch Graphics style between PS1
  and PS2 while playing.
- Gamepads: not implemented.

## Known limitations

- The OpenGL ES fallback backend is untested: it needs float render targets
  (`Rgba16Float`) and `noperspective` interpolation, which some GLES 3.0
  drivers lack. Vulkan, DX12 and Metal support both.
- Transparent objects are sorted per object by origin distance, not per
  triangle.
- Local lights: at most 8 per view (the nearest to the camera), no
  per-object light selection; lights do not cast shadows. Blob shadows are
  discs (no real occlusion), at most 8, and only fall on lit materials.
- Animation: one skin per model, 64 joints, rotation/translation/scale
  channels only (no morph targets), CUBICSPLINE approximated as linear,
  two-clip crossfades (no layered or additive blending). The palette updates
  per simulation tick (60 Hz) and is not interpolated between ticks.
  Colliders of skinned models use the rest pose.
- Style is per scene (and per view); there are no per-material style
  overrides beyond `allow_affine`.
- The sandbox comparison scene is still hard-coded Rust data; the editor and
  example project use scene files. Sprites/billboards are not implemented.
- Editor gizmos translate only; rotation and scale are edited numerically in
  the inspector. Multi-selection, copy/paste between scenes, and
  drag-and-drop reparenting in the hierarchy are not implemented.
- Asset hot-reload is manual (Assets tab: Reimport / Rescan assets folder); file watching is
  not implemented. Moving an asset outside the editor requires moving its
  `.meta.json` with it.
- glTF: no morph targets or PBR textures (see the support matrix in
  ARCHITECTURE). Imported models share one texture per primitive; glTF
  samplers are ignored (filtering is a style setting).
- Picking tests triangles on the CPU every click; fine for small rooms, not
  for scenes with millions of triangles.
- Saves hold the scene and the game state map, not the player's position;
  Continue restarts the saved scene at its start with the saved state.
- Collider sizes are multiplied by the entity's scale, so scaling an entity
  in the editor scales its collider too.
- The demo floor's texture is stretched over the whole floor (no UV
  tiling control yet).
- Software rendering can run slower than 60 Hz; the simulation then drops
  ticks rather than spiralling, so walks take longer in wall-clock time.
- A generated game crate depends on the engine by path, so it builds only
  while the engine checkout it was created from exists.
- Exports are about 30 MB because binaries are not stripped.
- Editor Game > Export needs a game crate; projects without one export with
  `copperframe build`.
- Gamepad input is not implemented.
- If `wgpu-hal` fails to create a replacement surface on Vulkan/X11 it panics
  internally (`expect` in wgpu-hal 30.0.1); this can only be reached when a
  live window's surface is lost twice in a row.
