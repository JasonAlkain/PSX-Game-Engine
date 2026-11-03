# Command batches

Every scene edit, whether from the editor or the CLI, is a **command**. A
**batch** is a JSON file with a list of commands that `copperframe apply`
runs against a scene, all or nothing. Batches let you build or change
scenes from scripts, generators or AI agents, and review the change as a
text diff.

```json
{
  "format": "psx_command_batch",
  "version": 1,
  "expected_revision": 3,
  "commands": [
    {
      "op": "create_entity",
      "name": "Crate",
      "transform": { "translation": [1, 0.5, -2] },
      "mesh": { "source": { "kind": "primitive", "shape": "cube" } }
    },
    { "op": "rename", "id": "7ee7a3ed-3d4f-4689-894e-4697a589acdc", "name": "Spinning cube" }
  ]
}
```

```sh
copperframe apply --project my_game batch.json            # startup scene
copperframe apply --project my_game --scene scenes/level2.scene.json batch.json
copperframe apply --project my_game --dry-run batch.json  # check only
generate_level | copperframe apply --project my_game -    # from stdin
```

## Guarantees

- The batch runs on a copy. If any command fails, or the result is not a
  valid scene, nothing is written and the error names the failing command's
  index.
- On success the scene's `revision` increases by one and the report lists
  the IDs of created entities.
- `expected_revision` (optional) makes the batch refuse (exit 3) if the
  scene changed since you read it.
- `apply` refuses (exit 3) while the editor has the project open.
- Commands never reorder entities they do not touch; new entities are
  appended.

## Operations

`copperframe schema` prints this list for the build you have.

| `op` | Fields | Effect |
| --- | --- | --- |
| `create_entity` | `id?`, `name`, `parent?`, `transform?`, `mesh?` | Creates an entity (a fresh ID if `id` is omitted) |
| `delete_entity` | `id` | Deletes the entity and its descendants |
| `duplicate_entity` | `id`, `new_id?` | Copies the subtree with fresh IDs |
| `rename` | `id`, `name` | |
| `set_transform` | `id`, `translation?`, `rotation?` or `rotation_euler_deg?`, `scale?` | Sets the given parts of the local transform. `rotation` is a quaternion `[x, y, z, w]`; `rotation_euler_deg` is `[x, y, z]` degrees |
| `set_parent` | `id`, `parent` (`null` = root), `keep_world_transform` (default `true`) | Reparents |
| `set_mesh` | `id`, `mesh` (`null` removes) | |
| `set_material` | `id`, `material` | Material overrides |
| `set_collider` | `id`, `collider` (`null` removes): `{shape, offset?, body?, sensor?, layer?, mask?, mass?, friction?}` | |
| `set_audio` | `id`, `audio` (`null` removes): `{sound, bus?, volume_db?, looping?, autoplay?, spatial?, min_distance?, max_distance?}` | |
| `set_behaviors` | `id`, `behaviors`: `[{kind, params?}]` | Replaces the whole list |
| `set_settings` | `style?`, `lighting?`, `camera?` | Scene settings |
| `set_light` | `id`, `light` (`null` removes): `{color?, intensity?, range?}` | |
| `set_animator` | `id`, `animator` (`null` removes): `{clip, speed?, looping?, autoplay?}` | |
| `set_shadow` | `id`, `shadow` (`null` removes): `{radius?, offset?}` | |

Field meanings and ranges are in
[Scenes, entities and components](../guide/scenes.md).

## Value shapes

```js
// create_entity: rotation is a quaternion [x, y, z, w]
"transform": { "translation": [0, 1, 0], "rotation": [0, 0.7071068, 0, 0.7071068], "scale": [1, 1, 1] }

// set_transform also accepts Euler degrees (applied Y, then X, then Z)
{ "op": "set_transform", "id": "<entity id>", "rotation_euler_deg": [0, 90, 0] }

"mesh": { "source": { "kind": "primitive", "shape": "cube", "texture": "<texture asset id>" },
          "material": { "tint": [1, 0.8, 0.6, 1], "lit": true } }
"mesh": { "source": { "kind": "model", "asset": "<model asset id>" } }

"collider": { "shape": { "kind": "box", "size": [1, 2, 0.2] }, "body": "kinematic" }
"collider": { "shape": { "kind": "capsule", "radius": 0.3, "height": 1.8 }, "sensor": true }

"audio": { "sound": "<sound asset id>", "bus": "effects", "looping": true, "autoplay": true }

"behaviors": [ { "kind": "room.lever", "params": { "target": "<entity id>", "prompt": "Pull" } } ]

"light": { "color": [1, 0.7, 0.4], "intensity": 2, "range": 6 }
```

Asset IDs come from the `.meta.json` sidecars (or `copperframe inspect
--json`, which lists every asset). Entity IDs come from `copperframe
inspect`.

## Examples in the repository

The example scene was built entirely by batches, applied in this order:

| File | What it adds |
| --- | --- |
| `examples/room/build_room.batch.json` | Floor, walls, pillars, crates, imported models and texture |
| `examples/room/build_demo.batch.json` | Colliders, the door, lever, corridor, exit and player |
| `examples/room/fixed_camera.batch.json` | The corridor's fixed camera zone |
| `examples/room/visuals.batch.json` | The caretaker, lights and blob shadows |
