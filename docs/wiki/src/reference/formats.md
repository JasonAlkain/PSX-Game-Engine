# File formats

Every file Copperframe reads is JSON with a `format` tag and a `version`:

```json
{ "format": "psx_scene", "version": 4, "scene": { … } }
```

A build loads every older version it knows, rejects newer versions (and
version 0) with a message naming the file, the version found and the
newest supported, and rejects unknown fields. Older files are never
rewritten on load; they are saved in the current version the next time the
engine writes them. Saves are atomic (temporary file, previous version kept
as `.bak`, rename).

The `psx_` prefixes are the engine's original name and are kept on purpose
so existing projects keep loading.

## Project files

| Format | Current | File | Contents |
| --- | --- | --- | --- |
| `psx_project` | 4 | `project.json` | `name`, `startup_scene`; optional `display {width, height, fullscreen, vsync}`, `style {default, player_can_change, look}` (`look` is the project look, a retro style), `build {export_dir, check_after_export, check_script}`, `physics {gravity}`, `audio {master, music, effects, voice, ambient, limiter}` (each bus `{volume_db, mute}`, the [Audio mixer](../guide/editor.md#the-audio-mixer)) (the [Project settings](../guide/editor.md#project-settings) panel; see also [Physics](../guide/physics.md)) |
| `psx_scene` | 8 | `scenes/*.scene.json` | ID, name, revision, settings (`look`, `style`, `lighting`, `camera`), entities. v1 to v7 load; v5 and older bodies move from the collider to a `rigidbody`; v6 and older keep their own look; v8 audio sources can use the `voice` and `ambient` buses. See [Scenes](../guide/scenes.md) |
| `psx_asset_meta` | 1 | `assets/**/<file>.meta.json` | Asset `id`, `importer` (`gltf`, `texture`, `audio`), `settings` (`scale`, `max_texture_size`). See [Asset pipeline](../guide/assets.md) |
| `psx_input` | 1 | `input.json` (optional) | Action → bindings. See [Input](../guide/player.md#input) |
| `psx_style` | 2 | `psx_style.json` (sandbox), and inside every scene | The retro style. v1 loads with PS1 values for the newer sections. See [Style](../guide/style.md) |

Every path stored in a project is relative to the project root with `/`
separators, and may not point outside the project.

## Tool and test files

| Format | Current | Where | Contents |
| --- | --- | --- | --- |
| `psx_command_batch` | 1 | Anywhere; applied with `copperframe apply` | A list of scene commands. See [Command batches](batches.md) |
| `psx_script` | 1 | For example `scripts/*.script.json` | Input steps and expectations. See [Testing](../guide/testing.md) |
| `psx_run_report` | 1 | Standard output of headless runs, or `--report` | Ticks, outcome, events, warnings, `passed`, `failures` |
| `psx_game_schema` | 1 | Standard output of `--schema`; cached in `.psx/game_schema.json` | The behaviour kinds a game registers, with typed fields |
| `psx_bench` | 1 | Standard output of `--bench`, or `--report` | Adapter, sizes, and tick/frame timing per style |
| `psx_export` | 1 | `export.json` in an export folder | Marks a folder `copperframe build --replace` may overwrite |

## Player files

Stored in the OS user folders, never in the project:

| Format | Current | File | Contents |
| --- | --- | --- | --- |
| `psx_player_settings` | 1 | `settings.json` in the config folder | Volumes, mouse, fullscreen, vsync, `graphics_style` (`scene`, `ps1`, `ps2`), key bindings |
| `psx_save` | 1 | `save.json` in the data folder | Scene path and game state map |

A damaged settings or save file is renamed to `*.json.bad` and defaults are
used.

## The `.psx/` folder

Each project has a working folder, `.psx/`, that should not be committed:

| Path | Purpose |
| --- | --- |
| `.psx/editor.lock` | The open editor's lock (process ID, with an OS lock) |
| `.psx/play/` | The scene copy Play mode runs |
| `.psx/game_schema.json` | Cached behaviour schema |

## History

[MIGRATION.md](https://github.com/JasonAlkain/PSX-Game-Engine/blob/main/docs/MIGRATION.md)
records every format version, importer versions, and Rust API changes per
milestone, with what you need to do when upgrading.
