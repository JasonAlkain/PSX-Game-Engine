# The editor

Start the editor by double-clicking `copperframe_editor.exe` (the itch.io
download) or with `cargo editor` in the engine folder. It opens at the
[project hub](projects.md), where you create or open a project.

From a command line you can also open a project directly:

```sh
copperframe_editor [PROJECT_DIR] [--scene PATH] [--steal-lock] [--frames N]
cargo editor [PROJECT_DIR] [...]       # the same, from the engine source
```

Without `PROJECT_DIR` the editor starts at the hub. `--scene` opens a scene
other than the project's startup scene. `--steal-lock` takes over a project
whose lock is held by a hung editor (see [Project lock](#project-lock)).
`--frames N` exits after N frames, for smoke tests.

![The editor with The Lever Room open: hierarchy on the left, the viewport filling its panel with a crate selected and its move gizmo, the toolbar above, the inspector and Scene settings tabs on the right, and the Assets browser and Console tabs at the bottom](../images/editor_lever_room.png)

## Layout

| Area | What it does |
| --- | --- |
| Menu bar | **File**, **Edit**, **Create**, **Game**, **Build**, **Window** and **Help** |
| Toolbar | The transform tools, snapping, view options and **Play**/**Stop** (see [The toolbar](#the-toolbar)) |
| Hierarchy | Every entity in the scene, children indented under parents. Click to select |
| Viewport | The scene rendered with its own retro style, with the selection gizmo and optional collider wireframes |
| Inspector | The selected entity's components |
| Scene settings | The scene's style, lighting and start camera |
| Assets | The [asset browser](#the-assets-panel) |
| Console | The log, including a running game's output |
| Status bar | Project and scene name, revision, unsaved state, frame rate, draw and triangle counts, and "idle" when the editor is not redrawing |

### Arranging panels

Every panel is a tab and can be rearranged:

- **Move** a panel by dragging its tab: onto another tab bar to join that
  group, or to the edge of a panel to split it there.
- **Resize** by dragging the border between panels.
- **Close** with the × on its tab, or middle-click the tab.
- **Window** lists every panel with a tick for whether it is open. Ticking a
  closed panel brings it back where it was. **Window > Reset layout** puts
  every panel back in its standard place.

The arrangement and the toolbar settings are remembered between sessions and
apply to every project. They are stored in `editor_layout.json` in the
engine's data folder; deleting it restores the defaults.

## The toolbar

| Control | What it does |
| --- | --- |
| Move (`Q`), Rotate (`W`), Scale (`E`) | The transform tool the viewport's gizmo uses |
| World / Local | Whether move and rotate use the world's axes or the selection's own. Scale always uses the selection's own |
| Frame | Moves the camera so the selection fills the view (`F`) |
| Grid snap, with its move, rotate and scale steps | Makes the tools work in steps. Moves land on multiples of the step. Hold `Ctrl` while dragging to flip it for that drag. The inspector's transform fields follow it too |
| Vertex snap (or hold `V`) | See [Snapping](#snapping) |
| Model snap | See [Snapping](#snapping) |
| Fill panel / Game frame | **Fill panel** uses all of the viewport and shows more of the scene when the panel is wide. **Game frame** shows exactly what the game's window shows, with its bars |
| Styled / Sharp | **Styled** draws the scene's own style. **Sharp** draws at full resolution with steady textures, for precise editing. It only changes the view: the scene's style and the game are not affected |
| Grid | Metre lines on the ground plane, drawn over the scene |
| Colliders | Collider wireframes |
| Play / Stop | Runs the game (`F5`) |

## Viewport controls

| Input | Action |
| --- | --- |
| Left click | Select the entity under the cursor; click empty space to clear |
| Drag a gizmo arrow (Move) | Move the selection along X (red), Y (green) or Z (blue) |
| Drag a ring (Rotate) | Turn the selection around that axis |
| Drag a square (Scale) | Scale along the selection's own axis; drag the centre square to scale evenly |
| Hold right mouse button | Look around; `W` `A` `S` `D` `E` `Q` fly while held, `Shift` is faster |
| Mouse wheel | Move forward or back |
| Drag a model from Assets | Places it on what is under the pointer, or on the ground |

A drag is one undo step.

### Snapping

- **Grid snap** works in steps, as above.
- **Vertex snap** lines up two models exactly. Turn it on in the toolbar or
  hold `V`: a square marks the selection's vertex nearest the pointer. Drag it,
  and when the pointer comes near a vertex of another model, the selection
  jumps so that the two vertices meet.
- **Model snap** lines up models as you move them. While dragging a move arrow
  along a world axis, the selection's edges and centre on that axis snap to
  the edges and centres of other models when they come close. The model it
  snapped to is outlined.

These move entities in the editor. The **Vertex snapping** switch in Scene
settings is unrelated: it is the PS1 wobble effect (see
[PS1 and PS2 style settings](style.md)).

## Keyboard shortcuts

| Keys | Action |
| --- | --- |
| `Q` / `W` / `E` | Move / Rotate / Scale tool (while the right mouse button is not held) |
| `F` | Frame the selection |
| Hold `V` | Vertex snap |
| `Ctrl+S` | Save |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` or `Ctrl+Shift+Z` | Redo |
| `Ctrl+D` | Duplicate the selection (with its children) |
| `Delete` | Delete the selection (with its children) |
| `F5` | Play / Stop |

## Menus

- **File**:
  - **New Project…**, **Open Project…**, **Recent Projects** and **Close
    Project** (back to the hub). See [Projects and the project
    hub](projects.md#switching-projects).
  - **New Scene…** creates `scenes/<name>.scene.json`, an empty scene with
    the current scene's style, lighting and camera, and opens it.
    **Open Scene** lists every scene in `scenes/` (the startup scene is
    marked). **Set as Startup Scene** makes the open scene the one the
    game starts with.
  - Save; Reload from disk (discard changes); Quit.

  Anything that leaves the open scene asks first if it has unsaved changes.
- **Edit**: Undo and Redo (the menu names the step), Duplicate and Delete.
- **Create**: Empty, Cube, Plane, Sphere, Quad. New entities appear a few
  metres in front of the editor camera and are selected.
- **Game**: Play/Stop and **Reload behaviour kinds** (rebuilds the game and
  re-reads the behaviours it registers).
- **Build** (each saves unsaved changes first and runs in the background;
  the result opens in its own window with **Copy** and, for exports,
  **Open folder**):
  - **Validate Project**: the checks of `copperframe validate`.
  - **Export folder** (with **Browse…**) and **Export Game**. See
    [Exporting a game](exporting.md).
  - **Run Test Script**: lists the project's `*.script.json` files, plus
    **Choose a script file…** and **Idle run** (5 seconds, no script). See
    [Testing gameplay headless](testing.md).
  - **Advanced > Benchmark Scene**: a release build measured offscreen in
    three styles. See [Benchmarking](../reference/game-options.md#benchmarking).
- **Window**: open or close each panel, and **Reset layout**.
- **Help**: a summary of the controls and **Report a bug…** (see
  [Crash reports and logs](../help/crash-reports.md)).

## The inspector

With an entity selected, the inspector shows, top to bottom:

- **Name** and **Parent**. Changing the parent keeps the entity where it
  is in the world unless you untick *keep world position*.
- **Transform**: position (metres), rotation (Euler degrees, applied as
  yaw Y, then pitch X, then roll Z) and scale.
- **Mesh**: a primitive shape with an optional texture, or an imported
  model. **Material overrides** replace the model's own values:
  *Tint*, *Alpha* (opaque, cutout with a threshold, or blend), and *Lit*,
  *Flat shading*, *Affine UVs* and *Double-sided*, each of which can be
  *source* (keep the model's value), *on* or *off*.
- **Collider**: shape (box, sphere, capsule, or mesh for static bodies),
  body (static, kinematic, dynamic), sensor, offset, layer, mass and
  friction.
- **Audio source**: sound, bus (effects or music), loop, autoplay, 3D,
  volume in dB and the min/max distance.
- **Behaviours**: the list of gameplay behaviours with their fields. The
  fields come from the game's own schema; **Reload** rebuilds the game and
  reads it again.
- **Point light**, **Blob shadow** and **Animator** (clip, speed, loop,
  autoplay; needs a skinned model). The viewport previews autoplaying clips.

Each section has a **+** button to add it and a **Remove** button. Hints
appear when something will not show, for example a light in a scene whose
style has local lights turned off.

See [Scenes, entities and components](scenes.md) for what every field means.

## Scene settings

The **Scene settings** panel holds what applies to the whole scene:

- the style: **PS1**, **PS2** and **Clean** preset buttons (the label shows
  *Custom* once you change something), then every effect as a switch with
  its value. See [PS1 and PS2 style settings](style.md);
- the directional light's direction, colour and the ambient level;
- **Use current view as start camera**, which stores the editor view as
  the scene's start camera: where the editor view opens next time, and the
  game's camera until a player or camera zone takes over.

## The Assets panel

A browser for `assets/`: folders on the left, a grid of tiles for the open
folder on the right (textures show a thumbnail), and a strip along the bottom
with the selected asset's path, any problem with it, and its actions.

- Click a folder, a part of the path above the grid, or double-click a folder
  tile to open it.
- **Search** looks through the open folder and everything below it. **All**,
  **Models**, **Textures** and **Sounds** filter by kind, and the slider sets
  the tile size.
- A dot on a tile marks an asset with a problem: yellow for import warnings,
  red for a failed import or a missing file. The strip says what is wrong.
- New files are registered automatically (see below). **Rescan** does it at
  once, and **Show in folder** opens the folder in Explorer.
- **Add to scene** (models) creates an entity using the model in front of the
  camera. Double-clicking the tile does the same, and dragging the tile into
  the viewport places it where you drop it.
- **Reimport** (models and textures) reads the file again after you changed
  it. If the new import fails, the last good version stays on screen and
  the error appears in the console.

### Automatic import

The editor watches `assets/` while a project is open, checking once a
second:

- **New files** are registered (their `.meta.json` is written) and appear
  in the list.
- **Changed files** that are loaded in the scene are reimported in place,
  so a texture you save in your paint program updates in the viewport. A
  file is picked up once it has stopped changing for a second, so large
  exports are not read half-written.
- **Deleted files** are reported in the Console; entities using them show
  the missing-asset checker until you replace or remove them.

Hidden files and `*.tmp` files are ignored.

## Saving, undo and conflicts

Every edit goes through the same command layer the CLI uses and becomes an
undo step. A continuous drag (gizmo or slider) is one step. Undoing back to
the state you saved clears the unsaved marker.

Saves are atomic: the editor writes a temporary file, keeps the previous
version as `<scene>.bak`, then replaces the scene. Each save increments the
scene's **revision**. If the file on disk has a different revision than the
one the editor loaded (for example `copperframe apply` changed it), Save
stops and offers to overwrite with your version or reload from disk.

The editor also watches `scenes/`. When another program changes the open
scene and you have no unsaved edits, the editor reloads it and says so in
the Console. With unsaved edits it asks: **Reload from disk** or **Keep my
version**.

Closing, or switching scene or project, with unsaved changes asks
**Save**, **Don't save** or **Cancel**.

## Play mode

**Play** (or `F5`) writes the current scene, unsaved edits included, to
`.psx/play/` in the project and runs the game on that copy in its own
window, skipping the title screen. The game's log streams into the
Console. **Stop** ends it. Play mode never writes the authored scene, the
undo history, or the game's saves.

If the project has a `game/` crate, Play runs it with `cargo run` (so the
first Play compiles it). Without one, the stock `copperframe_player` is
used. A project with a `game/` crate needs Rust; if it is not installed,
Play, Build and Export show a prompt with a link to
[rustup.rs](https://rustup.rs) and a **Check again** button.

If the game crashes, the editor shows its crash report. See
[Crash reports and logs](../help/crash-reports.md).

## Project lock

While open, the editor holds `.psx/editor.lock` in the project with an OS
file lock. A second editor, or `copperframe apply`, refuses to edit the
project while it is held. The OS releases the lock when the editor exits or
crashes, so a lock file left behind by a crash is detected and replaced
automatically. If an editor is still running (or hung) with the project
open, the hub says so and offers **Open anyway**, the same as
`--steal-lock`.

## Performance

The editor redraws only when something changes: input, a moving camera or
gizmo, the splash, an animation preview (30 Hz, 10 Hz in the background) or
a running game's output. The status bar says "idle" when it is not redrawing
continuously.
