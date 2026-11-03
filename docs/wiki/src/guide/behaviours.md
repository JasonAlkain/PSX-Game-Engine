# Behaviours and game code

Gameplay is written in Rust. A project's `game/` folder is a normal Rust
binary crate that depends on `copperframe_game`. It registers **behaviour
kinds**; scenes attach them to entities by name with parameters; the engine
creates one behaviour instance per entity when the scene loads.

There is no scripting language: behaviours are compiled Rust, so they are
fast, type-checked and debuggable with normal Rust tools. The editor and the
CLI never link your code. They build and run your game executable and ask
it what it registers (`--schema`), so a broken game crate can never crash
the editor.

## The game crate

`copperframe new` writes this layout:

```rust
// game/src/main.rs
fn main() -> std::process::ExitCode {
    // During development the project is the folder above this crate; an
    // exported build uses the `data` folder next to the executable.
    copperframe_game::main(
        || Box::new(my_game::MyGame),
        concat!(env!("CARGO_MANIFEST_DIR"), "/.."),
    )
}
```

`copperframe_game::main` gives every game the same window, menus, settings,
saves and [command-line options](../reference/game-options.md). Your code
lives in `lib.rs`.

## The `Game` trait

One per game. Only `title` and `register` are required.

```rust
use copperframe_game::{Behavior, BehaviorDescriptor, Ctx, Entity, FieldDescriptor, Game, Registry};

pub struct MyGame;

impl Game for MyGame {
    /// Shown on the title screen and in the window title. Also names the
    /// folder where settings and saves are kept.
    fn title(&self) -> String {
        "My Game".into()
    }

    /// Register every behaviour kind scenes may use.
    fn register(&self, r: &mut Registry) {
        r.register(BehaviorDescriptor {
            kind: "game.bob",
            description: "Bobs the entity up and down.",
            fields: vec![
                FieldDescriptor::number("height", 0.25, "distance (m)"),
                FieldDescriptor::number("speed", 1.0, "cycles per second"),
            ],
            create: |p| {
                Ok(Box::new(Bob {
                    height: p.number("height"),
                    speed: p.number("speed"),
                    base: None,
                    time: 0.0,
                }))
            },
        });
    }

    /// Optional: after each scene load.
    fn scene_started(&mut self, ctx: &mut Ctx) {
        ctx.show_message("WASD to move, mouse to look, Esc to pause", 4.0);
    }

    // Optional: fn fixed_update(&mut self, ctx: &mut Ctx) — every tick, before behaviours.
    // Optional: fn shutdown(&mut self, ctx: &mut Ctx) — when the game closes.
}
```

The built-in `psx.*` behaviours are always registered as well.

## Behaviour kinds and fields

A `BehaviorDescriptor` has:

| Field | Meaning |
| --- | --- |
| `kind` | The name scenes use. Prefix your own with your game's name (`room.door`); `psx.` is reserved for the engine |
| `description` | Shown in the editor and in `copperframe schema --project` |
| `fields` | Parameters, each a `FieldDescriptor` with a name, a default and a help text |
| `create` | `fn(&Params) -> Result<Box<dyn Behavior>, String>`. Return `Err` to reject bad values |

Parameters are scalars so the editor can show them:

| Constructor | Editor widget | Read with |
| --- | --- | --- |
| `FieldDescriptor::number(name, default, help)` | number box | `p.number(name)` → `f32` |
| `FieldDescriptor::bool(name, default, help)` | checkbox | `p.bool(name)` |
| `FieldDescriptor::text(name, default, help)` | text box | `p.text(name)` → `&str` |
| `FieldDescriptor::entity(name, help)` | entity picker | `p.entity(name)` → `Option<EntityId>` |

