# Physics

Copperframe simulates rigid bodies with [rapier](https://rapier.rs) at a
fixed 60 ticks per second. Physics runs in Play mode, in the player and in
headless tests; the editor scene itself never moves. Retro visual settings
(vertex snapping, low resolution) never change collision.

## The two physics components

| Entity has | Result |
| --- | --- |
| Collider only | A **static** collider: walls, floors, props that never move |
| Collider + Rigidbody (dynamic) | Falls under gravity, bounces off and slides along other colliders, and can be pushed by the player and by kinematic bodies |
| Collider + Rigidbody (kinematic) | Moves only when its transform changes (a door or lift driven by a behaviour). Pushes dynamic bodies but is never pushed |
| Collider with **sensor** ticked | Blocks nothing; reports overlaps as `on_trigger` |
| Rigidbody only | A dynamic point mass: it falls but touches nothing |

Add them in the inspector with **+ Collider** and **+ Rigidbody**. Turn on
**Colliders** in the toolbar to see the shapes: green is static, orange
dynamic, blue kinematic, yellow sensor.

Rules:

- Rigidbodies go on **root** entities (no parent). Children of a moving
  body are fine and move with it, but their colliders do not.
- **Mesh** colliders are static only. Give a moving model a box, sphere or
  capsule.
- Collider sizes follow the entity's scale when the scene starts.

## Settings

### On the collider

- **Friction** (0–10): how much surfaces grip.
- **Bounciness** (0–1): 0 stops dead, 1 bounces back at full speed. When two
  colliders touch, the bouncier one wins, so a bouncy ball bounces on any
  floor.

### On the rigidbody

- **Mass** in kilograms. Heavier bodies are harder for the player to push
  and push lighter ones aside.
- **Gravity scale**: 1 is normal, 0 floats, negative falls upwards.
- **Linear / angular damping**: slow movement and spinning over time (air
  drag).
- **Lock rotation**: contacts never tip the body over.

### Project gravity

Gravity is a project setting: **Project Settings > Physics** in the editor
(`physics.gravity` in `project.json`, default `[0, -9.81, 0]` m/s²). Game code can change it during play with
`ctx.set_gravity`.

## The player and physics

The `psx.player` character is a capsule that walks, climbs small steps and
slides along walls. It pushes dynamic bodies it walks into, as if it weighed
80 kg (`ctx.set_character_push_mass` changes that). Kinematic doors and lifts
block it.

## Physics in game code

Behaviours get these through `Ctx` (full list in the
[Ctx reference](../reference/ctx.md#physics)):

```rust
impl Behavior for Bouncer {
    // A solid contact involving a dynamic body started or stopped.
    fn on_collision(&mut self, ctx: &mut Ctx, me: Entity, other: Entity, started: bool) {
        if started {
            // Kick it upwards: impulse = mass x change in velocity.
            let kick = Vec3::Y * 5.0 * ctx.mass(me);
            ctx.apply_impulse(me, kick);
        }
    }

    fn fixed_update(&mut self, ctx: &mut Ctx, me: Entity) {
        // Everything within 3 m, sensors excluded.
        let pos = ctx.world_position(me).unwrap_or_default();
        for other in ctx.overlap_sphere(pos, 3.0, false, Some(me)) {
            // A steady push lasts one tick, so add it every tick.
            ctx.add_force(other, Vec3::Y * 20.0);
        }
    }
}
```

- `raycast` and `overlap_*` answer "what is there?".
- `velocity` / `set_velocity`, `apply_impulse` and `add_force` move dynamic
  bodies. Moving a dynamic body with `set_local` teleports it.
- Kinematic bodies are moved by setting their transform (`set_local`) every
  tick; physics follows on the next step.

## Limits

- No joints, ragdolls or vehicles yet.
- Changing an entity's scale while playing does not resize its collider.
