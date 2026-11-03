# Installing and building

Copperframe is distributed as source. You build it with Cargo, Rust's build
tool. Prebuilt Windows editor downloads will be published on
[itch.io](https://allyn-david.itch.io/copperframe-engine) once releases start.

## Requirements

| What | Details |
| --- | --- |
| Rust | Install with [rustup](https://rustup.rs). The exact toolchain (Rust 1.97, edition 2024) is pinned in `rust-toolchain.toml` and installed automatically the first time you build. No nightly. |
| Graphics | A GPU and driver supporting Vulkan, DirectX 12, Metal or OpenGL ES 3. Only WebGPU default limits are needed. |
| Operating system | Windows 11 x86-64 is the primary target. Linux builds and runs (tested on X11). macOS is untested. |
| Linux packages | X11/Wayland client libraries including `libxkbcommon-x11`, and ALSA headers (`libasound2-dev`) to build the audio backend. |
| Audio | Optional at run time. Without an output device games run silently and log a warning. |

On Ubuntu or Debian:

```sh
sudo apt install build-essential pkg-config libasound2-dev libxkbcommon-x11-0
```

On Windows, rustup installs the MSVC toolchain; accept its prompt to install
the Visual Studio C++ build tools if you do not have them.

## Getting the source

```sh
git clone https://github.com/JasonAlkain/PSX-Game-Engine.git
cd PSX-Game-Engine
```

Keep the checkout somewhere permanent. Projects you create with
`copperframe new` point their game crate at this folder by path, so moving
or deleting it breaks those projects' builds.

## Building

```sh
cargo build --workspace --release
```

The first build downloads and compiles all dependencies and takes several
minutes. Release binaries land in `target/release/`:

| Binary | What it is |
| --- | --- |
| `copperframe_editor` | The scene editor |
| `copperframe` | The [command-line tool](../reference/cli.md) |
| `copperframe_player` | The stock player for projects without their own game code |
| `copperframe_sandbox` | The renderer comparison scene, for trying style effects |
| `lever_room` | The example game, *The Lever Room* |

You can also skip the separate build step and let `cargo run` build what it
needs, as the rest of this wiki does:

```sh
cargo run -p copperframe_editor --release -- examples/room
```

Use `--release` for anything interactive. Debug builds work but render and
simulate much more slowly.

## Checking the build

```sh
cargo test --workspace
```

GPU tests print `SKIPPED` when no graphics adapter is available. On Linux
without a GPU, install Mesa's software Vulkan driver (`mesa-vulkan-drivers`)
to run them. See [Contributing](../dev/contributing.md) for the full set of
checks.

## Choosing a graphics backend

wgpu picks the best backend automatically. To force one, set
`WGPU_BACKEND` to `vulkan`, `dx12`, `metal` or `gl` before starting any
Copperframe program. The OpenGL ES fallback is untested; see
[Known limitations](../help/limitations.md).

## Next steps

- [Play the demo](demo.md) to see what the engine does.
- [Create your first project](first-project.md).
