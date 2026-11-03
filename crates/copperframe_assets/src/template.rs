//! Creating projects: the starter templates shared by `copperframe new` and
//! the editor's project hub, and the Rust game crate written into
//! `<project>/game`.
//!
//! The crate depends on this engine checkout by path, carries the engine's
//! lock file (so it starts from the tested dependency versions) and its
//! toolchain file, and is its own Cargo workspace. Game code therefore needs
//! the engine source: [`engine_root`] finds it.

use std::path::{Path, PathBuf};

use copperframe_scene::Scene;

use crate::game::GAME_DIR;
use crate::project::{ASSETS_DIR, PROJECT_FILE, Project, SCENES_DIR};

/// The engine source tree this program was built from, if it still exists
/// (a downloaded build has none).
pub fn engine_root() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = std::fs::canonicalize(root).ok()?;
    crate::game::is_engine_root(&root).then_some(root)
}

/// Folder of the example project, relative to the engine source or to the
/// folder of a downloaded build.
pub const DEMO_DIR: &str = "examples/room";

/// Where the example project's data is: in the engine source, or next to
/// the running executable (the downloaded bundle).
pub fn demo_source() -> Option<PathBuf> {
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(DEMO_DIR)));
    engine_root()
        .map(|r| r.join(DEMO_DIR))
        .into_iter()
        .chain(beside_exe)
        .find(|d| d.join(PROJECT_FILE).is_file())
}

/// What a new project starts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectTemplate {
    /// One empty scene.
    Empty,
    /// A floor, a spinning cube and a player: playable at once.
    Starter,
    /// A copy of the example project, The Lever Room.
    LeverRoom,
}

impl ProjectTemplate {
    pub const ALL: [ProjectTemplate; 3] = [Self::Empty, Self::Starter, Self::LeverRoom];

    pub fn label(self) -> &'static str {
        match self {
            Self::Empty => "Empty",
            Self::Starter => "Starter scene",
            Self::LeverRoom => "The Lever Room demo",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Empty => "One empty scene. Start from nothing.",
            Self::Starter => {
                "A floor, a spinning cube and a first-person player. Press Play straight away."
            }
            Self::LeverRoom => {
                "A copy of the example game: a room with a lever, a door, sounds and an \
                 animated character, with its Rust game code."
            }
        }
    }

    /// The command-line name (`copperframe new --template NAME`).
    pub fn id(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Starter => "starter",
            Self::LeverRoom => "lever-room",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.id() == id)
    }

    /// The template's behaviours are Rust code, so it needs the game crate.
    pub fn needs_game_code(self) -> bool {
        self == Self::LeverRoom
    }
}

/// Options for [`create_project`].
#[derive(Debug, Clone)]
pub struct NewProject {
    /// The project folder. It must not exist yet, or be empty.
    pub dir: PathBuf,
    pub name: String,
    pub template: ProjectTemplate,
    /// Write a Rust game crate into `game/` (needs [`engine_root`]).
    pub game_code: bool,
}

/// What [`create_project`] made.
#[derive(Debug, Clone)]
pub struct CreatedProject {
    pub project: Project,
    /// The engine source the game crate uses, if one was written.
    pub game_crate_engine: Option<PathBuf>,
}

/// Creates a project from a template. On failure after the folder was
/// created, the partial project is left for the user to inspect.
pub fn create_project(options: &NewProject) -> Result<CreatedProject, String> {
    let name = options.name.trim();
    if name.is_empty() {
        return Err("the project needs a name".into());
    }
    let dir = &options.dir;
    if dir.join(PROJECT_FILE).exists() {
        return Err(format!("{} is already a project", dir.display()));
    }
    if dir.is_file() {
        return Err(format!("{} is a file", dir.display()));
    }
    if std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some()) {
        return Err(format!(
            "{} already exists and is not empty; choose another name or location",
            dir.display()
        ));
    }
    if options.template.needs_game_code() && !options.game_code {
        return Err(format!(
            "the {} template needs its Rust game code (its lever, door and caretaker are \
             behaviours written in Rust)",
            options.template.label()
        ));
    }
    let engine = if options.game_code {
        Some(engine_root().ok_or(
            "this copy of Copperframe was not built from the engine source, so it cannot \
             create Rust game code. Untick \"Include Rust game code\", or use an engine \
             checkout from GitHub.",
        )?)
    } else {
        None
    };
    let demo = if options.template == ProjectTemplate::LeverRoom {
        Some(demo_source().ok_or("the example project (examples/room) was not found")?)
    } else {
        None
    };
    let mut project = Project::create(dir, name).map_err(|e| e.to_string())?;
    match options.template {
        ProjectTemplate::Starter => {}
        ProjectTemplate::Empty => {
            let mut scene = Scene::new("Main");
            // The starter scene's camera and style, without its entities.
            if let Ok(starter) = copperframe_scene::load_scene(&project.startup_scene_path()) {
                scene.settings = starter.settings;
            }
            copperframe_scene::save_scene(&scene, &project.startup_scene_path())
                .map_err(|e| e.to_string())?;
        }
        ProjectTemplate::LeverRoom => {
            let demo = demo.as_deref().unwrap_or(Path::new("."));
            copy_demo_data(demo, &mut project)?;
        }
    }
    if let Some(engine) = &engine {
        match (options.template, demo.as_deref()) {
            (ProjectTemplate::LeverRoom, Some(demo)) if demo.join(GAME_DIR).is_dir() => {
                write_demo_game_crate(dir, name, engine, &demo.join(GAME_DIR))?;
            }
            _ => write_game_crate(dir, name, engine)?,
        }
    }
    Ok(CreatedProject {
        project,
        game_crate_engine: engine,
    })
}

