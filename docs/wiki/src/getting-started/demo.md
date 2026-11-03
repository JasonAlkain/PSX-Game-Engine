# Playing the demo

*The Lever Room* is the example project in `examples/room`. It is small,
but it uses every major engine feature: physics, sensors, a fixed camera
zone, positional audio, an animated character, point lights and blob
shadows, menus and saves.

```sh
cargo run -p lever_room --release
```

The game opens with a short Copperframe splash (any key or click skips it),
then the title screen.

## Goal

Find the lever that opens the door, follow the humming corridor, and reach
the exit. The caretaker by the east wall waves and talks when you use him.

## Controls

| Input | Action |
| --- | --- |
| `W` `A` `S` `D` or arrow keys | Move |
| Mouse | Look |
| `Shift` | Run |
| `Space` | Jump (if the scene's player allows it) |
| `E` or left click | Interact with what you are aiming at |
| `Esc` | Pause |

Every action can be rebound in **Settings**.

## Settings

The title and pause menus open **Settings**, which holds:

- master, music and effects volume;
- mouse sensitivity and invert Y;
- fullscreen and vsync;
- **Graphics style**: *As authored* (the scene's own style), *PS1* or *PS2*.
  Switching applies immediately while playing. This is the quickest way to
  see the two looks on the same content;
- key rebinding.

Settings and the save used by **Continue** are stored in your user folder,
never in the project:

| OS | Settings | Saves |
| --- | --- | --- |
| Windows | `%APPDATA%\Copperframe Engine\the-lever-room\config` | `%APPDATA%\Copperframe Engine\the-lever-room\data` |
| Linux | `~/.config/the-lever-room` | `~/.local/share/the-lever-room` |
| macOS | a folder for the game under `~/Library/Application Support` (untested) | same |

The folder name comes from the game's title. A save is written when you
quit to the title or quit from the pause menu, and deleted when you finish
the game.

## Watching it play itself

The demo ships with an input script that walks through the whole game.
Run it headless (no window, GPU or audio) and you get a JSON report:

```sh
cargo run -p lever_room -- --script examples/room/scripts/walkthrough.script.json
```

The run fails with exit code 1 if any expected event (the lever pulled, the
door opening, reaching the exit) does not happen. See
[Testing gameplay headless](../guide/testing.md).

## The renderer sandbox

The sandbox shows a hard-coded comparison scene that is useful for learning
what each style effect does:

```sh
cargo run -p copperframe_sandbox --release
```

Click in the window to capture the mouse, then `W` `A` `S` `D` to move, `E`
and `Q` for up and down, `Shift` to go faster, `Esc` to release the mouse.

| Key | Action |
| --- | --- |
| `1` | Low-resolution rendering on/off |
| `2` | Vertex snapping on/off |
| `3` | Affine texture mapping on/off |
| `4` | Colour reduction on/off |
| `5` | Dithering on/off |
| `6` | Fog on/off |
| `7` | Integer scaling / aspect fit |
| `[` `]` | Colour bits per channel down/up |
| `-` `=` | Snap grid finer/coarser |
| `P` | Next preset (PS1, PS2, Clean) |
| `C` | Split view: current style on the left, effects off on the right |
| `F5` / `F9` | Save / reload the style file |
| `F1` | Print the key list to the log |

The window title shows the current state. Options: `--style PATH` picks the
style file (default `psx_style.json` in the current folder, loaded at start
if it exists), `--split` starts in split view, `--frames N` exits after N
frames.
