# Game executable options

Every game built with `copperframe_game::main` (your game crate, the demo
`lever_room`, and the stock `copperframe_player`) accepts the same options.
The editor and the CLI drive games through exactly these flags.

```sh
cargo run -p lever_room --release -- [OPTIONS]
my_game.exe [OPTIONS]            # an exported game
```

| Option | Effect |
| --- | --- |
| `--project DIR` | Project to load. Default: `data/` next to the executable if it holds `project.json` (an export), otherwise the development project the game was built from |
| `--scene FILE` | Scene to start in, relative to the project (default: the startup scene) |
| `--play` | Editor play mode: no title screen, never reads or writes saves |
| `--user-dir DIR` | Keep settings and saves in `DIR` instead of the OS user folders |
| `--frames N` | Exit after presenting N frames (smoke tests). With `--bench`: measured frames per style |
| `--schema` | Print the behaviour kinds the game registers (`psx_game_schema` JSON) and exit |
| `--headless` | Run without a window, GPU or audio and print a JSON report |
| `--script FILE` | Drive a headless run with a `psx_script` (implies `--headless`); exit 1 if an expectation fails |
| `--ticks N` | Ticks for a headless run without a script (default 600 = 10 s) |
| `--report FILE` | Also write the headless or bench report to `FILE` |
| `--bench` | Measure tick and frame times offscreen (see below) |
| `--bench-size WxH` | Output size for `--bench` (default 1280x720, each side 16–8192) |
| `-h`, `--help` | List the options |

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | Success |
| 1 | The scene cannot be played, a headless expectation failed, or the benchmark could not run (no adapter) |
| 2 | Usage error |
| 4 | Project or file error |

Logs go to standard error, so the JSON on standard output from `--schema`,
`--headless` and `--bench` is always clean. Set `RUST_LOG=debug` for more
logging.

## Benchmarking

```sh
cargo run -p lever_room --release -- --bench
cargo run -p lever_room --release -- --bench --frames 600 --bench-size 1920x1080 --report bench.json
copperframe bench --project examples/room --script examples/room/scripts/walkthrough.script.json
```

In the editor, **Build > Advanced > Benchmark Scene** runs the same on the
open scene (300 frames at 1280x720) and shows the report in a results
window.

The benchmark plays the scene with the real simulation (idle, or driven by
`--script`) and renders every tick offscreen in three styles: the scene's
own, PS1 and PS2. The first 10 frames per style are not measured (pipeline
creation and uploads). Frame time runs from the start of the tick until the
GPU has finished the frame, so it is an upper bound.

The `psx_bench` report names the adapter, backend and driver, the build
profile, and for each style the internal size, draw and triangle counts and
`tick` and `frame` statistics (`mean_ms`, `p50_ms`, `p95_ms`, `max_ms`).
Compare numbers only on the same machine and adapter. `copperframe bench`
always builds in release first; when running the game directly, use
`--release` or the numbers are debug numbers.

Reference numbers for the demo on a software rasteriser (llvmpipe) are in
[STATUS.md](https://github.com/JasonAlkain/PSX-Game-Engine/blob/main/docs/STATUS.md#milestone-6-hardening-and-platforms).
Hardware GPU numbers have not been recorded yet.

## Environment variables

| Variable | Effect |
| --- | --- |
| `RUST_LOG` | Log filter, for example `debug` or `copperframe_game=trace` |
| `WGPU_BACKEND` | Force `vulkan`, `dx12`, `metal` or `gl` |
| `COPPERFRAME_CRASH_DIR` | Where crash reports go (the editor sets this during Play so it can show a game's crash) |