/// Copies the example project's data (not its editor state or tools) and
/// keeps the new project's name.
fn copy_demo_data(demo: &Path, project: &mut Project) -> Result<(), String> {
    let source = Project::open(demo).map_err(|e| e.to_string())?;
    let root = project.root.clone();
    let scenes = root.join(SCENES_DIR);
    std::fs::remove_dir_all(&scenes).map_err(|e| format!("{}: {e}", scenes.display()))?;
    for dir in [ASSETS_DIR, SCENES_DIR, "scripts"] {
        let from = demo.join(dir);
        if from.is_dir() {
            copy_dir(&from, &root.join(dir))?;
        }
    }
    project.file.startup_scene = source.file.startup_scene;
    project.save().map_err(|e| e.to_string())
}

/// Copies a folder tree (files and folders only).
pub fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let entries = std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let target = to.join(entry.file_name());
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            copy_dir(&path, &target)?;
        } else if kind.is_file() {
            std::fs::copy(&path, &target).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
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
    let dir = project_root.join(GAME_DIR);
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
//! fields appear after Play or Game > "Reload behaviour kinds".

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

/// Writes `game/` with the example project's gameplay code: the usual
/// crate scaffolding (manifest, toolchain, lock file) with the demo's
/// sources in place of the starter behaviour.
fn write_demo_game_crate(
    project_root: &Path,
    name: &str,
    engine: &Path,
    demo_game: &Path,
) -> Result<(), String> {
    write_game_crate(project_root, name, engine)?;
    let src = project_root.join(GAME_DIR).join("src");
    let krate = crate_name(name);
    for file in ["lib.rs", "main.rs"] {
        let from = demo_game.join("src").join(file);
        let text =
            std::fs::read_to_string(&from).map_err(|e| format!("{}: {e}", from.display()))?;
        let text = text
            .replace("lever_room::", &format!("{krate}::"))
            .replace("`cargo run -p lever_room`", "`cargo run`");
        std::fs::write(src.join(file), text).map_err(|e| format!("{file}: {e}"))?;
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

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("cf_template_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn new(
        dir: &Path,
        template: ProjectTemplate,
        game_code: bool,
    ) -> Result<CreatedProject, String> {
        create_project(&NewProject {
            dir: dir.to_path_buf(),
            name: "My Game".into(),
            template,
            game_code,
        })
    }

    #[test]
    fn templates_create_valid_projects() {
        for template in ProjectTemplate::ALL {
            let dir = temp(template.id());
            let game_code = template.needs_game_code();
            let created = new(&dir, template, game_code).unwrap();
            assert_eq!(created.project.file.name, "My Game");
            assert_eq!(dir.join(GAME_DIR).exists(), game_code);
            let scene =
                copperframe_scene::load_scene(&created.project.startup_scene_path()).unwrap();
            match template {
                ProjectTemplate::Empty => assert!(scene.entities.is_empty()),
                ProjectTemplate::Starter => assert_eq!(scene.entities.len(), 3),
                ProjectTemplate::LeverRoom => {
                    assert!(scene.entities.len() > 10);
                    assert!(dir.join("assets/props/crate.glb.meta.json").is_file());
                    assert!(dir.join("scripts/walkthrough.script.json").is_file());
                    assert!(!dir.join(".psx").exists(), "editor state is not copied");
                }
            }
            let report = crate::validate_project(&dir);
            assert_eq!(report.errors, 0, "{template:?}: {:?}", report.diagnostics);
            assert_eq!(ProjectTemplate::from_id(template.id()), Some(template));
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }

    #[test]
    fn lever_room_requires_game_code() {
        let dir = temp("demo_no_game");
        let err = new(&dir, ProjectTemplate::LeverRoom, false).unwrap_err();
        assert!(err.contains("needs its Rust game code"), "{err}");
        assert!(!dir.exists(), "nothing is created");
    }

    #[test]
    fn game_code_uses_this_engine_checkout() {
        let dir = temp("demo_game");
        let created = new(&dir, ProjectTemplate::LeverRoom, true).unwrap();
        assert!(
            created.game_crate_engine.is_some(),
            "tests run from the engine source"
        );
        let main = std::fs::read_to_string(dir.join("game/src/main.rs")).unwrap();
        assert!(main.contains("my_game::LeverRoom"), "{main}");
        let lib = std::fs::read_to_string(dir.join("game/src/lib.rs")).unwrap();
        assert!(lib.contains("struct LeverRoom"));
        let manifest = std::fs::read_to_string(dir.join("game/Cargo.toml")).unwrap();
        assert!(manifest.contains("name = \"my_game\""));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn refuses_non_empty_folders_and_blank_names() {
        let dir = temp("busy");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), "keep me").unwrap();
        assert!(
            new(&dir, ProjectTemplate::Starter, false)
                .unwrap_err()
                .contains("not empty")
        );
        assert!(!dir.join(PROJECT_FILE).exists());
        let empty = dir.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert!(
            new(&empty, ProjectTemplate::Starter, false).is_ok(),
            "an empty folder is fine"
        );
        let err = create_project(&NewProject {
            dir: dir.join("x"),
            name: "  ".into(),
            template: ProjectTemplate::Empty,
            game_code: false,
        })
        .unwrap_err();
        assert!(err.contains("name"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
