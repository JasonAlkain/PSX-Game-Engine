# Glossary

**Affine texture mapping.** Interpolating texture coordinates linearly
across the screen instead of with perspective correction. The PS1 did this,
so textures warp on large, angled polygons.

**Asset ID.** The UUID in an asset's `.meta.json` sidecar. Scenes refer to
assets by it, so files can move as long as the sidecar moves with them.

**Batch.** A JSON list of scene commands applied all or nothing by
`copperframe apply`. See [Command batches](../reference/batches.md).

**Behaviour.** A piece of gameplay code attached to an entity by its
**kind** (for example `room.door`) with parameters. Registered by the game
crate. See [Behaviours](../guide/behaviours.md).

**Blob shadow.** A soft dark disc drawn under an entity on the surfaces
below it, the cheap shadow technique of the era.

**Clean.** The style preset with every retro effect off; used as a
reference.

**Colour reduction.** Quantising output colour to a few bits per channel,
which causes banding. Usually paired with **dithering**.

**Command.** A single scene edit (`create_entity`, `set_transform`, …).
Every edit in the editor and the CLI is one.

**Ctx.** The handle gameplay code uses to read and change the world. See
the [reference](../reference/ctx.md).

**Dithering.** A fixed 4×4 pattern added before colour reduction so
gradients turn into patterns instead of bands.

**Entity.** An object in a scene: ID, name, parent, transform, components.
At run time, `Entity` also names the runtime handle for one.

**Export.** A standalone folder with the game executable, its data and
licence notices. See [Exporting](../guide/exporting.md).

**Game crate.** The Rust crate in a project's `game/` folder that holds its
behaviours and builds its executable.

**Headless run.** Playing a scene with no window, GPU or audio, driven by a
script, producing a JSON report. See [Testing](../guide/testing.md).

**Integer scaling.** Upscaling the internal image by a whole-number factor
so every pixel becomes an even block, with black bars around it.

**Internal resolution.** The small image the world is rendered into (for
example 320×240) before being upscaled to the window.

**Preset.** A named style: PS1, PS2 or Clean. A changed style is "Custom".

**Revision.** A counter in each scene incremented on every save or batch,
used to detect concurrent edits.

**Sensor.** A collider that reports overlaps (`on_trigger`) but does not
block movement.

**Sidecar.** The `<file>.meta.json` next to each asset.

**Style.** The set of retro effect settings for a scene (`RetroStyle`,
stored as `psx_style`). See [Style settings](../guide/style.md).

**Tick.** One fixed simulation step: 60 per second.

**Vertex snapping.** Rounding projected vertex positions to the pixel grid,
which makes PS1 geometry jitter as the camera moves.
