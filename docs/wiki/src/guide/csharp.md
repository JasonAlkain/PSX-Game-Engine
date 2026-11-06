# C# scripting

Behaviours can be written in C# as well as Rust and [Lua](lua.md). A C#
behaviour is a class in the project's `scripts/` folder. The game compiles
the scripts with the .NET SDK, loads them into the .NET runtime inside the
game process, and each class becomes a behaviour kind like any other.
Scenes attach it to entities, the inspector shows its fields, and saves and
headless tests treat it the same way.

C# scripts use the same `ctx` API as Lua scripts, with C# names
(`ctx.SetPosition` for `ctx:set_position`). Rust, Lua and C# behaviours can
be on the same entity and send each other messages.

## What you need

- **To make the game:** the [.NET SDK](https://dotnet.microsoft.com/download)
  10.0 or newer. The game runs `dotnet build` when a script changed, so the
  first Play after an edit takes a few seconds longer (about 25 seconds the
  very first time, a second or two after that).
- **To play an exported game:** the .NET 10 runtime. Players who do not
  have it get a message that says where to download it. An export contains
  the compiled scripts, not their sources.

Lua needs nothing installed, so pick Lua if your players should not have to
install .NET.

## Turning it on

C# is off for every project unless `project.json` turns it on:

```json
"scripting": { "lua": true, "csharp": true }
```

Or tick **C# scripts** in **Project settings > Scripting** and save.
(`project.json` version 6. Older projects load with C# off.)

## Writing a behaviour

`scripts/Spinner.cs`:

```csharp
using Copperframe;

[Behaviour("Turns the entity about its Y axis", Folder = "Motion")]
public class Spinner : Behaviour
{
    [Help("degrees per second")] public float Speed = 90;
    public bool Running = true;

    public override void FixedUpdate(Ctx ctx)
    {
        if (Running) ctx.Rotate(Entity, Speed * ctx.Dt());
    }
}
```

Every public, non-abstract class deriving from `Behaviour` is a behaviour.
Its kind is `cs.` followed by the class name in snake_case: `Spinner` is
`cs.spinner`, `DoorOpener` is `cs.door_opener`. Set
`[Behaviour(Name = "door")]` to choose another name (a-z, 0-9 and `_`).
A class needs a public constructor without parameters (C# gives it one when
you write none).

The `Behaviour` attribute is optional:

| Property | Does |
| --- | --- |
| `Description` (or the first argument) | Shown in Add component and on the card |
| `Folder` | Files it under **Behaviours > Scripts > *Folder*** in Add component |
| `Name` | Overrides the kind (`cs.<Name>`) |

Any number of classes can be in one file, and files can be in sub-folders
of `scripts/`. All of them are compiled together, so classes can use each
other and shared helper classes.

### Fields

Public instance fields of these types are fields in the inspector. Their
names are snake_case there (`Speed` is `speed`), and their initial values
are the defaults:

| C# field | Inspector field |
| --- | --- |
| `float`, `double`, `int` | number (an `int` is rounded) |
| `bool` | true/false |
| `string` | text |
| `EntityRef` | an entity of the same scene; `ctx.Find(field)` gives the entity, or `null` when it is not set |

`[Help("...")]` adds a tooltip; `[Hide]` keeps a public field out of the
inspector. Fields of other types, properties and `readonly` fields are
ignored. The values set in the scene are in the fields before `Start`.

### Callbacks

Override the ones you need. Each entity gets its own instance, so fields
and anything else you store in the object are per entity. `Entity` is this
entity.

| Method | When |
| --- | --- |
| `Start(Ctx ctx)` | Once, on the first tick after the entity appears |
| `FixedUpdate(Ctx ctx)` | Every fixed tick (60 per second) |
| `OnTrigger(Ctx ctx, Entity other, bool entered)` | A sensor collider on this entity started or stopped overlapping `other` |
| `OnCollision(Ctx ctx, Entity other, bool started)` | A solid collider on this entity started or stopped touching `other` |
| `OnInteract(Ctx ctx, Entity by)` | The player used the interact action while aiming at this entity |
| `OnMessage(Ctx ctx, string message, Entity? from)` | Another behaviour sent this entity a message |
| `OnDestroy(Ctx ctx)` | The entity is despawned or its scene unloaded |

**Interaction:** set `Prompt = "Open door"` to make the entity interactive;
set it to `null` to stop.

A lever that opens a door:

```csharp
using System.Numerics;
using Copperframe;

public class Lever : Behaviour
{
    [Help("door to open")] public EntityRef Door;

    public override void Start(Ctx ctx) => Prompt = "Pull lever";

    public override void OnInteract(Ctx ctx, Entity by)
    {
        if (ctx.Find(Door) is Entity door) ctx.Send(door, "open");
        Prompt = null;
    }
}

public class Door : Behaviour
{
    public override void OnMessage(Ctx ctx, string message, Entity? from)
    {
        if (message != "open") return;
        ctx.SetPosition(Entity, ctx.Position(Entity) + new Vector3(0, 3, 0));
        ctx.PlaySound(Entity);
        ctx.Event("door_opened");
    }
}
```

## The `ctx` API

The methods are those of the [Lua `ctx` API](lua.md#the-ctx-api) in
PascalCase, with C# types: positions and vectors are
`System.Numerics.Vector3`, angles are degrees (`Rotation` returns yaw,
pitch, roll as X, Y, Z), entities are `Entity` values and a missing one is
`null` (`Entity?`). A few differ in shape:

| C# | Lua |
| --- | --- |
| `ctx.Find(EntityRef field)` / `ctx.Find(string id)` | `ctx:find(id)` |
| `ctx.Raycast(origin, direction, max, ignore)` returns `RaycastHit?` (`Entity`, `Distance`) | `ctx:raycast(...)` returns `entity, distance` |
| `ctx.GetState(key)` returns `bool`, `long`, `double`, `string` or `null` | `ctx:get_state(key)` |
| `ctx.SetState(key, value)` takes `bool`, `int`, `long`, `float`, `double`, `string` or `null` | `ctx:set_state(key, value)` |

Your IDE shows every method with its documentation once it knows the API:
open `.psx/csharp/CopperframeScripts.csproj` (written by the first build)
in Visual Studio, Rider or VS Code with the C# extension.

`ctx` is valid only during the callback it is passed to; do not keep it.
A method that is given an entity that no longer exists throws a
`CtxException` (check with `ctx.Exists(e)` first if it may be gone).

## In the editor

With C# on, the [Lua tools](lua.md#in-the-editor) work for C# too:

- **New script…** (in **Game**, in **Add component > Behaviours > Scripts**
  and on the Scripting page) has a **Lua / C#** choice. For C#, type a class
  name such as `DoorOpener` or `Puzzles/DoorOpener`; it writes
  `scripts/Puzzles/DoorOpener.cs` from a template (filed under the
  `Puzzles` folder), adds `cs.door_opener` to the selected entity and opens
  the file.
- A behaviour card of a C# kind shows its file and **Edit script** when the
  class is named after its file.
- When you save a `.cs` file, the editor builds the scripts again and
  updates the inspector. Compiler errors appear in the **Problems** panel
  with the file and line.
- **Project settings > Scripting** lists every script with its behaviour
  kind.

## Errors

A compiler error is reported with its file, line and code, and no C#
behaviour loads until the scripts build again:

```text
scripts/Door.cs:12: CS0103: The name 'positon' does not exist in the current context
```

An exception thrown in a callback never stops the game: that behaviour is
switched off for the rest of the run, the rest of the game keeps running,
and the problem is reported once with the file and line of your script
(from the build's debug information), the entity and the callback:

```text
scripts/Door.cs:14: NullReferenceException: Object reference not set to an instance of an object. (in OnMessage on "Door"; behaviour cs.door switched off)
```

The report goes to the log, the editor's Problems panel and the corner of
the game window. [Headless runs](testing.md) fail on it, like on a Lua
error.

## What to know

- C# scripts are trusted code, like the Rust game code: they are not
  sandboxed and can use all of .NET (files, network, threads). An endless
  loop in a callback hangs the game; Lua stops one, C# cannot.
- Call `ctx` only from the callback's thread.
- Builds go to `.psx/csharp/` in the project (generated; leave it out of
  version control, it has its own `.gitignore`). The newest three builds
  are kept. Do not edit the files there.
- The .NET runtime cannot unload scripts, so a running game keeps the
  scripts it started with. Play again (or restart the game) after editing.
