# Asset pipeline

Assets are ordinary files in the project's `assets/` folder. Copperframe
reads them directly; there is no separate cooking step.

| Kind | Extensions | Used by |
| --- | --- | --- |
| Model | `.gltf`, `.glb` (glTF 2.0) | Mesh components; colliders (mesh shape); animators |
| Texture | `.png`, `.jpg`, `.jpeg` | Primitive meshes; models reference their own textures |
| Sound | `.wav`, `.ogg` (Ogg Vorbis) | Audio source components |

## Registering assets

Each source file gets a **sidecar** next to it, `<file>.meta.json`, that
holds the asset's permanent ID and import settings:

```json
{
  "format": "psx_asset_meta",
  "version": 1,
  "id": "cc9ec486-405b-4579-a154-28bccb14eac2",
  "importer": "gltf",
  "settings": { "scale": 1.0, "max_texture_size": 1024 }
}
```

Scenes refer to assets by this ID, not by path. To rename or move an asset,
move the source **and its `.meta.json` together**; every scene reference
keeps working. Deleting a sidecar gives the file a new ID on the next
registration and breaks references to the old one.

Register new files with either:

- the editor's **Assets** tab, **Rescan assets folder**; or
- `copperframe import --project DIR [FILE]`, which also test-imports and
  reports problems (with no file it registers everything unregistered).

`copperframe validate` reports duplicate IDs, sidecars without a source,
sources without a sidecar, failed imports and scene references to unknown
assets or assets of the wrong kind.

### Import settings

| Setting | Default | Meaning |
| --- | --- | --- |
| `scale` | 1.0 | Uniform scale applied to model positions (glTF units are metres) |
| `max_texture_size` | 1024 | Textures larger than this (1–8192) are reduced with nearest sampling, keeping hard texels. PS1-style projects often use 128 or 256 |

Edit the sidecar, then **Reimport** in the editor.

## Models (glTF)

Export glTF 2.0 from Blender or any other tool. Both `.gltf` (with external
or embedded buffers and images) and binary `.glb` work.

| Feature | Support |
| --- | --- |
| Triangle lists, strips and fans | Supported |
| Positions, normals, `TEXCOORD_0`, `COLOR_0`; 8/16/32-bit indices | Supported. Normals are computed when missing |
| Node hierarchy of the default scene | Baked into the vertices; mirrored nodes get their winding flipped |
| Base colour factor and texture (PNG/JPEG), alpha mode and cutoff, double-sided, emissive factor, `KHR_materials_unlit` | Supported |
| One skin per model, up to 64 joints, `JOINTS_0`/`WEIGHTS_0`, inverse bind matrices | Supported |
| Animation channels on joints (translation, rotation, scale; STEP and LINEAR) | Supported. CUBICSPLINE is imported as LINEAR with a warning |
| Points, lines, morph targets, a second skin, channels on non-joint nodes, extra UV sets, metallic/roughness, normal/occlusion/emissive textures, samplers | Ignored with a warning |
| Draco, meshopt and other required extensions, sparse accessors | Rejected |
| Out-of-range data, non-finite values, `http(s)` URIs, files outside the project | Rejected |

Texture filtering is a style setting, so glTF sampler settings are ignored.

Tips for the retro look:

- Keep polygon counts low and textures small (64–256 px). Large polygons
  show affine warping more.
- Vertex colours (`COLOR_0`) are kept, so lighting or tinting baked into
  them in your modelling tool carries over.
- Use unlit materials (`KHR_materials_unlit`) for signs, skies and
  emissive props.

## Animated characters

A skinned glTF with animation clips imports its skeleton and clips. List
them with:

```sh
copperframe import --project examples/room examples/room/assets/characters/caretaker.glb
```

which reports the joint count and clip names (the file path is relative
to the current folder). Then give the entity an
**Animator** component with a clip, or play clips from code with
`Ctx::play_animation` (which crossfades). `copperframe validate` warns when
an animator names a clip the model does not have.

Limits: one skin per model, 64 joints, no morph targets, two-clip
crossfades (no layered or additive blending).

## Textures

PNG and JPEG are decoded with limits (8192 px per side, 512 MiB) and
uploaded with a full mip chain, so the style can switch filtering and
mipmaps at any time. Use textures on primitives through the mesh
inspector's **Texture** box.

## Sounds

WAV and Ogg Vorbis files are decoded when a scene that uses them loads and
released when a scene that does not use them loads. Attach them with an
**Audio source** component, or play them from code through the entity's
audio source.

## When an import fails

Importers never crash the editor. A broken or unsupported file shows as
*failed* in the Assets tab with a message in the Console, and entities
using it draw as a magenta and black checker cube so they stay visible and
selectable. If a reimport fails after an earlier success, the last good
version stays on screen.

## Example assets

The example project's art (crates, pillars, floor texture and the skinned
caretaker) is generated by code:

```sh
cargo run -p copperframe_assets --example make_room_assets
```

Its sounds are generated by `examples/room/tools/make_sounds.py`.
