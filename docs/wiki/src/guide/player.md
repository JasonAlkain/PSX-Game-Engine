# Input, audio and the player app

Every game built on `copperframe_game` gets the same player application:
splash, title screen, HUD, pause and settings menus, saves, and a finished
screen. You supply the scenes and behaviours.

## Input

Gameplay reads named **actions**, never keys. Every game has these
defaults:

| Action | Default bindings |
| --- | --- |
| `move_forward` | `W`, `↑` |
| `move_back` | `S`, `↓` |
| `move_left` | `A`, `←` |
| `move_right` | `D`, `→` |
| `run` | Left `Shift` |
| `jump` | `Space` |
| `interact` | `E`, left mouse button |
| `pause` | `Esc` |

To change the defaults or add your own actions, put an `input.json` in the
project root:

```json
{
  "format": "psx_input",
  "version": 1,
  "actions": {
    "interact": ["KeyE", "MouseLeft"],
    "flashlight": ["KeyF"]
  }
}
```

Key names are winit `KeyCode` names: `KeyA`–`KeyZ`, `Digit0`–`Digit9`,
`ArrowUp`/`Down`/`Left`/`Right`, `Space`, `Enter`, `Escape`, `Tab`,
`Backspace`, `ShiftLeft`/`Right`, `ControlLeft`/`Right`, `AltLeft`/`Right`,
`CapsLock`, `F1`–`F12`, punctuation (`Minus`, `Equal`, `BracketLeft`,
`BracketRight`, `Semicolon`, `Quote`, `Comma`, `Period`, `Slash`,
`Backslash`, `Backquote`), `Insert`, `Delete`, `Home`, `End`, `PageUp`,
`PageDown`, `Numpad0`–`Numpad9`, `NumpadEnter`. Mouse buttons are
`MouseLeft`, `MouseRight` and `MouseMiddle`.

Players can rebind any action in **Settings**; their bindings are stored
in their settings file and applied on top of the project's.

In code:

```rust
let input = ctx.input();
if input.pressed("flashlight") { /* true only on the tick the key went down */ }
if input.held("run") { /* true while held */ }
let strafe = input.axis("move_left", "move_right"); // -1, 0 or 1
let look = input.look; // degrees this tick, sensitivity and invert-Y applied
```

A press is delivered to exactly one tick, even if it was shorter than a
tick. OS key repeat is ignored. Losing window focus releases every key and
the mouse and pauses the game. Mouse look uses raw mouse motion while the
cursor is captured.

Gamepads are not supported yet.

## Audio

Audio uses three buses: **master**, **music** and **effects**, with volumes
the player sets in Settings. Each **audio source** component plays on the
music or effects bus. Spatial sources follow their entity, and the single
listener follows the camera.

From code, through the entity's audio source:

```rust
ctx.play_sound(me);
ctx.stop_sound(me, 0.5);          // fade out over 0.5 s
if ctx.is_sound_playing(me) { … }
```

Pausing fades the effects bus out. If the computer has no audio device, the
game logs a warning and runs silently. Headless runs never open a device.

## Title, HUD and menus

- **Title screen:** New game, Continue (when a save exists), Settings, Quit.
  The window and title screen show the game's `title()`.
- **HUD:** a crosshair, the interaction prompt (`[E] Open door`) and
  messages from `ctx.show_message`.
- **Pause** (`Esc`): Resume, Settings, Quit to title, Quit game.
- **Finished screen:** shown when a behaviour calls `ctx.finish(message)`.

## Settings

| Setting | Notes |
| --- | --- |
| Master, music and effects volume | 0–1 |
| Mouse sensitivity, invert Y | |
| Fullscreen, vsync | |
| Graphics style | *As authored* (the scene's style), *PS1* or *PS2*. Applies a preset's look while keeping the scene's fog and atmosphere |
| Key bindings | Per action |

Settings are a `psx_player_settings` JSON file in the OS config folder; a
damaged file is renamed to `settings.json.bad` and defaults are used.

## Saves

A save (`psx_save`) holds the current scene path and the game state map
(`ctx.state()`). It is written when the player quits to the title or quits
from the pause menu, used by **Continue**, and deleted when the game is
finished. Play mode in the editor never reads or writes saves.

Saves do not store the player's position: **Continue** restarts the saved
scene at its start with the saved state. Design behaviours to restore
themselves from the state in `start` (the demo's door opens itself if
`door_open` is set).

## Where files go

Under the OS folders for `Copperframe Engine/<game title as a slug>`:

| OS | Example (*The Lever Room*) |
| --- | --- |
| Windows | `%APPDATA%\Copperframe Engine\the-lever-room\config` and `…\data` |
| Linux | `~/.config/the-lever-room` and `~/.local/share/the-lever-room` |

`--user-dir DIR` puts settings and saves in `DIR` instead (useful for
testing). Crash reports and logs go to the data folder too; see
[Crash reports and logs](../help/crash-reports.md).
