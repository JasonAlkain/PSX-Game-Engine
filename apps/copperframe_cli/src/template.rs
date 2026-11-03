//! The Rust game crate `copperframe new` writes into `<project>/game`.
//!
//! The crate depends on this engine checkout by path, carries the engine's
//! lock file (so it starts from the tested dependency versions) and its
//! toolchain file, and is its own Cargo workspace.

use std::path::Path;

/// The engine source tree this CLI was built from, if it still exists.
pub fn engine_root() -> Option<std::path::PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = std::fs::canonicalize(root).ok()?;
    copperframe_assets::game::is_engine_root(&root).then_some(root)
}

pub fn crate_name(project_name: &str) -> String {
    let mut s: String = project_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    s = s.trim_matches('_').to_owned();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        s = format!("game_{s}");
    }
    s
}

/// Writes `game/` into the project. Fails if it exists.
pub fn write_game_crate(project_root: &Path, name: &str, engine: &Path) -> Result<(), String> {
    let dir = project_root.join(copperframe_assets::game::GAME_DIR);
    if dir.exists() {
        return Err(format!("{} already exists", dir.display()));
    }
    let krate = crate_name(name);
    let engine_path = engine.to_string_lossy().replace('\\', "/");
    if engine_path.contains('\'') {
        return Err("the engine path contains a quote character; move the engine checkout".into());
    }
    let title = name.replace('\\', "\\\\").replace('"', "\\\"");
    let files: [(&str, String); 5] = [
        (
            "Cargo.toml",
            format!(
                r#"[package]
name = "{krate}"
version = "0.1.0"
edition = "2024"
publish = false

[dependencies]
copperframe_game = {{ path = '{engine_path}/crates/copperframe_game' }}
copperframe_core = {{ path = '{engine_path}/crates/copperframe_core' }}
glam = {{ version = "0.34", features = ["serde"] }}

# The game is its own workspace, independent of the engine's.
[workspace]
"#
            ),
        ),
        (
            "src/main.rs",
            format!(
                r#"//! Game executable. `cargo run` plays the project; `cargo run -- --help`
//! lists headless and editor options.

fn main() -> std::process::ExitCode {{
    // During development the project is the folder above this crate; an
    // exported build uses the `data` folder next to the executable.
    copperframe_game::main(
        || Box::new({krate}::MyGame),
        concat!(env!("CARGO_MANIFEST_DIR"), "/.."),
    )
}}
"#
            ),
        ),
        (
            "src/lib.rs",
            format!(
                r#"//! Gameplay code. Register behaviour kinds here; attach them to
//! entities in the editor (Behaviours section of the inspector), where their
//! fields appear after Play or "Refresh behaviours".

use copperframe_game::{{Behavior, BehaviorDescriptor, Ctx, Entity, FieldDescriptor, Game, Registry}};

pub struct MyGame;

impl Game for MyGame {{
    fn title(&self) -> String {{
        "{title}".into()
    }}

    fn register(&self, r: &mut Registry) {{
        r.register(BehaviorDescriptor {{
            kind: "game.bob",
            description: "Bobs the entity up and down.",
            fields: vec![
                FieldDescriptor::number("height", 0.25, "distance (m)"),
                FieldDescriptor::number("speed", 1.0, "cycles per second"),
            ],
            create: |p| {{
                Ok(Box::new(Bob {{
                    height: p.number("height"),
                    speed: p.number("speed"),
                    base: None,
                    time: 0.0,
                }}))
            }},
        }});
    }}

    fn scene_started(&mut self, ctx: &mut Ctx) {{
        ctx.show_message("WASD to move, mouse to look, Esc to pause", 4.0);
    }}
}}

struct Bob {{
    height: f32,
    speed: f32,
    base: Option<glam::Vec3>,
    time: f32,
}}

impl Behavior for Bob {{
    fn start(&mut self, ctx: &mut Ctx, me: Entity) {{
        self.base = ctx.local(me).map(|t| t.translation);
    }}

    fn fixed_update(&mut self, ctx: &mut Ctx, me: Entity) {{
        self.time += ctx.dt();
        if let (Some(base), Some(mut t)) = (self.base, ctx.local(me)) {{
            let phase = self.time * self.speed * std::f32::consts::TAU;
            t.translation = base + glam::Vec3::Y * self.height * phase.sin();
            ctx.set_local(me, t);
        }}
    }}
}}
"#
            ),
        ),
        (".gitignore", "target/\n".to_owned()),
        (
            "rust-toolchain.toml",
            std::fs::read_to_string(engine.join("rust-toolchain.toml"))
                .map_err(|e| format!("reading the engine's rust-toolchain.toml: {e}"))?,
        ),
    ];
    for (rel, text) in files {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    // Start from the engine's tested dependency versions.
    let lock = engine.join("Cargo.lock");
    if lock.is_file() {
        std::fs::copy(&lock, dir.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crate_names_are_valid_identifiers() {
        assert_eq!(crate_name("My Game!"), "my_game");
        assert_eq!(crate_name("7 Days"), "game_7_days");
        assert_eq!(crate_name("***"), "game_");
    }
}
