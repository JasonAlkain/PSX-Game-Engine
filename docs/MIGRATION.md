# Migration guide

Every file the engine reads carries a `format` tag and a `version`. A build
loads every older version it knows and rejects newer versions (and version
0) with a message that names the file, the version found and the newest
version supported. Older files are never rewritten on load; they are saved
in the current version the next time the engine writes them (the editor's
Save, `psx apply`, the player's settings screen).

## File formats

| Format | Current | Older versions | What changed | What you do |
| --- | --- | --- | --- | --- |
| `psx_project` (`project.json`) | 1 | — | — | — |
| `psx_asset_meta` (`*.meta.json`) | 1 | — | — | — |
| `psx_scene` (`scenes/*.scene.json`) | 3 | 1, 2 | v2 (M4): optional `collider`, `audio`, `behaviors`. v3 (M5): optional `light`, `animator`, `shadow`, and the style inside the scene gains `textures`, `shading`, `shadows` | Nothing. Older scenes load unchanged; missing style sections get the PS1 values, so an M4 scene looks the same. Saving writes v3, which older engine builds refuse to open |
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
`crates/psx_scene/fixtures/v1_room.scene.json` (scene v1, Milestone 3),
`crates/psx_scene/fixtures/v2_room.scene.json` (scene v2, Milestone 4),
`crates/psx_core/fixtures/v1.style.json` (style v1),
`crates/psx_game/fixtures/m4_settings.json` (player settings without
`graphics_style`).

## Importer versions

Imports are computed from the source file when a project opens (there is no
cooked cache yet), so an importer change takes effect without any action.

| Version | Change |
| --- | --- |
| 1 | Static glTF, PNG/JPEG, WAV/Ogg |
| 2 (M5) | glTF skins and animation clips. Before, skins and animations were ignored with a warning and the mesh was drawn in its node pose; now a skinned mesh is drawn from its bind pose through the skeleton (the rest pose), which for most exported files is the same shape |

## Game code (Rust API)

Game crates depend on the engine by path, so they move with the checkout.

### Milestone 4 → 5

- `GameEvent` has a new variant, `AnimationFinished { entity, clip }`.
  A `match` over `GameEvent` without a wildcard arm needs a new arm.
- `psx_game::sim::DrawEntity` has a new field `joints: Option<Vec<Mat4>>`
  (only matters if you build `DrawEntity` values yourself).
- `psx_render::Lighting` has new fields `points` and `blobs`. Use
  `..Lighting::default()` or `psx_runtime::scene_gpu::scene_lighting` /
  `lighting_for_scene` to build it.
- `SceneResources::draws` takes a third argument, the joint palettes per
  entity (`&HashMap::new()` for none); `draws_from` items are 4-tuples with
  an optional palette.
- `psx_render` vertices are 72 bytes (`joints`, `weights` added to
  `Vertex`). Code that builds `Vertex` literals sets them to zero, or uses
  `MeshData` helpers.
- New: `Ctx::play_animation`, `play_animation_at`, `stop_animation`,
  `animation`, `is_animation_playing`, `animation_clips`.

### Milestone 5 → 6

- New `psx_game::bench` module and `--bench`/`--bench-size` flags. Exit
  code 1 now also covers "no graphics adapter for the benchmark".
- `Simulation::load_scene` frees cached models and decoded sounds the new
  scene does not reference; `SceneResources::release_unused` does the same
  for GPU assets. Code that kept an asset alive only by having loaded it in
  an earlier scene reloads it on demand.
