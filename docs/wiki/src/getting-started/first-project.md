# Your first project

This walks through creating a project, adding an object with your own
behaviour, playing it, and exporting it, all from the editor.

## 1. Start the editor

- **Downloaded from itch.io:** double-click `copperframe_editor.exe`.
- **From the engine source:** run `cargo editor` in the engine folder. The
  first start compiles the editor and takes a few minutes.

The editor opens at the [project hub](../guide/projects.md).

## 2. Create the project

Click **New project…**, then:

1. Type a **Name**, for example *My Game*.
2. Keep the **Location** (`Documents\Copperframe Projects`) or pick
   another folder with **Browse…**.
3. Choose the **Starter scene** template.
4. Leave **Include Rust game code** ticked if it is available, so you can
   write your own behaviours in step 4.
5. Click **Create**.

The project opens in the editor right away. Next time it is at the top of
the hub's **Recent projects**. The project folder holds:

```text
My Game/
├── project.json            # name and startup scene
├── assets/                 # your models, textures and sounds (with .meta.json sidecars)
├── scenes/
│   └── main.scene.json     # a playable starter scene
├── game/                   # Rust crate with your gameplay code (if included)
│   ├── Cargo.toml          # depends on this engine checkout by path
│   ├── rust-toolchain.toml
│   └── src/
│       ├── lib.rs          # your Game and behaviours
│       └── main.rs         # one line: copperframe_game::main(...)
└── .gitignore
```

The starter scene has a floor, a spinning cube (the built-in `psx.spin`
behaviour), both with box colliders, and a player (`psx.player`), so it is
playable immediately. Without game code the project plays in the stock
player with the built-in behaviours only.

The editor shows the scene in the PS1 style. Try:

- **Create > Cube** to add another cube in front of the camera;
- drag the coloured gizmo arrows to move it;
- hold the right mouse button and use `W` `A` `S` `D` `Q` `E` to fly;
- press `Ctrl+S` to save.

See [The editor](../guide/editor.md) for everything else.

## 3. Play it

Press **Play** (or `F5`). The editor builds your game crate and runs it in
a separate window on a copy of the current scene, including unsaved edits.
The game's log appears in the editor's **Console**. Press **Stop** (or `F5`)
to end it. Play never changes your scene file.

The first Play compiles the game crate, so it takes a while.

## 4. Add a behaviour to your cube

`game/src/lib.rs` already registers an example behaviour, `game.bob`, that
moves an entity up and down. In the editor:

1. Select the cube you created.
2. In the inspector's **Behaviours** section, add `game.bob`. If the list
   is empty, use **Game > Reload behaviour kinds** so the editor rebuilds
   the game and reads which behaviours it registers.
3. Set `height` and `speed`, then **Play**.

To write your own, add a `BehaviorDescriptor` in `register` and a type that
implements `Behavior`. [Behaviours and game code](../guide/behaviours.md)
explains how, with the full [Ctx reference](../reference/ctx.md).

## 5. Add your own art

Copy `.glb`/`.gltf` models, `.png`/`.jpg` textures and `.wav`/`.ogg` sounds
into the project's `assets/` folder (**Show in folder** in the **Assets**
tab opens it). The editor notices new files within a second or two and
lists them in the **Assets** tab; click **Add to scene** next to a model.
When you save a changed file from your paint or modelling program, the
editor reimports it in place.
Textures are picked in a mesh's inspector and sounds in an audio source's.
See [Asset pipeline](../guide/assets.md).

## 6. Choose the look

Select nothing to see **Scene settings** in the inspector. Click **PS1**,
**PS2** or **Clean**, or turn individual effects on and off. See
[PS1 and PS2 style settings](../guide/style.md).

## 7. Check and export

1. **Build > Validate Project** checks every asset and scene and lists any
   problems in a results window.
2. **Build > Export Game** writes a standalone copy of the game into the
   **Export folder** (default `dist/My Game` in the project; **Browse…**
   picks another). It compiles a release build, copies the project data
   next to it, writes licence notices for every third-party crate, and runs
   the result once to check it starts.
3. **Open folder** in the results window shows the export. Zip that folder
   to share it.

See [Exporting a game](../guide/exporting.md).

## Keeping projects in version control

The generated `.gitignore` excludes build output and the editor's working
folder `.psx/`. Commit `project.json`, `input.json` if you have one,
`scenes/`, `assets/` (sources and their `.meta.json` files) and `game/`.
