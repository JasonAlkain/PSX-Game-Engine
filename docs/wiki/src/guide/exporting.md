# Exporting a game

An export is a self-contained folder you can zip and share. It builds for
the platform you run it on: export on Windows for a Windows build.

## From the editor

1. Set the **Export folder** on the **Build** page of
   [Project settings](editor.md#project-settings) (default `dist/<name>` in
   the project). The **Build** menu shows the folder too and can change it
   for this session.
2. Click **Build > Export Game**.

The editor saves unsaved changes, exports in the background (build output
streams into the Console), replaces an earlier export in that folder, and
runs the result once to check it starts (turn the check off, or pick a test
script for it, on the same Build page). A results window shows where the
game is; **Open folder** opens it.

Projects without a `game/` crate export the stock player renamed after the
game, so they need no Rust. The itch.io download exports these projects
too, using the licence list it ships in `NOTICES.txt`.

## From the command line

```sh
copperframe build --project my_game --out "my_game/dist/My Game"
```

| Option | Meaning |
| --- | --- |
| `--out DIR` | Output folder (default: the project's export folder setting, else `dist/<name>`). Created if missing; must be empty or an earlier export |
| `--replace` | Overwrite an earlier export in `DIR` |
| `--script FILE` | Script to use for the check run (default: the project's setting, else an idle run) |
| `--no-verify` | Skip the check run (also skipped when the project's build settings turn it off) |
| `--json` | Machine-readable result |

## What happens

1. The project is validated (the same checks as **Build > Validate Project**); errors stop the export.
2. The game crate is built in release mode.
3. The output folder is written:

   | Path | Contents |
   | --- | --- |
   | `<game>` (`.exe` on Windows) | The game executable |
   | `data/` | `project.json`, `input.json`, scenes and assets |
   | `NOTICES.txt` | Licences of every third-party crate compiled into the executable |
   | `README.txt` | How to run the game |
   | `export.json` | Marks the folder as an export, so `--replace` may overwrite it |

4. Unless `--no-verify` is given, the export is copied to a temporary folder
   whose name contains spaces and run headless from there (with `--script`
   if given), to prove it starts and finds its data.

The executable looks for `data/` next to itself, so the folder can be moved,
renamed or zipped. Settings and saves go to the player's user folder, never
into the export.

## Size

Exports are about 30 MB, mostly the executable, because binaries are not
stripped: release builds keep line tables so crash backtraces have file and
line numbers.

## Licences

`NOTICES.txt` is generated from the dependency graph of the executable
(about 460 crates for the demo). Ship it with your game. Copperframe itself
is MIT-licensed.

## Checklist before shipping

- **Build > Validate Project** reports no errors.
- Your walkthrough script passes: **Build > Run Test Script**.
- Play the exported executable from its folder, not from the project.
- Check settings and saves appear in the user folder (see
  [Where files go](player.md#where-files-go)).
- Run **Build > Advanced > Benchmark Scene** on the slowest machine you
  want to support.
