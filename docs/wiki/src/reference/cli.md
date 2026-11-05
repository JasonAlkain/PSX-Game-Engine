# Command-line tool

`copperframe` creates, checks and edits projects. It uses the same command
layer as the editor, so anything you can do to a scene in the editor you
can script.

You do not need it to make games: the editor's [project
hub](../guide/projects.md) and **Build** menu do what `new`, `import`,
`validate`, `test-scene`, `build` and `bench` do. The tool is for scripts,
CI and automated agents; `inspect`, `apply` and `schema` exist only here.

```sh
cargo run -p copperframe_cli --release -- <command> [options]
# or, after cargo build --release:
target/release/copperframe <command> [options]
```

Every command accepts `--json` to print one JSON object instead of text,
and `--project DIR` (default: the current folder) where it applies.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 1 | Validation failed, a batch command failed, or a test expectation failed |
| 2 | Usage error (bad arguments) |
| 3 | Conflict: the editor holds the project lock, or the scene's revision is not the one a batch expected |
| 4 | I/O error (missing or unreadable files) |

## `new`

```sh
copperframe new DIR [--name NAME] [--template empty|starter|lever-room] [--no-game]
```

Creates a project: `project.json`, `assets/`, `scenes/main.scene.json` and
`game/`, a Rust crate with an example behaviour that depends on this engine
checkout by path. `--template` picks what the project starts with (default
`starter`, a playable scene; see
[Templates](../guide/projects.md#templates)). `--no-game` skips the crate;
the project then plays in the stock player with engine behaviours only.
Without the engine source (a downloaded build) `new` needs `--no-game`.
`DIR` must not exist or must be empty.

## `validate`

```sh
copperframe validate [--project DIR]
```

Checks the project file, every asset sidecar, every import, and every scene
(schema, hierarchy, references to unknown assets or assets of the wrong
kind, animator clips). Prints each diagnostic with its severity; exits 1 if
any is an error. Warnings do not fail.

## `import`

```sh
copperframe import [--project DIR] [FILE]
```

Registers an asset under `assets/` (writes its `.meta.json`) and
test-imports it, reporting its ID, kind, triangle, material and texture
counts, warnings, and for skinned models the joint count and clip names.
`FILE` is a path from the current folder. With no file, registers every
unregistered asset.

```text
$ copperframe import --project examples/room examples/room/assets/characters/caretaker.glb
b37004fb-… assets/characters/caretaker.glb (model) {"clips":[{"duration_secs":3.0,"name":"idle"},{"duration_secs":1.8,"name":"wave"}],"joints":5,"materials":1,"parts":1,"textures":0,"triangles":168}
```

## `inspect`

```sh
copperframe inspect [--project DIR] [SCENE]
```

Prints a scene (default: the startup scene) with each entity's ID, name,
mesh and position. `--json` prints every component in full, which is the
easiest way to find IDs for a batch.

```text
scene "Main" 51f2a909-… revision 0 (3 entities)
  94adcbe4-… "Floor" [Plane] at [0.0, 0.0, 0.0]
  7ee7a3ed-… "Cube" [Cube] at [0.0, 0.5, -1.5]
  fcfd3d9c-… "Player" at [0.0, 0.0, 2.5]
```

## `apply`

```sh
copperframe apply [--project DIR] [--scene SCENE] [--dry-run] BATCH
```

Applies a [command batch](batches.md) to a scene, all or nothing. `BATCH`
may be `-` to read standard input. `--dry-run` validates and reports
without writing. Refuses (exit 3) while the editor has the project open,
or when the batch's `expected_revision` does not match the scene.

## `schema`

```sh
copperframe schema [--project DIR]
```

Describes the file formats and versions this build reads, the components,
and every batch command with its fields. With `--project`, also builds the
project's game and lists the behaviour kinds it registers with their
fields. This is the reference to hand an automated agent.

## `test-scene`

```sh
copperframe test-scene [--project DIR] [SCENE] [--script FILE | --ticks N]
```

Builds the game and plays the scene headless (no window, GPU or audio),
driven by a script or idle for `--ticks` (default 300). Prints a
[run report](../guide/testing.md#reports); exits 1 if an expectation fails.

## `build`

```sh
copperframe build [--out DIR] [--project DIR] [--replace] [--script FILE] [--no-verify]
```

Exports the game for this platform and checks the export starts. Without
`--out`, `--script` and `--no-verify`, the project's build settings decide. See
[Exporting a game](../guide/exporting.md).

## `bench`

```sh
copperframe bench [--project DIR] [SCENE] [--script FILE] [--frames N] [--size WxH]
```

Builds the game in release, then measures simulation tick and frame times
offscreen for `--frames` frames (default 300) in the authored, PS1 and PS2
styles, at `--size` (default 1280x720). Needs a graphics adapter. See
[Benchmarking](game-options.md#benchmarking).

## Examples

```sh
# Find the door's ID, then move it up 1 m without the editor
copperframe inspect --project examples/room --json | jq '.scene.entities[] | select(.name=="Door") | .id'
echo '{"format":"psx_command_batch","version":1,"commands":[
  {"op":"set_transform","id":"<door id>","translation":[0,1,0]}]}' \
  | copperframe apply --project examples/room --dry-run -

# CI: fail the build if the project is broken or the walkthrough fails
copperframe validate --project my_game
copperframe test-scene --project my_game --script my_game/scripts/walkthrough.script.json
```
