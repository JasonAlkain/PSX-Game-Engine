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

**Project versus scene settings:** Project settings provide shared defaults
(including the retro look and audio mixer). A scene can use the shared
project look or override it; its fog, background, lighting and start camera
remain scene-specific. For scripting, open **Project settings > Scripting**
to enable C#, review Lua/C# scripts, and access script creation. A save
refreshes the editor's script schema; restart Play to run modified code.

## Layout

| Area | What it does |
| --- | --- |
| Menu bar | **File**, **Edit**, **Create**, **Game**, **Build**, **Window** and **Help** |
| Toolbar | The transform tools, snapping, view options, **Play**/**Stop** and the cog for project settings (see [The toolbar](#the-toolbar)) |
| Hierarchy | Every entity in the scene as a tree: search, collapse, drag to reorder and reparent (see [The hierarchy](#the-hierarchy)) |
| Viewport | The scene rendered with its own retro style, with the selection gizmo and optional collider wireframes |
| Inspector | The selected entity's components |
| Scene settings | The scene's style, lighting and start camera |
| Project settings | Settings for the whole game (closed at first; the toolbar's cog opens it). See [Project settings](#project-settings) |
| Assets | The [asset browser](#the-assets-panel) |
| Console | The log, including a running game's output |
| Problems | Script problems: scripts that do not load, and errors from the last Play (closed at first; opens by itself when a problem appears). See [Lua scripting](lua.md#in-the-editor) and [C# scripting](csharp.md#in-the-editor) |
| Status bar | The number of script problems (click to open the Problems panel), project and scene name, revision, unsaved state, frame rate, draw and triangle counts, and "idle" when the editor is not redrawing |

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

## The hierarchy

| Input | Action |
| --- | --- |
| Type in **Search by name…** | Shows only entities whose name contains the text (upper and lower case are the same). Their parents stay visible, dimmed, so you can see where each match sits. **×** or `Esc` clears the search |
| Click the arrow beside a parent | Collapses or expands its children. The editor remembers this for each entity until it closes |
| Click a row | Selects the entity. Selecting in the viewport opens the parents and scrolls the hierarchy to it |
| Drag a row onto the top or bottom edge of another row | Moves the entity before or after it, under the same parent |
| Drag a row onto the middle of another row | Makes it a child of that entity (last among its children) |
| Drag a row into the empty space below the list | Moves it to the end of the scene root |
| Double-click a row, or `F2` | Renames in place. `Enter` or clicking away keeps the name, `Esc` cancels |
| Right-click a row | Rename, Duplicate, Delete, Create child, Move to scene root, Expand all, Collapse all |
| Right-click empty space | Create an empty entity or a primitive |
| `↑` `↓` with the pointer over the panel | Select the row above or below |
| `←` `→` | Collapse or expand the selected entity (`←` on a closed or childless row selects its parent) |

While you drag, a copper line or outline shows where the entity will go.
Red means the drop is not allowed: an entity cannot go under itself or its
own children, and entities with a rigidbody must stay at the scene root.
Moving keeps the entity where it is in the world when the inspector's
**keep world position** is ticked (the default). Every move is one undo
step, and the order you see is the order saved in the scene file.

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
| Pause / Resume, Step | While the game runs: freeze and continue it (`F6`), or advance it by one tick, 1/60 s (`F7`). The label beside them shows the state and, while paused, the tick |

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
| `Ctrl+S` | Save the scene |
| `Ctrl+Shift+S` | Save the scene under a new name (Save Scene As) |
| `Ctrl+N` | New scene |
| `Ctrl+Z` | Undo |
| `Ctrl+Y` or `Ctrl+Shift+Z` | Redo |
| `Ctrl+D` | Duplicate the selection (with its children) |
| `Delete` | Delete the selection (with its children) |
| `F5` | Play / Stop |
| `F6` | Pause / Resume the running game |
| `F7` | Advance the running game by one tick (pauses it first) |

## Menus

- **File**:
  - **New Project…**, **Open Project…**, **Recent Projects** and **Close
    Project** (back to the hub). See [Projects and the project
    hub](projects.md#switching-projects).
  - **New Scene…** (`Ctrl+N`) asks for a name, a folder under `scenes/`
    and a template (**Empty**, or **Basic**: a floor with a collider and a
    player), creates `<folder>/<name>.scene.json` with the open scene's
    look and lighting, and opens it. An existing file is never
    overwritten: the dialog says so and lets you pick another name.
  - **Open Scene** lists every scene in `scenes/` (the start scene is
    marked), and **Browse…** picks a scene file with the system dialog.
    **Recent Scenes** lists the last ten scenes opened in this project.
    **Set as Start Scene** makes the open scene the one the game starts
    with.
  - **Save Scene** (`Ctrl+S`) and **Save Scene As…** (`Ctrl+Shift+S`). Save
    As writes the scene, unsaved edits included, to a new file and keeps
    editing that one; the original stays as it was last saved.
  - **Project Settings…** opens the [Project settings](#project-settings)
    panel.
  - Reload from disk (discard changes); Quit.

  Anything that leaves the open scene asks first if it has unsaved changes,
  and anything that closes the project asks about unsaved project settings.
- **Edit**: Undo and Redo (the menu names the step), Duplicate and Delete.
- **Create**: Empty, Cube, Plane, Sphere, Quad. New entities appear a few
  metres in front of the editor camera and are selected.
- **Game**: Play/Stop, Pause and Step, **Reload behaviour kinds** (rebuilds
  the game and re-reads the behaviours it registers), **New script…**
  (Lua or C#) and **Problems** (see [Lua scripting](lua.md#in-the-editor)).
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

With an entity selected, the inspector shows **Name**, **Parent** and
**Transform** at the top, then one card per component, then the **Add
component** button.

- **Name** and **Parent**. Changing the parent keeps the entity where it
  is in the world unless you untick *keep world position*.
- **Transform**: position (metres), rotation (Euler degrees, applied as
  yaw Y, then pitch X, then roll Z) and scale.

### Component cards

Each component is a card. Its coloured left edge tells you which folder of
the Add component menu it comes from (copper for Rendering, yellow for
Lighting, green for Physics, blue for Audio, violet for Behaviours).

| In the card header | What it does |
| --- | --- |
| Triangle | Folds the card away (remembered per entity) |
| Dotted handle and title | Drag onto another card to move it there; a copper line shows where it will land |
| Up and down arrows | Move the card one place |
| Bin | Removes the component (undo brings it back) |
| Right-click | Move to top, Move up, Move down, Move to bottom, Remove |

The order is saved with the scene. Behaviours also **run** in this order
each frame, so put the one that should act first higher up.

### Add component

**Add component** opens a menu of folders:

| Folder | Contains |
| --- | --- |
| Rendering | **Mesh** (Cube, Sphere, Plane, Quad, or a Model from `assets/`), **Animator** (pick a clip; needs a skinned model) |
| Lighting | **Point light**, **Blob shadow** |
| Physics | **Collider** (sized to fit the mesh) |
| Audio | **Audio source** (pick a sound from `assets/`) |
| Behaviours | The game's behaviours, in the folders the game files them in (see [Behaviours and game code](behaviours.md#folders-in-the-add-component-menu)), plus **Reload behaviour kinds**. Lua and C# scripts are under **Scripts**, with **New script…** |

Type in the search field at the top to list every match instead, with its
folder next to it; `Enter` adds the first match. Entries the entity cannot
take are greyed out, and hovering one says why (it already has one, or
there are no sounds in `assets/` yet). Before the game has been built the
Behaviours folder offers **Load behaviour kinds**, which builds it and reads
the kinds it registers.

### What the cards edit

- **Mesh**: a primitive shape with an optional texture, or an imported
  model. **Tile texture** repeats a primitive's texture by its size in the
  world, so scaling it does not stretch the texture; the value is the metres
  one repeat covers. **Material overrides** replace the model's own values:
  *Tint*, *Alpha* (opaque, cutout with a threshold, or blend), and *Lit*,
  *Flat shading*, *Affine UVs* and *Double-sided*, each of which can be
  *source* (keep the model's value), *on* or *off*.
- **Collider**: shape (box, sphere, capsule, or mesh for static colliders),
  sensor, offset, layer, friction and bounciness.
- **Rigidbody**: makes the entity move under physics. Type (dynamic or
  kinematic), mass, gravity scale, linear and angular damping, and lock
  rotation. Only root entities can have one. Press **Play** to see it
  simulate; the editor scene itself never moves.
- **Audio source**: sound, mixer bus (Music, SFX, Voice or Ambient), loop,
  autoplay, 3D, volume in dB and the min/max distance.
- **Behaviours**: one card per behaviour with its fields, which come from
  the game's own schema.
- **Point light**, **Blob shadow** and **Animator** (clip, speed, loop,
  autoplay). The viewport previews autoplaying clips.

Hints appear when something will not show, for example a light in a scene
whose style has local lights turned off.

See [Scenes, entities and components](scenes.md) for what every field means.

## Scene settings

The **Scene settings** panel holds what applies to the whole scene:

- **Look**: **Project look** uses the console look set once for the whole
  game in Project settings > Look (**Edit the project look…** opens it), so
  every scene that uses it changes together. **This scene's own** gives the
  scene its own **PS1**, **PS2** and **Clean** presets and every effect as a
  switch with its value (see [PS1 and PS2 style settings](style.md)).
  Switching either way keeps what is on screen: the current values are
  copied across. New scenes use the project look; scenes made before 0.8
  keep their own until you switch them;
- **Atmosphere**: fog and background colour, which always belong to the
  scene, whichever look it uses;
- the directional light's direction, colour and the ambient level;
- **Use current view as start camera**, which stores the editor view as
  the scene's start camera: where the editor view opens next time, and the
  game's camera until a player or camera zone takes over.

## Project settings

The **Project settings** panel (the cog at the right of the toolbar, or
**File > Project Settings…**) holds what applies to the whole game rather
than one scene. It is a panel like the others, so it can be docked anywhere;
it starts as a closed tab next to Scene settings.

Edits are a draft: **Save** checks them and writes `project.json` (and
`input.json` when the keys changed), **Revert** drops them, and *unsaved*
shows while there is something to save. Project settings are not part of
the scene's undo history. Play and Export use the saved values.

| Page | Settings |
| --- | --- |
| General | **Name** (hub, status bar, default export folder), **Start scene** (the scene the game opens with; the same as **File > Set as Startup Scene**), and the project folder with **Open** |
| Window | **Window size** of the game in points, with presets (640 × 480 up to 1920 × 1080), and whether a new player starts **fullscreen** and with **vertical sync** |
| Look | **Default look**: *As authored* (each scene's look), *PS1* or *PS2*, and whether **players can switch** between them in the game's settings screen. **Project look**: the console look (presets and every effect) of all scenes set to *Project look*; edits show in the viewport before you save. **Copy from the open scene** takes the open scene's look. **Scenes** lists the scenes that still have their own look, and **Make all use the project look…** switches them (after asking; their fog, background and lighting are kept). Save the project settings first |
| Scripting | Whether the project loads **Lua scripts** and **C# scripts**, the scripts in `scripts/` with their behaviour kinds (or how many problems each has), **Open** for each, **New script…** and **Open folder**. See [Lua scripting](lua.md) and [C# scripting](csharp.md) |
| Input | The keys each action starts with. Click a key to unbind it, **+ key** to add one (a key used by another action moves here, and the panel says so), add your own actions by name for game code to read, or **Reset to engine defaults**. Players can still remap keys in the game |
| Audio | The project mixer's bus volumes, mutes and the master limiter in a table, and **Open the Audio mixer** for faders and meters (see [The Audio mixer](#the-audio-mixer)) |
| Build | The **Export folder** (relative to the project or a full path; empty means `dist/<name>`), whether to **check the exported game starts**, and **Check with** an idle run or one of the project's test scripts |

The game's low internal resolution belongs to each scene's style, so it is
in [Scene settings](#scene-settings), not here.

## The Audio mixer

**Window > Audio mixer** (or **Open the Audio mixer** on the Audio page of
Project settings) opens a strip per bus: **Master**, **Music**, **SFX**,
**Voice** and **Ambient**. Every Audio source plays through the bus picked
in its inspector, and every bus feeds Master.

- The fader and the dB box set the bus volume (−60 to +12 dB). **M** mutes
  the bus. Both are saved in `project.json` and used by Play and Export.
- **S** solos a bus: previews then sound only on soloed buses. Solo is for
  listening in the editor and is not saved.
- The meter shows the peak level of sound previews, turning yellow above
  −6 dB and red at 0 dB, with a line holding the recent peak. The light on
  top turns red when a bus clipped; click the meter to clear it.
- **Limiter** on Master pulls peaks down before they clip (a fast
  compressor at −1 dBFS).

The mixer edits the same draft as Project settings, so **Save** in either
writes both, and the "unsaved changes" prompt covers it. A game started
with Play reads the mixer when it starts; changes made while it runs apply
the next time you press Play. Players' music and effects volume sliders
multiply on top of the mixer (the effects slider also covers Voice and
Ambient).

## The Assets panel

A browser for the project's three folders: `assets/` (models, textures,
sounds, and text or data files), `scenes/` and `scripts/` (Lua and C#
scripts). Folders are on the left, a grid of tiles for the open folder on
the right (textures show a thumbnail), and a strip along the bottom with the
selected item's path, any problem with it, and its actions.

- Click a folder, a part of the path above the grid, or double-click a folder
  tile to open it.
- **Search** looks through the open folder and everything below it. **All**,
  **Models**, **Textures** and **Sounds** filter by kind; **Scenes** and
  **Scripts** list every scene or script in the project. The slider sets
  the tile size.
- A dot on a tile marks an asset with a problem: yellow for import warnings,
  red for a failed import or a missing file. The strip says what is wrong.
- New files are registered automatically (see below). **Rescan** does it at
  once, and **Show in folder** opens the folder in Explorer.
- **Add to scene** (models) creates an entity using the model in front of the
  camera. Double-clicking the tile does the same, and dragging the tile into
  the viewport places it where you drop it.
- **Sounds** preview in the editor: double-click a sound tile to play it
  (again to stop), or select it and use the strip's **play/pause**,
  **stop**, position slider (drag to seek), time, preview **volume** and the
  **bus** it plays through. A playing sound's tile shows a copper play mark.
  Previews use the editor's own audio device and never reach a running
  game.
- **Reimport** (models and textures) reads the file again after you changed
  it. If the new import fails, the last good version stays on screen and
  the error appears in the console.
- **Open** (right-click, or the strip) opens a file in the program your
  system uses for it: your text editor for scripts and data files, your
  image viewer or paint program for textures.

### Creating things

The panel's **Create** button, and a right-click on empty space, a tile or
a folder, offer:

| Item | Where it goes |
| --- | --- |
| **Folder…** | The open folder (in any of the three) |
| **Scene…** | The open folder if it is in `scenes/`, else `scenes/` |
| **Lua script…**, **C# script…** | The open folder if it is in `scripts/`, else `scripts/`. Opens the New script dialog; the language must be on in Project settings > Scripting |
| **Text file…**, **JSON data file…** | The open folder if it is in `assets/`, else `assets/`. An empty `.txt` file, or a `.json` file holding `{}` |

New scripts and data files open in your text editor straight away.
Everything in `assets/` is copied into exported games, data files included.
Copperframe has no material files: a model's materials come from the model,
and an entity's changes to them are set on its Mesh component.

### Deleting

**Delete…** (right-click a tile or a folder, or the strip) moves the file
or folder to `.psx/deleted/` in the project folder, so it can be recovered
by hand. Before it does, the dialog says what else is affected:

- an **asset**: the scenes that use it. Its `.meta.json` goes with it, so
  moving both back restores it with the same ID;
- a **script**: the scenes whose entities use its behaviour. Those scenes do
  not play until the script is back or the components are removed;
- a **folder**: how many files go with it. A folder holding the open scene
  or the start scene cannot be deleted, and neither can `assets`, `scenes`
  or `scripts` themselves.

### Scenes in the Assets panel

Scene files have their own **scenes** folder in the tree (and the
**Scenes** filter lists every scene in the project, searching all of them by
name). The open scene's tile is copper; the start scene is marked
**start**. The Hierarchy's header shows the open scene's name and
**\* unsaved** while it has unsaved changes.

- **Double-click** a scene to open it (you are asked about unsaved
  changes first).
- **Right-click** a scene for Open, Rename…, Delete…, Set as Start Scene and
  Show in folder, and right-click a scenes folder or empty space for
  **Scene…** and **Folder…** (also on the panel's **Create** button).
- **Drag** a scene onto a scenes folder (in the tree or the grid) to move it.
- Renaming and moving keep references working: the project's start scene,
  the recent list, and behaviour parameters that hold the old path (in every
  scene; in the open scene that change is an undoable edit, saved with the
  scene).
- **Delete** moves the file to `.psx/deleted/` in the project folder, so it
  can be recovered by hand. The open scene and the start scene cannot be
  deleted.

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

`scenes/` and `scripts/` are watched the same way: a scene changed by
another program is reloaded (or, with unsaved edits, the editor asks), and
a saved `.lua` script (or `.cs` script with C# on) makes the editor read the behaviour kinds again.

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
Console. Play mode never writes the authored scene, the undo history, or
the game's saves.

While the game runs, the toolbar (and the **Game** menu) has:

- **Pause** (`F6`): the simulation stops; the game window keeps drawing
  the frozen frame and shows *Paused by the editor · tick N*. Sound pauses
  and the mouse is released.
- **Resume** (`F6` again): it continues at normal speed.
- **Step** (`F7`): it advances by exactly one fixed tick (1/60 s) and stays
  paused. Stepping uses the same tick as normal play and headless tests, so
  a game stepped 30 times is in the same state as one that ran 30 ticks
  with the same input. Keys held in the game window apply to each step.
- **Stop** (`F5`): the game is asked to close, so scene unload and
  `Game::shutdown` run, and is ended if it has not closed within 3 seconds.
  Stop while the game is still compiling ends the build.

Pause and Step become available once the game has loaded (the label reads
*Starting…* until then). If the editor exits or crashes, the game closes
by itself.

A script error while the game runs appears in the **Problems** panel with
the failing lines; the game keeps running without that behaviour. Problems
from a run stay listed until the next Play.

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