Missing parameters get their defaults. An unregistered kind, an unknown
parameter name, a wrong type or an `Err` from `create` skips that behaviour
and adds a warning naming the entity. Warnings appear in the game's log
(the editor's Console during Play) and in the `warnings` list of a
`copperframe test-scene` report.

After changing fields, use **Game > Reload behaviour kinds** in the editor
(or just press Play) so the inspector picks them up.

## The `Behavior` trait

Every method is optional.

```rust
pub trait Behavior: 'static {
    fn start(&mut self, ctx: &mut Ctx, me: Entity) {}
    fn fixed_update(&mut self, ctx: &mut Ctx, me: Entity) {}
    fn on_trigger(&mut self, ctx: &mut Ctx, me: Entity, other: Entity, entered: bool) {}
    fn on_interact(&mut self, ctx: &mut Ctx, me: Entity, by: Entity) {}
    fn interaction_prompt(&self) -> Option<String> { None }
    fn on_message(&mut self, ctx: &mut Ctx, me: Entity, message: &str, from: Option<Entity>) {}
    fn on_destroy(&mut self, ctx: &mut Ctx, me: Entity) {}
}
```

| Method | When it runs |
| --- | --- |
| `start` | Once, on the first tick after the entity appears |
| `fixed_update` | Every simulation tick (60 per second; `ctx.dt()` is 1/60 s) |
| `on_trigger` | A **sensor** collider on `me` started (`entered = true`) or stopped overlapping `other` |
| `interaction_prompt` | While the player aims at `me` (or a child of `me`). Return the prompt text ("Open door") to make it interactive, or `None` |
| `on_interact` | The player pressed the interact action while a prompt was shown |
| `on_message` | Another behaviour called `ctx.send(me, …)` |
| `on_destroy` | `me` is despawned or its scene unloads |

`me` is a runtime handle (`Entity`), valid until the entity is despawned.
Convert between handles and persistent IDs with `ctx.find(id)` and
`ctx.id_of(entity)`.

For an entity to be interactive, the player's aim ray must hit a solid
(non-sensor) collider on it or on one of its children, within the player's
`reach`.

## An example: a lever that opens a door

From *The Lever Room* (`examples/room/game/src/lib.rs`), simplified:

```rust
struct Lever {
    target: Option<copperframe_core::EntityId>,
    pulled: bool,
}

impl Behavior for Lever {
    fn interaction_prompt(&self) -> Option<String> {
        (!self.pulled).then(|| "Pull lever".to_owned())
    }

    fn on_interact(&mut self, ctx: &mut Ctx, me: Entity, _by: Entity) {
        self.pulled = true;
        ctx.play_sound(me);
        ctx.log(GameEvent::Custom { name: "lever_pulled".into(), detail: ctx.name(me) });
        match self.target.and_then(|id| ctx.find(id)) {
            Some(door) => ctx.send(door, Some(me), "open"),
            None => ctx.show_message("The lever moves, but nothing happens.", 3.0),
        }
    }
}

struct Door { /* … */ }

impl Behavior for Door {
    fn on_message(&mut self, ctx: &mut Ctx, me: Entity, message: &str, _from: Option<Entity>) {
        if message == "open" {
            self.opening = true;
            ctx.play_sound(me);
            // Kept in saves, so a continued game starts with the door open.
            ctx.state_mut().insert("door_open".into(), true.into());
        }
    }

    fn fixed_update(&mut self, ctx: &mut Ctx, me: Entity) {
        // Slide a little each tick with ctx.local(me) / ctx.set_local(me, t).
    }
}
```

In the scene, the lever entity has `{"kind": "room.lever", "params":
{"target": "<door entity id>"}}`; in the editor you pick the door from the
`target` field's entity list.

## The tick

Each 60 Hz tick runs in a fixed order:

1. `start` for behaviours created since the last tick.
2. `Game::fixed_update`, then every behaviour's `fixed_update`.
3. Interaction: the aim ray picks the nearest interactive entity within
   reach; the interact action calls its `on_interact`.
4. Deferred commands (messages, spawns, despawns).
5. World transforms and kinematic body targets are updated.
6. Physics step.
7. Trigger events (`on_trigger`) and their commands.
8. Animation, final transforms, audio emitters and the listener.

Spawning, despawning and messages are deferred to the end of the current
phase, so iterating the world is always safe.

## Rules that keep games testable

- **Change the world only through `Ctx`.** The simulation never writes the
  authored scene, and the same input produces the same result windowed and
  headless. That is what makes [headless tests](testing.md) reliable.
- **Read actions, not keys:** `ctx.input().held("move_forward")`,
  `pressed("interact")`. See [Input](player.md#input).
- **Keep persistent progress in `ctx.state_mut()`**, a string-to-JSON map.
  It is what a save contains; behaviour fields are not saved.
- **Log milestones** with `ctx.log(GameEvent::Custom { name, detail })` so
  scripts can expect them.

## Built-in behaviours

| Kind | What it does | Parameters (default) |
| --- | --- | --- |
| `psx.player` | First-person player: walks a character capsule with the move/run/jump actions, looks with the mouse, owns the camera and aims interactions. The entity's position is the feet; its yaw is the starting view direction | `speed` (2.5 m/s), `run_speed` (4.5), `jump_speed` (0 = no jumping), `eye_height` (1.6 m), `radius` (0.3), `height` (1.75), `reach` (2.0 m), `fov` (60°) |
| `psx.camera_zone` | Fixed camera: while the player is inside this entity's sensor collider, the view comes from the `camera` entity, looking along its −Z | `camera` (entity; empty = this entity) |
| `psx.spin` | Rotates the entity about its local Y axis | `degrees_per_second` (45) |

The player capsule stops against static and moving kinematic bodies, so
sliding doors and lifts work without extra code.

## What `Ctx` can do

`Ctx` is the only handle behaviours get. The [Ctx reference](../reference/ctx.md)
lists every method; in short:

- **Entities:** find by ID or name, read and set local transforms, world
  position, parent, hide, spawn, copy an authored entity (`instantiate`),
  despawn.
- **Physics:** ray casts; a character capsule for custom controllers.
- **Camera and HUD:** read and set the camera, override it with an entity,
  show a message.
- **Audio and animation:** play, stop and fade the entity's sound; play and
  crossfade animation clips.
- **Flow:** send messages, load another scene, finish the game, quit.
- **State and events:** the saved game state map and the event log.
- **Escape hatch:** `world()`/`world_mut()` give the `hecs` world for your
  own components.

## Using other crates

The game crate is its own Cargo workspace. Add any crate you like to
`game/Cargo.toml`. Keep `glam` on the engine's version (0.34) because it
appears in the API.
