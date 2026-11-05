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
| `psx_project` (`project.json`) | 1 | — | — | — |
| `psx_asset_meta` (`*.meta.json`) | 1 | — | — | — |
| `psx_scene` (`scenes/*.scene.json`) | 4 | 1, 2, 3 | v2 (M4): optional `collider`, `audio`, `behaviors`. v3 (M5): optional `light`, `animator`, `shadow`, and the style inside the scene gains `textures`, `shading`, `shadows`. v4: optional `tile_size` on primitive mesh sources | Nothing. Older scenes load unchanged and look the same: missing style sections get the PS1 values, and primitives without `tile_size` keep one texture per face. To tile an existing primitive, tick **Tile texture** in the inspector. Saving writes v4, which older engine builds refuse to open |
| `psx_style` (`psx_style.json`, scene style) | 2 | 1 | `textures {filter, mipmaps}`, `shading {per_pixel, local_lights}`, `shadows {enabled, opacity}` | Nothing; v1 gets PS1 values. Pick the PS2 preset in the editor or sandbox to opt in |
| `psx_command_batch` | 1 | — | M5 added the ops `set_light`, `set_animator`, `set_shadow` | Batches written for M3/M4 still apply |
| `psx_input` (`input.json`) | 1 | — | — | — |
| `psx_game_schema` (`--schema`) | 1 | — | — | — |
| `psx_script`, `psx_run_report` | 1 | — | M5 events may include `animation_finished` | Script expectations match by subset, so existing scripts pass |
| `psx_player_settings` | 1 | — | M5 added `graphics_style` (`scene`, `ps1`, `ps2`), defaulted to `scene` | Nothing |
| `psx_save` | 1 | — | — | — |
| `psx_export` (`export.json`) | 1 | — | — | — |
| `psx_bench` (`--bench` report) | 1 | — | New in M6 | — |

Fixtures that pin older versions, each loaded by a test:
`crates/copperframe_scene/fixtures/v1_room.scene.json` (scene v1, Milestone 3),
`crates/copperframe_scene/fixtures/v2_room.scene.json` (scene v2, Milestone 4),
`crates/copperframe_scene/fixtures/v3_room.scene.json` (scene v3, Milestone 5),
`crates/copperframe_core/fixtures/v1.style.json` (style v1),
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
