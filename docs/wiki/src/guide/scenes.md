# Scenes, entities and components

A scene is a JSON document in `scenes/` (for example
`scenes/main.scene.json`). It holds a list of **entities**, each with a
persistent ID, a name, an optional parent, a transform and optional
**components**, plus the scene's **settings** (style, lighting, start
camera).

The scene file is the authored data. At play time the engine builds a
separate runtime world from it; gameplay never writes the scene back. That
is why Play mode and headless tests can never damage your work.

## Coordinates and units

- Right-handed, **+Y up**, 1 unit = 1 metre.
- Cameras and "forward" look along local **−Z**; +X is right.
- Rotation in the editor (and `rotation_euler_deg` in a `set_transform`
  batch command) is Euler degrees `[x, y, z]`, applied as yaw (Y), then
  pitch (X), then roll (Z). Positive yaw turns left. Inside the file
  rotations are quaternions `[x, y, z, w]`.
- Scale components must be at least `0.0001` in magnitude (negative values
  mirror).
- A child's world transform is `parent_world × translation × rotation ×
  scale`.

## Entities

| Field | Meaning |
| --- | --- |
| `id` | A UUID that never changes. Scene references (a lever's `target`, a camera zone's `camera`) use it |
| `name` | Up to 256 bytes. Gameplay can look entities up by name |
| `parent` | Another entity's ID, or none. Hierarchies may be up to 64 deep; cycles are rejected |
| `transform` | Local translation, rotation and scale |

The order of entities in the file is the order in the hierarchy panel.

## Components

Every component is optional. The editor's inspector edits them; batches
set them with the `set_*` operations listed in
[Command batches](../reference/batches.md).

### Mesh

What the entity draws.

- **Source**: a primitive (`cube` 1×1×1, `plane` 1×1 facing +Y, `sphere`
  1 m diameter, `quad` 1×1 facing +Z) with an optional texture asset, or an
  imported **model** asset.
- **Tile size** (`tile_size`, primitives except spheres): how many metres one
  repeat of the texture covers, measured on the entity's size in the world
  including its parents' scale. Scaling the entity then shows more of the
  texture instead of stretching it. Without it the texture covers each face
  once, however large the entity is. New primitives start at 1 m.
- **Material overrides** replace the model's values when set: `tint`
  (linear RGBA), `alpha` (`opaque`, `cutout` with a threshold, or `blend`),
  `lit`, `flat_shading`, `allow_affine` (set false to keep this mesh's
  textures perspective-correct even when the style uses affine mapping) and
  `double_sided`.

Blended (transparent) meshes are sorted back to front per object, not per
triangle.

### Collider

Makes the entity solid or a trigger. Without a rigidbody it is static: it
never moves.

| Field | Values |
| --- | --- |
| `shape` | `box` (`size`, full edge lengths), `sphere` (`radius`), `capsule` (`radius`, `height` including the caps; vertical), or `mesh` (the entity's own triangles; static only) |
| `sensor` | Reports overlaps as trigger events but never blocks anything |
| `offset` | Shape centre relative to the entity origin |
| `layer`, `mask` | Collision layer 0–15 and a bit mask of layers it interacts with (default: all) |
| `friction` | 0–10 (default 0.5) |
| `restitution` | Bounciness 0–1 (default 0). The bouncier of two touching surfaces wins |

### Rigidbody

Makes the entity a moving physics body. Root entities only. See
[Physics](physics.md).

| Field | Values |
| --- | --- |
| `kinematic` | `false` (default): dynamic, moved by gravity, contacts and forces. `true`: moved only by its transform (doors, lifts); pushes dynamic bodies but is never pushed |
| `mass` | Kilograms (default 1) |
| `gravity_scale` | Multiplies the project's gravity, −100 to 100 (default 1; 0 floats) |
| `linear_damping`, `angular_damping` | 0–100: slow movement and spinning over time (defaults 0 and 0.05) |
| `lock_rotation` | Contacts never rotate the body (keeps it upright) |

Collider sizes are multiplied by the entity's world scale. Colliders of
skinned models use the model's rest pose. Turn on **Colliders** in the toolbar
to see them as wireframes.

### Audio source

A sound played from the entity.

| Field | Values |
| --- | --- |
| `sound` | A WAV or Ogg Vorbis asset |
| `bus` | `effects` (default) or `music` |
| `volume_db` | −60 to +12 |
| `looping`, `autoplay` | Loop the sound; start it when the scene starts (otherwise gameplay code plays it) |
| `spatial` | Positional 3D sound (default on); off plays it flat |
| `min_distance`, `max_distance` | Full volume within min (default 1 m), silent beyond max (default 20 m) |

### Behaviours

A list of `{kind, params}` entries naming gameplay code, for example
`{"kind": "psx.player"}` or `{"kind": "room.lever", "params": {"target":
"<entity id>"}}`. The game executable registers the kinds and their
parameters. See [Behaviours and game code](behaviours.md). Behaviours run
in list order.

### Component order

`component_order` (scene version 5, optional) is the order the inspector
lists the entity's components in, for example `["behavior:room.lever",
"mesh", "collider"]`. Keys are `mesh`, `animator`, `light`, `shadow`,
`collider`, `audio` and `behavior:<kind>`. Components it leaves out follow
in the default order. It may only name components the entity has, each
once; removing a component drops it from the list. The editor writes it
when you reorder cards, and keeps `behaviors` sorted the same way. Batches
set it with `set_component_order`.

### Point light

A local light: `color` (linear 0–1), `intensity` (0–16) and `range`
(0.05–100 m). Light falls off smoothly to exactly zero at the range. At most
8 lights per view are used, the nearest to the camera. Lights are evaluated
per vertex in the PS1 style (the mesh's triangles show in the light pools)
and per pixel in the PS2 style. Lights do not cast shadows.

### Blob shadow

A soft dark disc drawn on lit, upward-facing surfaces below the entity:
`radius` (0.05–20, scaled by the entity's horizontal scale) and `offset`
(the bottom of the caster relative to the entity origin). At most 8 per
view. Unlit materials never receive them.

### Animator

Plays a skeletal animation clip on a skinned model: `clip` (the name in the
model; `copperframe import` lists them), `speed` (−10 to 10; negative plays
backwards), `looping`, `autoplay`. Gameplay code can crossfade to other
clips with `Ctx::play_animation`. Requires a model mesh with a skin.

## Scene settings

| Setting | Meaning |
| --- | --- |
| `look` | `project` (the project's look from `project.json` `style.look`, with this scene's fog and atmosphere) or `scene` (the scene's own `style`). Missing in scenes before version 7, which means `scene` |
| `style` | The scene's retro style. See [PS1 and PS2 style settings](style.md). With `look: project` only its `fog` and `atmosphere` are used |
| `lighting` | Directional light `direction` and `color`, and `ambient` colour (linear) |
| `camera` | Start camera: `position`, `yaw_deg`, `pitch_deg`, `fov_deg` |

## Revisions and safety

`revision` counts saves and applied batches. Tools compare it to detect a
file that changed under them. Loading rejects unknown fields, unknown
format versions, duplicate IDs, missing parents, cycles, and out-of-range
or non-finite values, so an invalid scene is never loaded or saved. Every
save keeps the previous file as `.bak`.

Scene files are versioned (`psx_scene` v3). Older v1 and v2 scenes load
unchanged and are saved as v3 next time. See
[File formats](../reference/formats.md).
