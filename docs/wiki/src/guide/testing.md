# Testing gameplay headless

The simulation runs the same with or without a window. A **headless run**
plays a scene with no window, GPU or audio device, drives it with an input
**script**, and prints a JSON **report**. Use it to test levels in CI, to
check a change did not break a puzzle, or to let an automated agent play
your game.

## Running

In the editor, use **Build > Run Test Script**. It lists every
`*.script.json` in the project (outside `assets/` and `game/`), has
**Choose a script file…** for others, and **Idle run** plays the scene for
5 seconds with no input. The run uses the open scene as saved (the editor
saves first), builds the game crate if needed, and shows the report in a
results window: **passed**, or the failed expectations.

From a command line (it builds the game crate first):

```sh
copperframe test-scene --project my_game --script my_game/scripts/walkthrough.script.json
copperframe test-scene --project my_game --ticks 600          # idle for 10 seconds
copperframe test-scene --project my_game scenes/level2.scene.json --script …
```

Or call the game executable directly:

```sh
cargo run -p lever_room -- --headless --script examples/room/scripts/walkthrough.script.json
cargo run -p lever_room -- --headless --ticks 300 --report report.json
```

Exit code 0 means the run passed; 1 means an expectation failed or the
scene could not be played.

## Scripts

```json
{
  "format": "psx_script",
  "version": 1,
  "steps": [
    { "ticks": 72, "hold": ["move_forward"] },
    { "ticks": 10, "look": [3.87, 0] },
    { "ticks": 1, "press": ["interact"] },
    { "ticks": 90 }
  ],
  "expect": {
    "finished": true,
    "events": [
      { "event": "custom", "name": "lever_pulled" },
      { "event": "trigger_entered", "sensor": "Exit" },
      { "event": "finished" }
    ]
  }
}
```

Each step runs for `ticks` ticks (60 per second):

| Field | Meaning |
| --- | --- |
| `hold` | Actions held for every tick of the step |
| `press` | Actions pressed on the step's first tick only |
| `look` | Mouse look in degrees per tick, `[x, y]`: +x turns right, +y looks up |

The run stops early when the game finishes or quits. A script may run at
most about 2.8 hours of game time.

`expect` is optional:

- `finished`: whether the game must (`true`) or must not (`false`) reach
  `ctx.finish`.
- `events`: each entry is a partial event. It matches if any event in the
  run has all of the entry's fields with the same values. Order is not
  checked.

## Events

| `event` | Fields | Logged when |
| --- | --- | --- |
| `scene_loaded` | `scene` | A scene loads |
| `interacted` | `entity` | The player interacts with an entity |
| `trigger_entered`, `trigger_exited` | `sensor`, `other` | A sensor's overlap starts or ends |
| `sound_started` | `entity` | An audio source starts |
| `animation_finished` | `entity`, `clip` | A non-looping clip reaches its end |
| `message` | `text` | `ctx.show_message` |
| `custom` | `name`, `detail` | Your code calls `ctx.log(GameEvent::Custom { … })` |
| `finished` | `message` | `ctx.finish` |

Entities are named by their scene name. Logging `custom` events for each
milestone in your game makes scripts short and robust.

## Reports

An abbreviated report from the demo's walkthrough:

```json
{
  "format": "psx_run_report",
  "version": 1,
  "game": "The Lever Room",
  "scene": "scenes/main.scene.json",
  "ticks": 535,
  "seconds": 8.92,
  "outcome": { "kind": "finished", "message": "You escaped!" },
  "events": [ { "tick": 72, "event": "custom", "name": "inspected", "detail": "Crate" }, … ],
  "warnings": [],
  "camera": { "position": [0.7, 2.6, …], "yaw_deg": …, "pitch_deg": … },
  "entities": …,
  "bodies": …,
  "passed": true,
  "failures": []
}
```

`warnings` lists problems found while loading (an unregistered behaviour
kind, a bad parameter) and actions the script uses that are not default
actions. `failures` explains each failed expectation.

## Writing a script

1. Play the level and note what to do: how far to walk, where to turn.
2. Walking speed is the player's `speed` (2.5 m/s by default), so 60 ticks
   of `move_forward` cover 2.5 m. Running uses `run_speed`.
3. Turn with `look` over a few ticks rather than one big step.
4. Add `expect.events` for each milestone and `finished: true` at the end.
5. Run with `copperframe test-scene` and adjust using the `camera` and
   `events` in the report.

## As a Rust test

The demo runs its walkthrough as a normal test
(`examples/room/game/tests/walkthrough.rs`), so `cargo test -p lever_room`
fails if the game can no longer be completed. The same works in your own
game crate (add `copperframe_assets` as a dev-dependency):

```rust
use std::path::{Path, PathBuf};
use copperframe_assets::project::Project;
use copperframe_game::{Simulation, audio::Audio, headless::{self, Script}};

#[test]
fn walkthrough_passes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let project = Project::open(&root).unwrap();
    let scene = PathBuf::from(&project.file.startup_scene);
    let mut sim = Simulation::new(project, Box::new(my_game::MyGame), Audio::silent());
    sim.load_scene_file(&scene).unwrap();
    let text = std::fs::read_to_string(root.join("scripts/walkthrough.script.json")).unwrap();
    let report = headless::run(&mut sim, &Script::parse(&text).unwrap());
    assert_eq!(report["passed"], true, "{report:#}");
}
```

## Benchmarks

`--bench` (and `copperframe bench`) uses the same simulation to measure
frame times. See [Game executable options](../reference/game-options.md#benchmarking).
