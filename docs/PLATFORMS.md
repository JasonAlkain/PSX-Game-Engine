# Platform validation

What has been run on each platform, and the manual checklist for the ones
that have not. Record results in `docs/STATUS.md` (Verification record or
Not verified) with the date, machine, GPU and driver.

## Status

| Platform | Build | Automated tests | Windowed runtime | Evidence |
| --- | --- | --- | --- | --- |
| Linux x86-64 (Ubuntu 24.04, X11) | yes | all, GPU tests on Mesa lavapipe (CI and container) | editor, player and sandbox under Xvfb with lavapipe; no hardware GPU | STATUS Verification record; `copperframe bench` numbers below |
| Windows 11 x86-64 (primary target) | CI (`windows-latest`): clippy, tests, release build | CI; GPU tests run only if the runner exposes an adapter (WARP), otherwise print `SKIPPED` | not run | — |
| macOS | not built | not run | not run | — |

Software adapters (lavapipe, WARP) are correct but slow; performance
numbers from them do not predict hardware GPUs.

## Measuring performance

```sh
copperframe bench --project examples/room                       # idle, 300 frames per style
copperframe bench --project examples/room --script examples/room/scripts/walkthrough.script.json
cargo run -p lever_room --release -- --bench --frames 600 --bench-size 1920x1080 --report bench.json
```

The benchmark builds the game in release, plays the scene with the real
simulation and renders each frame offscreen in the authored, PS1 and PS2
styles. Frame time runs from the start of the tick until the GPU has
finished the frame (no pipelining), so it is an upper bound. The report
names the adapter, backend and driver.

## Windows 11 checklist

Use a release build on a machine with a hardware GPU (note GPU, driver and
monitor setup).

1. `cargo test --workspace` with `COPPERFRAME_REQUIRE_GPU=1`: all pass. Reference
   images are compared only on llvmpipe; on other adapters the test only
   renders them.
2. `cargo run -p copperframe_sandbox --release`: the comparison scene renders.
   Keys 1–7 toggle effects, `P` cycles PS1 → PS2 → Clean, `C` shows the
   split view, F5 saves `psx_style.json` and a restart reloads it. Resize,
   minimise and restore, drag between monitors with different scaling, and
   Alt+Tab with the mouse captured.
3. `cargo run -p copperframe_editor --release -- examples/room`: the
   Copperframe splash shows for about two and a half seconds (a key or click skips it)
   and the taskbar shows the Copperframe icon; select Caretaker,
   confirm the animator shows `idle` and the viewport animates it; switch
   Scene settings between PS1 and PS2; move a crate, Ctrl+S, reopen, the
   crate stayed; Ctrl+Z then close shows the unsaved-changes dialog; F5
   plays, Stop returns to the editor with the scene unchanged.
4. `cargo run -p lever_room --release`: the splash shows before the title
   screen; walk, inspect the crate, pull the
   lever, talk to the caretaker (wave then idle), reach the exit. Settings:
   switch Graphics style between As authored, PS1 and PS2 while playing;
   volume sliders; machine hum louder near the machine; rebinding a key
   survives a restart; settings appear under
   `%APPDATA%\Copperframe Engine\the-lever-room\config`.
5. `cargo run -p copperframe_cli --release -- build --project examples/room --out
   "%USERPROFILE%\Desktop\Lever Room"`, run `lever_room.exe` from that
   folder and finish the game.
6. `cargo run -p copperframe_cli --release -- bench --project examples/room`:
   record the report in STATUS.
7. Project hub: start the editor with no arguments (double-click
   `target\release\copperframe_editor.exe` or `cargo editor`). The hub
   opens; **New project…** → Browse… shows the Windows folder picker; create
   a Starter project with and without Rust game code, and a Lever Room
   project; each opens straight away and appears under Recent projects on
   the next start. Copy a PNG into the project's `assets` folder from
   Explorer and confirm it appears in the Assets panel within about two
   seconds; edit a texture used in the scene in Paint, save, and confirm the
   viewport updates. Build > Validate, Run Test Script and Export Game show
   results; **Open folder** opens Explorer. File > Close Project returns to
   the hub.
8. Crash reporting: put any text file named `test.txt` in
   `%APPDATA%\Copperframe Engine\copperframe-engine\data\crash_reports`,
   start the editor, and confirm the "closed unexpectedly" dialog shows it;
   "Email report…" opens a draft in the default mail program addressed to
   `zacharyaars+copperframeengine@gmail.com`; after closing it the dialog
   does not come back. Help → "Report a bug…" → "Open email draft" does the
   same. `logs\copperframe_editor.log` holds the session log.

## macOS checklist

Not supported yet beyond what `wgpu` (Metal), `winit`, `kira` and
`directories` provide. Run steps 1–6 above (paths: `~/Library/Application
Support/Copperframe Engine/the-lever-room`), plus a Retina display check of the
integer upscale and the editor's picking at scale factor 2.

## Linux checklist (hardware GPU)

Steps 1–6 above on X11 and on Wayland, plus a run with `WGPU_BACKEND=gl`
to exercise the OpenGL ES fallback (see Known limitations in STATUS).
