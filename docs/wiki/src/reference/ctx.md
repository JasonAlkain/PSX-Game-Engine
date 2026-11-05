# Game API (Ctx) reference

`Ctx` is passed to every `Game` and `Behavior` method. It is the only way
gameplay code changes the world, which is what keeps windowed and headless
runs identical. The full rustdoc is available with:

```sh
cargo doc -p copperframe_game --open
```

Types used below: `Entity` is a runtime handle (`hecs::Entity`), valid
until despawned. `EntityId` is the persistent UUID stored in the scene.
`Transform` is `copperframe_scene::Transform` (translation `Vec3`, rotation
`Quat`, scale `Vec3`). Vectors are `glam` 0.34.

## Time and input

| Method | Returns |
| --- | --- |
| `input()` | `&ActionState`: `held(action)`, `pressed(action)` (first tick only), `axis(negative, positive)` (−1, 0 or 1), and `look` (`Vec2` degrees this tick; +x right, +y up) |
| `dt()` | Seconds per tick (1/60) |
| `tick()` | The current tick number |

## Entities

| Method | Effect |
| --- | --- |
| `find(id: EntityId) -> Option<Entity>` | Handle for a persistent ID |
| `find_by_name(name) -> Option<Entity>` | An entity with this name (if several share it, any one of them) |
| `id_of(e) -> Option<EntityId>` | Persistent ID of a handle |
| `name(e) -> String` | Entity name |
| `exists(e) -> bool` | Whether the handle is still alive |
| `local(e) -> Option<Transform>` | Local transform |
| `set_local(e, t)` | Set the local transform. For kinematic bodies this is the next physics target; dynamic bodies are teleported |
| `world_matrix(e) -> Option<Mat4>` | World matrix as of the last transform update |
| `world_position(e) -> Option<Vec3>` | World position |
| `parent(e) -> Option<Entity>` | Parent |
| `set_hidden(e, hidden)` | Stop or resume drawing the entity |
| `spawn(entity: SceneEntity) -> EntityId` | Create an entity (with components) after this phase. A nil or duplicate ID is replaced; the ID used is returned |
| `instantiate(template: EntityId, transform) -> Option<EntityId>` | Copy an authored entity and its children with fresh IDs at `transform`; references inside the copy are remapped. Good for prefabs: keep a hidden template in the scene |
| `despawn(e)` | Remove the entity after this phase |
| `world()`, `world_mut()` | Direct `hecs` access for your own components. Change engine components through the methods above |

## Physics

| Method | Effect |
| --- | --- |
| `raycast(origin, direction, max_distance, ignore: Option<Entity>) -> Option<RayHit>` | First solid collider along the ray. `RayHit` has `entity`, `distance`, `point`, `normal` |
| `add_character(e, feet: Vec3, radius, height)` | Give `e` a character capsule (for custom controllers; `psx.player` does this itself) |
| `move_character(e, desired: Vec3) -> Option<(Vec3, bool)>` | Move the capsule by `desired` metres with collision and sliding. Returns the new feet position and whether it is on the ground |
| `aim(by, origin, direction, max_distance)` | Ask the engine to resolve interaction along this ray this tick (custom player controllers) |
| `overlap_sphere(center, radius, include_sensors, ignore) -> Vec<Entity>` | Entities whose colliders overlap a sphere |
| `overlap_box(center, half_extents, rotation, include_sensors, ignore) -> Vec<Entity>` | Entities whose colliders overlap a box |
| `overlap(shape: QueryShape, center, rotation, include_sensors, ignore)` | Same with a `QueryShape` (`Sphere`, `Box`, `Capsule`) |
| `velocity(e) -> Vec3`, `set_velocity(e, v)` | Linear velocity of a rigidbody (m/s) |
| `angular_velocity(e)`, `set_angular_velocity(e, v)` | Spin in radians per second about each world axis |
| `apply_impulse(e, impulse)` | Kick a dynamic rigidbody: its velocity changes by `impulse / mass` at once (N·s) |
| `apply_torque_impulse(e, impulse)` | Spin a dynamic rigidbody at once |
| `add_force(e, force)` | Push a dynamic rigidbody (newtons) for this tick only; call it every tick for a steady push |
| `mass(e) -> f32`, `is_sleeping(e) -> bool` | Mass of a dynamic rigidbody; whether it has come to rest |
| `gravity()`, `set_gravity(g)` | World gravity; starts at the project setting |
| `set_character_push_mass(kg)` | How hard the player pushes dynamic bodies (default 80 kg; 0 stops pushing) |

Sensor overlaps arrive as `Behavior::on_trigger`, solid contacts of
dynamic bodies as `Behavior::on_collision`.

## Camera and HUD

| Method | Effect |
| --- | --- |
| `camera() -> CameraState` | Current camera: `position`, `yaw`, `pitch` (radians), `fov_deg` |
| `set_camera(c)` | Set the camera |
| `set_camera_override(Some(e))` | View from entity `e` (looking along its −Z) until cleared with `None` |
| `camera_override() -> Option<Entity>` | The current override |
| `show_message(text, seconds)` | Show a line of text on the HUD (also logged as a `message` event) |

## Audio

Each works on the entity's audio source component.

| Method | Effect |
| --- | --- |
| `play_sound(e)` | Start the sound |
| `stop_sound(e, fade_secs)` | Stop, with a fade |
| `is_sound_playing(e) -> bool` | |

## Animation

Each works on the entity's animator (a skinned model with clips).

| Method | Effect |
| --- | --- |
| `play_animation(e, clip, fade_secs, looping) -> bool` | Crossfade from the current pose to `clip`. Returns `false` (and changes nothing) if `e` has no animator or no such clip |
| `play_animation_at(e, clip, fade_secs, looping, speed) -> bool` | Same with a speed (−10 to 10; negative plays backwards) |
| `stop_animation(e, fade_secs)` | Fade back to the rest pose |
| `animation(e) -> Option<(String, f32)>` | Current clip and its time in seconds |
| `is_animation_playing(e) -> bool` | `true` until a non-looping clip reaches its end |
| `animation_clips(e) -> Vec<String>` | Clip names of the model |

A non-looping clip that ends logs an `animation_finished` event once.

## Messages, scenes and flow

| Method | Effect |
| --- | --- |
| `send(to, from: Option<Entity>, message)` | Deliver `message` to every behaviour on `to` (`on_message`) after this phase |
| `load_scene(path)` | Unload the current scene and load `path` (project-relative) at the end of the tick. Game state is kept |
| `finish(message)` | End the run successfully; the player shows `message` on the finished screen and the save is deleted |
| `quit()` | End the run |

Message chains are processed up to a bound per tick, to stop two
behaviours ping-ponging forever.

## State and events

| Method | Effect |
| --- | --- |
| `state() -> &GameState`, `state_mut()` | The game state: a `BTreeMap<String, serde_json::Value>` that saves contain |
| `log(event: GameEvent)` | Record an event for logs, the editor console and headless reports. Use `GameEvent::Custom { name, detail }` for your own milestones |

## Constants

`copperframe_game::TICK_HZ` (60) and `TICK_DT` (1/60 s).
