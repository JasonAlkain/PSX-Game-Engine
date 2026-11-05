# Migration guide

Every file the engine reads carries a `format` tag and a `version`. A build
loads every older version it knows and rejects newer versions (and version
0) with a message that names the file, the version found and the newest
version supported. Older files are never rewritten on load; they are saved
in the current version the next time the engine writes them (the editor's
Save, `copperframe apply`, the player's settings screen).

## Rename to Copperframe

The engine was renamed from "PSX Engine" to **Copperframe Engine**. Names
that identify the engine changed; on-disk data formats did not, so every
existing project, scene, style file, batch, script, settings file and save
loads exactly as before.

| What | Before | After |
| --- | --- | --- |
| Crates | `psx_core`, `psx_scene`, `psx_assets`, `psx_render`, `psx_runtime`, `psx_editor`, `psx_game` | `copperframe_core`, `copperframe_scene`, `copperframe_assets`, `copperframe_render`, `copperframe_runtime`, `copperframe_editor`, `copperframe_game` |
| App packages | `psx_cli`, `psx_player`, `psx_sandbox` (in `apps/psx_*`) | `copperframe_cli`, `copperframe_player`, `copperframe_sandbox` (in `apps/copperframe_*`) |
| CLI binary | `psx` | `copperframe` (`copperframe new`, `copperframe validate`, ...) |
| Editor / player / sandbox binaries | `psx_editor`, `psx_player`, `psx_sandbox` | `copperframe_editor`, `copperframe_player`, `copperframe_sandbox` |
| Test environment variables | `PSX_REQUIRE_GPU`, `PSX_UPDATE_GOLDEN` | `COPPERFRAME_REQUIRE_GPU`, `COPPERFRAME_UPDATE_GOLDEN` |
| Player settings and saves (OS user folders) | `PSX Engine/<game>` | `Copperframe Engine/<game>` |

Kept as they were, because they are data rather than branding: the `psx_*`
`format` tags in the table below, `psx_style.json`, the project's `.psx/`
folder (editor lock, play copies, schema cache), and the engine behaviour
kinds `psx.player`, `psx.camera_zone` and `psx.spin`.

What you do:

- **Game crates** (the `game/` folder that `psx new` wrote): in
  `game/Cargo.toml`, change the dependency `psx_game = { path =
  ".../crates/psx_game" }` to `copperframe_game = { path =
  ".../crates/copperframe_game" }`, and replace `psx_game::` with
  `copperframe_game::` in the sources (also `psx_scene::` and friends if you
  use them).
- **Scripts and CI** that call `psx` or set `PSX_REQUIRE_GPU`: use
  `copperframe` and `COPPERFRAME_REQUIRE_GPU`.
- **Player settings and saves** written by an earlier build are in the old
  `PSX Engine` folder (for example `%APPDATA%\PSX Engine\<game>` on
  Windows). Move that folder's contents to `Copperframe Engine\<game>` to
  keep them; otherwise the game starts with defaults.

The editor, player and sandbox now show the Copperframe window icon, and the
editor and the player's title screen open with a short Copperframe splash
(about two and a half seconds; any key or click skips it). The editor's Play mode and
`--frames` runs skip the splash. Branding artwork lives in
`assets/branding/`.

## File formats

| Format | Current | Older versions | What changed | What you do |
| --- | --- | --- | --- | --- |
| `psx_project` (`project.json`) | 5 | 1, 2, 3, 4 | v2 (Project settings): optional `display`, `style`, `build` and `physics` sections. v3 (0.8): optional `style.look`, the project look. v4 (0.9): optional `audio` section, the project mixer (bus volumes and mutes, limiter); missing means every bus at 0 dB with the limiter on, which sounds like before apart from the limiter catching peaks over −1 dBFS. v5 (0.11): optional `scripting {lua}`; missing means Lua on, which only matters once the project has `scripts/**/*.lua` files | Nothing. A v1 file loads with the defaults (1280 × 960 window, vsync on, each scene's own look, export to `dist/<name>` with a start check, gravity 0, −9.81, 0); a v1 or v2 file gets the PS1 preset as the project look, which only scenes set to use it see. The editor writes v3 the next time project settings or the start scene are saved, which older engine builds refuse to open |
| `psx_asset_meta` (`*.meta.json`) | 1 | — | — | — |
| `psx_scene` (`scenes/*.scene.json`) | 7 | 1, 2, 3, 4, 5, 6 | v2 (M4): optional `collider`, `audio`, `behaviors`. v3 (M5): optional `light`, `animator`, `shadow`, and the style inside the scene gains `textures`, `shading`, `shadows`. v4: optional `tile_size` on primitive mesh sources. v5: optional `component_order` on entities (inspector order) | Nothing. Older scenes load unchanged and look the same: missing style sections get the PS1 values, primitives without `tile_size` keep one texture per face, and components without `component_order` are listed in the default order. To tile an existing primitive, tick **Tile texture** in the inspector. v6: moving bodies get their own `rigidbody` component; the collider's `body` and `mass` move there (a `dynamic` collider becomes a rigidbody, a `kinematic` one a rigidbody with `kinematic: true`), and colliders gain `restitution`. This happens on load. v7 (0.8): `settings.look` (`scene` or `project`); older scenes load as `scene`, so they look exactly as before. Switch a scene in Scene settings, or all of them from Project settings > Look. Saving writes v7, which older engine builds refuse to open. v8 (0.9): audio sources may name the `voice` and `ambient` buses; older scenes only use `music` and `effects` and load unchanged. Saving writes v8 |
| `psx_style` (`psx_style.json`, scene style) | 2 | 1 | `textures {filter, mipmaps}`, `shading {per_pixel, local_lights}`, `shadows {enabled, opacity}` | Nothing; v1 gets PS1 values. Pick the PS2 preset in the editor or sandbox to opt in |
| `psx_command_batch` | 1 | — | M5 added the ops `set_light`, `set_animator`, `set_shadow`; 0.4 added `set_component_order`; 0.5 added `set_rigidbody` and removed `body`/`mass` from `set_collider` | Batches that set `"body"` or `"mass"` on a collider no longer parse: delete `"body": "static"`, and replace `dynamic`/`kinematic` with a following `set_rigidbody` op (`{}` or `{"kinematic": true}`, with `mass` moved there) |
| `psx_input` (`input.json`) | 1 | — | — | — |
| `psx_game_schema` (`--schema`) | 1 | — | 0.4: optional `folder` on each behaviour. 0.11: Lua kinds (`lua.*`) and an optional top-level `problems` list | Nothing; readers ignore it if they do not know it |
| `psx_script`, `psx_run_report` | 1 | — | M5 events may include `animation_finished`; 0.11 reports add `diagnostics`, and a script error fails the run | Script expectations match by subset, so existing scripts pass |
| `psx_player_settings` | 1 | — | M5 added `graphics_style` (`scene`, `ps1`, `ps2`), defaulted to `scene` | Nothing |
| `psx_save` | 1 | — | — | — |
| `psx_export` (`export.json`) | 1 | — | — | — |
| `psx_bench` (`--bench` report) | 1 | — | New in M6 | — |
| `copperframe_play_status` (`--control stdin` replies) | 1 | — | New in 0.10 | — |

Fixtures that pin older versions, each loaded by a test:
`crates/copperframe_scene/fixtures/v1_room.scene.json` (scene v1, Milestone 3),
`crates/copperframe_scene/fixtures/v2_room.scene.json` (scene v2, Milestone 4),
`crates/copperframe_scene/fixtures/v3_room.scene.json` (scene v3, Milestone 5),
`crates/copperframe_scene/fixtures/v4_room.scene.json` (scene v4, release 0.1),
`crates/copperframe_scene/fixtures/v6_room.scene.json` (scene v6, release 0.5),
`crates/copperframe_assets/fixtures/v2.project.json` (project v2),
`crates/copperframe_assets/fixtures/v4.project.json` (project v4, release 0.9),
`crates/copperframe_core/fixtures/v1.style.json` (style v1),
`crates/copperframe_assets/fixtures/v1.project.json` (project v1),
`crates/copperframe_game/fixtures/m4_settings.json` (player settings without
`graphics_style`).

## Importer versions

Imports are computed from the source file when a project opens (there is no
cooked cache yet), so an importer change takes effect without any action.

| Version | Change |
| --- | --- |
| 1 | Static glTF, PNG/JPEG, WAV/Ogg |
| 2 (M5) | glTF skins and animation clips. Before, skins and animations were ignored with a warning and the mesh was drawn in its node pose; now a skinned mesh is drawn from its bind pose through the skeleton (the rest pose), which for most exported files is the same shape |

## Game code (Rust API)

### Lua scripting (0.11)

`ProjectFile` has a new `scripting: ScriptingSettings` field (use
`..Default::default()` in struct literals). `Simulation::new` now loads
the project's `scripts/**/*.lua` as behaviour kinds `lua.*`: a project
that already kept unrelated `.lua` files there gets them loaded (and
reported if they do not return a behaviour table); set
`"scripting": {"lua": false}` to keep the old behaviour. New API:
`Registry::register_dynamic` (`DynamicKind`), `Registry::add_problem`,
`ScriptDiagnostic`, `Ctx::report_script_problem`,
`Simulation::take_diagnostics`. Rust behaviours and `Game` code need no
change.

### Play control (0.10)

`PlayerOptions` has a new `control: Option<ControlInput>` field (pass
`None` to keep the old behaviour; `copperframe_game::main` fills it from
`--control stdin`). Game crates that only call `copperframe_game::main`
need no change.

### Audio mixer (0.9)

`AudioBus` gained `Voice` and `Ambient`: add arms for them to a `match`
(or use `_`). Mixer types live in the new `copperframe_audio` crate
(`Mixer`, `Strip`, `Preview`, meters); `copperframe_game::audio` still
re-exports `linear_to_db`. `ProjectFile` has a new `audio: AudioSettings`
field (use `..Default::default()` in struct literals), and
`Audio::set_mixer` applies it (the player does this from `project.json`).
Emitters and `Ctx` sounds now play on the bus's mixer track rather than
directly on the music or effects track, so a bus's mute and volume apply
to them.

### Project look (0.8)

`SceneSettings` has a new `look: LookSource` field (add `look:
LookSource::Scene` to a struct literal, or use `..Default::default()`), and
`Command::SetSettings` a new `look: Option<LookSource>` field. A scene's
`settings.style` is no longer always what it renders with: use
`project.file.style.scene_style(&scene.settings)` (or
`Simulation::project()` in game code) for the authored style, then
`GraphicsStyle::apply` for the player's choice. `StyleSettings` gained
`look` and is no longer `Eq`.

### Project settings

`ProjectFile` has new fields `display`, `style` and `build`: code that
builds one needs `..` or the three defaults. `copperframe_game::settings::
GraphicsStyle` now lives in `copperframe_assets::project` and is
re-exported, so existing paths keep working. The game window opens at the
project's window size; a first-run player starts with the project's
fullscreen, vsync and look (`UserSettings::for_project`), and
`UserDirs::load_settings_or` takes those defaults. The key list and default
actions are also in `copperframe_assets::input` for the editor; a test keeps
them equal to `copperframe_game::actions`.

### Component order and behaviour folders (0.4)

`copperframe_scene::Entity` has a new `component_order: Vec<String>` field:
add `component_order: Vec::new()` where you build an `Entity` with a struct
literal (`Entity::new` needs no change). `Command` has a new
`SetComponentOrder` variant, so an exhaustive `match` on `Command` needs an
arm for it. `Registry::register_in(folder, descriptor)` is new and
optional: `register` keeps working and lists the kind directly under
Behaviours.

### Texture tiling

`MeshSource::Primitive` has a new `tile_size: Option<f32>` field: add
`tile_size: None` where you build one, or `..` where you match one.
`copperframe_render::Material` has a new `uv_tile` field; code using
`..Material::default()` needs no change. `Command`s and batches accept the new
field in mesh sources, and leaving it out keeps the old behaviour.

Game crates depend on the engine by path, so they move with the checkout.

### Milestone 4 → 5

- `GameEvent` has a new variant, `AnimationFinished { entity, clip }`.
  A `match` over `GameEvent` without a wildcard arm needs a new arm.
- `copperframe_game::sim::DrawEntity` has a new field `joints: Option<Vec<Mat4>>`
  (only matters if you build `DrawEntity` values yourself).
- `copperframe_render::Lighting` has new fields `points` and `blobs`. Use
  `..Lighting::default()` or `copperframe_runtime::scene_gpu::scene_lighting` /
  `lighting_for_scene` to build it.
- `SceneResources::draws` takes a third argument, the joint palettes per
  entity (`&HashMap::new()` for none); `draws_from` items are 4-tuples with
  an optional palette.
- `copperframe_render` vertices are 72 bytes (`joints`, `weights` added to
  `Vertex`). Code that builds `Vertex` literals sets them to zero, or uses
  `MeshData` helpers.
- New: `Ctx::play_animation`, `play_animation_at`, `stop_animation`,
  `animation`, `is_animation_playing`, `animation_clips`.

### Milestone 5 → 6

- New `copperframe_game::bench` module and `--bench`/`--bench-size` flags. Exit
  code 1 now also covers "no graphics adapter for the benchmark".
- `Simulation::load_scene` frees cached models and decoded sounds the new
  scene does not reference; `SceneResources::release_unused` does the same
  for GPU assets. Code that kept an asset alive only by having loaded it in
  an earlier scene reloads it on demand.
